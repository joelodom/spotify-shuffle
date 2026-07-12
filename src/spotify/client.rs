//! Raw typed Spotify Web API client — February 2026 API surface.
//!
//! Implements ONLY endpoints verified to work for NEW development-mode apps
//! as of July 2026 (see the README's API-status table, researched with
//! citations). Notable current realities this client encodes:
//!
//! * Playlist item operations live at `/playlists/{id}/items` — the old
//!   `/tracks` paths are deprecated. Page limit is 50.
//! * Playlist creation is `POST /me/playlists` (the `/users/{id}/playlists`
//!   path was removed).
//! * "Deleting" a playlist is removing its URI from the library:
//!   `DELETE /me/library?uris=spotify:playlist:{id}` — with a fallback to the
//!   deprecated-but-still-live `DELETE /playlists/{id}/followers`.
//! * Search is capped at `limit=10`.
//! * Batch artist lookup, recommendations, audio features, related artists,
//!   featured playlists and browse endpoints are unavailable — none are used.
//! * `popularity` and several other fields are stripped in development mode;
//!   every non-essential model field is optional.
//!
//! Visibility is part of the safety model: read endpoints are `pub`,
//! mutating endpoints are `pub(super)` so only `spotify::service` (which
//! demands safety grants) can reach them.

use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::auth::{AuthError, TokenManager};
use super::models::*;

const API: &str = "https://api.spotify.com/v1";

/// Max URIs per add/replace request, fixed by the API.
pub const TRACK_WRITE_CHUNK: usize = 100;
/// Courtesy delay between pagination requests (dev-mode rate buckets are
/// small; the rolling window is 30 s).
const PAGE_DELAY: Duration = Duration::from_millis(120);

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Spotify API error {status}: {message}")]
    Status { status: u16, message: String },
    #[error("unexpected Spotify response: {0}")]
    Decode(String),
}

#[derive(Deserialize)]
struct ApiErrorBody {
    error: ApiErrorInner,
}
#[derive(Deserialize)]
struct ApiErrorInner {
    #[serde(default)]
    message: String,
}

/// Which route a playlist deletion ultimately used (for the operation log).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteRoute {
    /// `DELETE /me/library` — the current endpoint.
    Library,
    /// `DELETE /playlists/{id}/followers` — deprecated fallback.
    LegacyUnfollow,
}

/// Progress callback: (items fetched/written so far, total if known).
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>);

pub struct SpotifyClient {
    http: reqwest::Client,
    tokens: TokenManager,
}

impl SpotifyClient {
    pub(super) fn new(tokens: TokenManager) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self { http, tokens }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(super) fn tokens_mut(&mut self) -> &mut TokenManager {
        &mut self.tokens
    }

    pub fn is_authenticated(&self) -> bool {
        self.tokens.is_authenticated()
    }

    /// Send with auth, one 401-triggered refresh, 429 Retry-After honoring,
    /// and a light 5xx retry. Returns the successful response.
    async fn execute(
        &mut self,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response, ApiError> {
        let mut refreshed_after_401 = false;
        let mut server_retries = 0u8;
        let mut rate_limit_waits = 0u8;
        loop {
            let bearer = self.tokens.bearer(&self.http).await?;
            let mut req = self.http.request(method.clone(), url).bearer_auth(bearer);
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            match status {
                200..=299 => return Ok(resp),
                401 if !refreshed_after_401 => {
                    refreshed_after_401 = true;
                    self.tokens.force_refresh(&self.http).await?;
                }
                429 if rate_limit_waits < 5 => {
                    rate_limit_waits += 1;
                    let wait = resp
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(2)
                        .min(120);
                    tracing::warn!("rate limited; waiting {wait}s (attempt {rate_limit_waits})");
                    tokio::time::sleep(Duration::from_secs(wait + 1)).await;
                }
                500..=504 if server_retries < 2 => {
                    server_retries += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                _ => {
                    let text = resp.text().await.unwrap_or_default();
                    let mut message = serde_json::from_str::<ApiErrorBody>(&text)
                        .map(|b| b.error.message)
                        .unwrap_or_else(|_| text.chars().take(300).collect());
                    if status == 403 {
                        message.push_str(
                            " — hints: development-mode apps can only read the contents of \
                             playlists the user OWNS or collaborates on; the account must be \
                             allowlisted under 'User Management' in the Spotify dashboard; and \
                             scope changes require disconnecting and reconnecting.",
                        );
                    }
                    return Err(ApiError::Status { status, message });
                }
            }
        }
    }

    async fn request_json<T: DeserializeOwned>(
        &mut self,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, ApiError> {
        let resp = self.execute(method, url, body).await?;
        let text = resp.text().await?;
        serde_json::from_str(&text).map_err(|e| {
            ApiError::Decode(format!(
                "{e} in: {}",
                text.chars().take(200).collect::<String>()
            ))
        })
    }

    async fn request_unit(
        &mut self,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(), ApiError> {
        self.execute(method, url, body).await.map(|_| ())
    }

    async fn paginate<T: DeserializeOwned>(
        &mut self,
        first_url: String,
        progress: Progress<'_>,
    ) -> Result<Vec<T>, ApiError> {
        let mut out: Vec<T> = Vec::new();
        let mut url = first_url;
        loop {
            let page: Page<T> = self.request_json(Method::GET, &url, None).await?;
            out.extend(page.items);
            progress(out.len() as u64, page.total);
            match page.next {
                // `next` links from the API are absolute URLs.
                Some(next) => {
                    tokio::time::sleep(PAGE_DELAY).await;
                    url = next;
                }
                None => return Ok(out),
            }
        }
    }

    // ------------------------------------------------------------------
    // Reads (pub)
    // ------------------------------------------------------------------

    pub async fn me(&mut self) -> Result<UserProfile, ApiError> {
        self.request_json(Method::GET, &format!("{API}/me"), None)
            .await
    }

    /// All of the current user's playlists (owned and followed). Note that
    /// only owned/collaborative playlists have readable CONTENTS.
    pub async fn my_playlists(
        &mut self,
        progress: Progress<'_>,
    ) -> Result<Vec<SimplifiedPlaylist>, ApiError> {
        // Items can be null (ghost playlists) — filter them out.
        let items: Vec<Option<SimplifiedPlaylist>> = self
            .paginate(format!("{API}/me/playlists?limit=50"), progress)
            .await?;
        Ok(items.into_iter().flatten().collect())
    }

    /// Full contents of an owned/collaborative playlist, trimmed via
    /// `fields`. Page limit is 50 on the `/items` endpoint.
    pub async fn playlist_items(
        &mut self,
        playlist_id: &str,
        progress: Progress<'_>,
    ) -> Result<Vec<PlaylistItem>, ApiError> {
        const FIELDS: &str = "items(added_at,track(id,uri,name,duration_ms,popularity,is_local,\
                              type,artists(id,name),album(name,release_date))),next,total";
        let url = url::Url::parse_with_params(
            &format!("{API}/playlists/{playlist_id}/items"),
            &[("limit", "50"), ("fields", FIELDS)],
        )
        .expect("valid url")
        .to_string();
        self.paginate(url, progress).await
    }

    /// First page only (≤50 items) of an owned/collaborative playlist —
    /// cheap taste sampling for the library digest.
    pub async fn playlist_items_first_page(
        &mut self,
        playlist_id: &str,
    ) -> Result<Vec<PlaylistItem>, ApiError> {
        const FIELDS: &str = "items(added_at,track(id,uri,name,duration_ms,popularity,is_local,\
                              type,artists(id,name),album(name,release_date))),next,total";
        let url = url::Url::parse_with_params(
            &format!("{API}/playlists/{playlist_id}/items"),
            &[("limit", "50"), ("fields", FIELDS)],
        )
        .expect("valid url")
        .to_string();
        let page: Page<PlaylistItem> = self.request_json(Method::GET, &url, None).await?;
        Ok(page.items)
    }

    /// The user's Liked Songs ("saved tracks"). Page limit is 50.
    pub async fn saved_tracks(
        &mut self,
        progress: Progress<'_>,
    ) -> Result<Vec<SavedTrackItem>, ApiError> {
        self.paginate(format!("{API}/me/tracks?limit=50"), progress)
            .await
    }

    /// `time_range`: "short_term" (~4 weeks) | "medium_term" (~6 months) |
    /// "long_term" (~1 year+).
    pub async fn top_artists(&mut self, time_range: &str) -> Result<Vec<FullArtist>, ApiError> {
        let url = format!("{API}/me/top/artists?time_range={time_range}&limit=50");
        Ok(self
            .request_json::<Page<FullArtist>>(Method::GET, &url, None)
            .await?
            .items)
    }

    pub async fn top_tracks(&mut self, time_range: &str) -> Result<Vec<PlayableItem>, ApiError> {
        let url = format!("{API}/me/top/tracks?time_range={time_range}&limit=50");
        Ok(self
            .request_json::<Page<PlayableItem>>(Method::GET, &url, None)
            .await?
            .items)
    }

    /// The most recently played tracks (the API exposes no deeper history
    /// than the recent window; limit max 50).
    pub async fn recently_played(&mut self) -> Result<Vec<PlayHistoryItem>, ApiError> {
        let url = format!("{API}/me/player/recently-played?limit=50");
        Ok(self
            .request_json::<Page<PlayHistoryItem>>(Method::GET, &url, None)
            .await?
            .items)
    }

    /// Track search. Since Feb 2026 the page limit is 10 (default 5).
    pub async fn search_tracks(
        &mut self,
        query: &str,
        limit: u8,
    ) -> Result<Vec<PlayableItem>, ApiError> {
        let limit = limit.clamp(1, 10);
        let url = url::Url::parse_with_params(
            &format!("{API}/search"),
            &[
                ("q", query),
                ("type", "track"),
                ("limit", limit.to_string().as_str()),
            ],
        )
        .expect("valid url")
        .to_string();
        let resp: SearchResponse = self.request_json(Method::GET, &url, None).await?;
        Ok(resp.tracks.map(|p| p.items).unwrap_or_default())
    }

    // ------------------------------------------------------------------
    // Mutations (pub(super) — reachable only through spotify::service,
    // which enforces the safety policy)
    // ------------------------------------------------------------------

    pub(super) async fn create_playlist(
        &mut self,
        name: &str,
        description: &str,
        public: bool,
    ) -> Result<SimplifiedPlaylist, ApiError> {
        // Spotify clients cap descriptions around 300 chars.
        let description: String = description.chars().take(300).collect();
        let body = json!({ "name": name, "description": description, "public": public });
        self.request_json(Method::POST, &format!("{API}/me/playlists"), Some(&body))
            .await
    }

    pub(super) async fn change_playlist_details(
        &mut self,
        playlist_id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut body = serde_json::Map::new();
        if let Some(n) = name {
            body.insert("name".into(), json!(n));
        }
        if let Some(d) = description {
            let d: String = d.chars().take(300).collect();
            body.insert("description".into(), json!(d));
        }
        if body.is_empty() {
            return Ok(());
        }
        self.request_unit(
            Method::PUT,
            &format!("{API}/playlists/{playlist_id}"),
            Some(&serde_json::Value::Object(body)),
        )
        .await
    }

    pub(super) async fn add_items(
        &mut self,
        playlist_id: &str,
        uris: &[String],
        progress: Progress<'_>,
    ) -> Result<(), ApiError> {
        let total = uris.len() as u64;
        let mut done = 0u64;
        for chunk in uris.chunks(TRACK_WRITE_CHUNK) {
            let body = json!({ "uris": chunk });
            self.request_unit(
                Method::POST,
                &format!("{API}/playlists/{playlist_id}/items"),
                Some(&body),
            )
            .await?;
            done += chunk.len() as u64;
            progress(done, Some(total));
            tokio::time::sleep(PAGE_DELAY).await;
        }
        Ok(())
    }

    /// Replace the playlist's entire contents with `uris` (empty clears it).
    pub(super) async fn replace_items(
        &mut self,
        playlist_id: &str,
        uris: &[String],
        progress: Progress<'_>,
    ) -> Result<(), ApiError> {
        let first: Vec<String> = uris.iter().take(TRACK_WRITE_CHUNK).cloned().collect();
        let body = json!({ "uris": first });
        self.request_unit(
            Method::PUT,
            &format!("{API}/playlists/{playlist_id}/items"),
            Some(&body),
        )
        .await?;
        progress(first.len() as u64, Some(uris.len() as u64));
        if uris.len() > TRACK_WRITE_CHUNK {
            let total = uris.len() as u64;
            self.add_items(playlist_id, &uris[TRACK_WRITE_CHUNK..], &mut |done, _| {
                progress(TRACK_WRITE_CHUNK as u64 + done, Some(total));
            })
            .await?;
        }
        Ok(())
    }

    /// Delete (= remove from library / unfollow) a playlist the user owns or
    /// follows. Tries the current consolidated endpoint first, then the
    /// deprecated one. Both routes act ONLY on the given playlist id.
    /// For owned playlists Spotify keeps a ~90-day server-side recovery
    /// window (spotify.com/account → Recover playlists).
    pub(super) async fn delete_playlist(
        &mut self,
        playlist_id: &str,
    ) -> Result<DeleteRoute, ApiError> {
        let uri = format!("spotify:playlist:{playlist_id}");
        let url =
            url::Url::parse_with_params(&format!("{API}/me/library"), &[("uris", uri.as_str())])
                .expect("valid url")
                .to_string();
        // rspotify 0.16 sends an empty JSON object body with the query form.
        match self
            .request_unit(Method::DELETE, &url, Some(&json!({})))
            .await
        {
            Ok(()) => Ok(DeleteRoute::Library),
            Err(ApiError::Status { status, .. })
                if (400..500).contains(&status) && status != 401 =>
            {
                tracing::warn!(
                    "DELETE /me/library returned {status}; falling back to the deprecated \
                     unfollow endpoint"
                );
                self.request_unit(
                    Method::DELETE,
                    &format!("{API}/playlists/{playlist_id}/followers"),
                    None,
                )
                .await?;
                Ok(DeleteRoute::LegacyUnfollow)
            }
            Err(e) => Err(e),
        }
    }
}
