//! Authorization Code with PKCE for a native desktop app.
//!
//! Current Spotify rules this implements (verified July 2026, see README):
//! * No client secret — PKCE only. The client id is not a secret.
//! * Redirect URIs must be HTTPS **except** the explicit loopback IP;
//!   `http://127.0.0.1:<port>/callback` is allowed, the hostname
//!   `localhost` is not. The URI registered in the developer dashboard must
//!   match byte-for-byte.
//! * Refresh tokens rotate: every refresh may return a new refresh token
//!   which MUST replace the stored one.
//!
//! Tokens persist to `tokens.json` (0600 on Unix) in the app config dir,
//! tagged with the client id they belong to.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// Everything this app ever asks for. Deliberately minimal:
/// * no `user-library-modify` — the app never changes Liked Songs;
/// * no playback-control scopes — out of scope for a playlist manager;
/// * no `ugc-image-upload` — no cover uploads.
pub const SCOPES: &[&str] = &[
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-public",
    "playlist-modify-private",
    "user-library-read",
    "user-top-read",
    "user-read-recently-played",
];

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("not connected to Spotify — connect from the Settings view")]
    NotAuthenticated,
    #[error(
        "port {0} is already in use; close the conflicting app or change the \
         redirect port (remember to update the URI in the Spotify dashboard)"
    )]
    PortInUse(u16),
    #[error("timed out waiting for the browser authorization (5 minutes)")]
    Timeout,
    #[error("authorization was denied in the browser: {0}")]
    Denied(String),
    #[error("state parameter mismatch — possible CSRF; authorization aborted")]
    StateMismatch,
    #[error("token endpoint error: {0}")]
    Token(String),
    #[error("network error during authorization: {0}")]
    Network(#[from] reqwest::Error),
    #[error("i/o error during authorization: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredTokens {
    /// The client id these tokens were issued to; a mismatch invalidates.
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds after which `access_token` is considered stale.
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct TokenErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn random_urlsafe(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    OsRng
        .try_fill_bytes(&mut buf)
        .expect("operating system RNG unavailable");
    URL_SAFE_NO_PAD.encode(buf)
}

/// RFC 7636 S256: BASE64URL(SHA256(verifier)).
pub fn code_challenge_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn expires_at_from(expires_in: i64) -> i64 {
    // 30s safety margin so we never present a token mid-expiry.
    now_unix() + expires_in - 30
}

/// Run the full interactive PKCE flow: bind the loopback listener, open the
/// browser, wait for the redirect, exchange the code. `notify` receives
/// human-readable status lines (including the authorize URL as a fallback if
/// the browser fails to open).
pub async fn run_pkce_flow(
    http: &reqwest::Client,
    client_id: &str,
    port: u16,
    notify: impl Fn(String),
) -> Result<StoredTokens, AuthError> {
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let verifier = random_urlsafe(64); // 86 chars, within RFC 7636's 43..=128
    let challenge = code_challenge_s256(&verifier);
    let state = random_urlsafe(24);

    // Bind BEFORE opening the browser so the redirect cannot race us.
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|_| AuthError::PortInUse(port))?;

    let authorize_url = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
            ("scope", SCOPES.join(" ").as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ],
    )
    .expect("static authorize URL is valid")
    .to_string();

    notify("Opening Spotify authorization in your browser…".to_string());
    if open::that(&authorize_url).is_err() {
        notify(format!(
            "Could not open a browser automatically. Paste this URL into one:\n{authorize_url}"
        ));
    } else {
        notify(format!(
            "If no browser appeared, open this URL manually:\n{authorize_url}"
        ));
    }

    let code = tokio::time::timeout(
        Duration::from_secs(300),
        wait_for_callback(&listener, &state),
    )
    .await
    .map_err(|_| AuthError::Timeout)??;

    notify("Authorization received; exchanging code for tokens…".to_string());

    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?;

    let tokens = parse_token_response(client_id, None, resp).await?;
    Ok(tokens)
}

/// Accept connections until the OAuth callback arrives; answer every request
/// with a small HTML page. Ignores stray requests (favicon etc.).
async fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String, AuthError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = Vec::with_capacity(2048);
        let mut chunk = [0u8; 1024];
        // Read just the request head; 16 KiB cap.
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf);
        let Some(first_line) = head.lines().next() else {
            continue;
        };
        let mut parts = first_line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if method != "GET" || !path.starts_with("/callback") {
            let _ = respond(&mut stream, 404, "Not the callback endpoint.").await;
            continue;
        }

        let parsed = url::Url::parse(&format!("http://127.0.0.1{path}"))
            .map_err(|e| AuthError::Token(format!("unparsable callback: {e}")))?;
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                _ => {}
            }
        }

        if state.as_deref() != Some(expected_state) {
            let _ = respond(&mut stream, 400, "State mismatch — authorization aborted.").await;
            return Err(AuthError::StateMismatch);
        }
        if let Some(err) = error {
            let _ = respond(
                &mut stream,
                200,
                "Authorization was denied. You can close this tab.",
            )
            .await;
            return Err(AuthError::Denied(err));
        }
        match code {
            Some(code) => {
                let _ = respond(
                    &mut stream,
                    200,
                    "Connected to Spotify — you can close this tab and return to Playlist Studio.",
                )
                .await;
                return Ok(code);
            }
            None => {
                let _ = respond(&mut stream, 400, "Missing authorization code.").await;
                continue;
            }
        }
    }
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    message: &str,
) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Playlist Studio</title>\
         <body style=\"font-family: system-ui; margin: 4rem auto; max-width: 32rem\">\
         <h2>Playlist Studio</h2><p>{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn parse_token_response(
    client_id: &str,
    previous_refresh: Option<&str>,
    resp: reqwest::Response,
) -> Result<StoredTokens, AuthError> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<TokenErrorBody>(&text)
            .map(|b| format!("{} ({})", b.error, b.error_description.unwrap_or_default()))
            .unwrap_or_else(|_| format!("HTTP {status}: {}", &text[..text.len().min(300)]));
        return Err(AuthError::Token(msg));
    }
    let body: TokenResponse = serde_json::from_str(&text)
        .map_err(|e| AuthError::Token(format!("unexpected token response: {e}")))?;
    let refresh_token = body
        .refresh_token
        .or_else(|| previous_refresh.map(str::to_string))
        .ok_or_else(|| AuthError::Token("no refresh token in response".into()))?;
    Ok(StoredTokens {
        client_id: client_id.to_string(),
        access_token: body.access_token,
        refresh_token,
        expires_at: expires_at_from(body.expires_in),
        scope: body.scope.unwrap_or_default(),
    })
}

/// Owns token persistence and refresh. Lives inside the single worker; no
/// interior locking needed.
pub struct TokenManager {
    client_id: String,
    path: PathBuf,
    tokens: Option<StoredTokens>,
}

impl TokenManager {
    /// Load persisted tokens if they exist AND belong to `client_id`.
    pub fn load(path: PathBuf, client_id: &str) -> Self {
        let tokens = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<StoredTokens>(&text).ok())
            .filter(|t| t.client_id == client_id && !t.refresh_token.is_empty());
        Self {
            client_id: client_id.to_string(),
            path,
            tokens,
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.tokens.is_some()
    }

    pub fn set(&mut self, tokens: StoredTokens) -> std::io::Result<()> {
        self.tokens = Some(tokens);
        self.persist()
    }

    pub fn clear(&mut self) {
        self.tokens = None;
        let _ = std::fs::remove_file(&self.path);
    }

    fn persist(&self) -> std::io::Result<()> {
        let Some(tokens) = &self.tokens else {
            return Ok(());
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(tokens)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// A currently-valid bearer token, refreshing (and persisting the rotated
    /// refresh token) when needed.
    pub async fn bearer(&mut self, http: &reqwest::Client) -> Result<String, AuthError> {
        let expired = match &self.tokens {
            None => return Err(AuthError::NotAuthenticated),
            Some(t) => t.expires_at <= now_unix(),
        };
        if expired {
            self.force_refresh(http).await?;
        }
        Ok(self
            .tokens
            .as_ref()
            .expect("checked above")
            .access_token
            .clone())
    }

    /// Refresh unconditionally (used on 401 responses as well as expiry).
    pub async fn force_refresh(&mut self, http: &reqwest::Client) -> Result<(), AuthError> {
        let Some(current) = &self.tokens else {
            return Err(AuthError::NotAuthenticated);
        };
        let resp = http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", current.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
            ])
            .send()
            .await?;
        match parse_token_response(&self.client_id, Some(&current.refresh_token), resp).await {
            Ok(tokens) => {
                self.set(tokens)?;
                Ok(())
            }
            Err(AuthError::Token(msg)) if msg.contains("invalid_grant") => {
                // Refresh token revoked/expired: force a clean re-connect.
                self.clear();
                Err(AuthError::NotAuthenticated)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        // RFC 7636 appendix B.
        assert_eq!(
            code_challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_length_is_within_rfc_bounds() {
        let v = random_urlsafe(64);
        assert!((43..=128).contains(&v.len()), "got {}", v.len());
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn token_manager_ignores_tokens_from_a_different_client_id() {
        let dir = std::env::temp_dir().join(format!("ps-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tokens.json");
        let tokens = StoredTokens {
            client_id: "client-A".into(),
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: now_unix() + 3600,
            scope: String::new(),
        };
        std::fs::write(&path, serde_json::to_string(&tokens).unwrap()).unwrap();

        assert!(TokenManager::load(path.clone(), "client-A").is_authenticated());
        assert!(!TokenManager::load(path.clone(), "client-B").is_authenticated());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_tokens_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ps-test-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tokens.json");
        let mut mgr = TokenManager::load(path.clone(), "client-A");
        mgr.set(StoredTokens {
            client_id: "client-A".into(),
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: now_unix() + 3600,
            scope: String::new(),
        })
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }
}
