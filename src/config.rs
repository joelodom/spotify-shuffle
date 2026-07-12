//! App configuration, persisted as TOML in the platform config directory
//! (e.g. `~/Library/Application Support/spotify-shuffle/` on macOS,
//! `~/.config/spotify-shuffle/` on Linux).
//!
//! Secrets policy: the Spotify *client id* is not a secret (PKCE apps have no
//! client secret at all). OAuth tokens are stored separately in
//! `tokens.json` with 0600 permissions. The Anthropic API key, if the user
//! switches providers, is read from an environment variable and never written
//! to disk by this app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "spotify-shuffle";

/// Data directory used before the app was renamed to Spotify Shuffle.
const LEGACY_DIR_NAME: &str = "playlist-studio";

/// One-time migration: earlier builds stored config/tokens under the old
/// name; move that directory to the new location if the new one doesn't
/// exist yet.
fn migrate_legacy_dir() {
    let Some(base) = dirs::config_dir() else {
        return;
    };
    let old = base.join(LEGACY_DIR_NAME);
    let new = base.join(APP_DIR_NAME);
    if old.is_dir() && !new.exists() {
        match std::fs::rename(&old, &new) {
            Ok(()) => tracing::info!("migrated data dir {old:?} -> {new:?}"),
            Err(e) => tracing::warn!("could not migrate legacy data dir: {e}"),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub spotify: SpotifyConfig,
    pub ai: AiConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpotifyConfig {
    /// Client ID of the user's own Spotify app (developer.spotify.com
    /// dashboard). Not a secret.
    pub client_id: String,
    /// Loopback port for the OAuth redirect. The registered redirect URI must
    /// be exactly `http://127.0.0.1:<port>/callback`.
    pub redirect_port: u16,
    /// Whether playlists created by this app default to public.
    pub create_public: bool,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            redirect_port: 8888,
            create_public: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AiProviderKind {
    /// Drive the locally installed, already-authenticated Claude Code CLI in
    /// headless mode — usage counts against the user's Claude subscription.
    ClaudeCode,
    /// Call the Anthropic Messages API directly (pay-per-token). Requires an
    /// API key in the environment.
    AnthropicApi,
}

impl AiProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            AiProviderKind::ClaudeCode => "Claude Code (subscription)",
            AiProviderKind::AnthropicApi => "Anthropic API (pay per token)",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiConfig {
    pub provider: AiProviderKind,
    /// Model for the Claude Code CLI (`--model`). Alias ("sonnet", "opus",
    /// "haiku", "fable") or full model id. Empty = whatever default model the
    /// user's own CLI is configured with — least surprise, and model access
    /// varies by subscription tier.
    pub claude_code_model: String,
    /// Absolute path to the `claude` binary. Empty = search PATH plus
    /// well-known install locations (GUI apps often launch without a shell
    /// PATH).
    pub claude_binary: String,
    /// Hard timeout for one CLI generation call.
    pub claude_timeout_secs: u64,
    /// Model id for the direct Anthropic API provider.
    pub anthropic_model: String,
    /// Name of the environment variable holding the Anthropic API key.
    pub anthropic_api_key_env: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: AiProviderKind::ClaudeCode,
            claude_code_model: String::new(),
            claude_binary: String::new(),
            claude_timeout_secs: 600,
            anthropic_model: "claude-opus-4-8".to_string(),
            anthropic_api_key_env: "ANTHROPIC_API_KEY".to_string(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn tokens_path() -> PathBuf {
    config_dir().join("tokens.json")
}

/// Empty working directory for headless `claude` runs, so the CLI never picks
/// up CLAUDE.md/settings from whatever project the app happened to be
/// launched from.
pub fn claude_workdir() -> PathBuf {
    config_dir().join("claude-workdir")
}

impl AppConfig {
    pub fn load() -> Self {
        migrate_legacy_dir();
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("config {path:?} unparsable ({e}); using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let text = toml::to_string_pretty(self)?;
        std::fs::write(config_path(), text)?;
        Ok(())
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.spotify.redirect_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = AppConfig::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.ai.provider, AiProviderKind::ClaudeCode);
        assert_eq!(back.spotify.redirect_port, 8888);
    }

    #[test]
    fn partial_config_fills_defaults() {
        let cfg: AppConfig = toml::from_str("[spotify]\nclient_id = \"abc123\"\n").unwrap();
        assert_eq!(cfg.spotify.client_id, "abc123");
        assert_eq!(cfg.spotify.redirect_port, 8888);
        assert_eq!(cfg.ai.provider, AiProviderKind::ClaudeCode);
    }

    #[test]
    fn provider_kind_kebab_case() {
        let cfg: AppConfig = toml::from_str("[ai]\nprovider = \"anthropic-api\"\n").unwrap();
        assert_eq!(cfg.ai.provider, AiProviderKind::AnthropicApi);
    }

    #[test]
    fn redirect_uri_uses_loopback_ip_not_localhost() {
        // Spotify no longer accepts "localhost" as a redirect host; the app
        // must always use the literal loopback IP.
        let cfg = AppConfig::default();
        assert_eq!(cfg.redirect_uri(), "http://127.0.0.1:8888/callback");
    }
}
