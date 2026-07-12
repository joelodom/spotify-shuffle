//! The safety-gated gateway to Spotify.
//!
//! `SpotifyService` is the ONLY public path to playlist mutation. Every
//! mutating method first asks the session's [`SafetyPolicy`] for a grant;
//! the raw client methods then operate on the id carried inside the grant.
//! Reads pass through ungated (reading protected playlists is expected and
//! encouraged — that is how the app learns taste).

use std::path::PathBuf;

use crate::safety::{PendingDeletion, PlaylistId, SafetyError, SafetyPolicy, Tier};

use super::auth::{self, AuthError, TokenManager};
use super::client::{Progress, SpotifyClient};
use super::models::{SimplifiedPlaylist, UserProfile};

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// A safety-model refusal. Never the result of a network problem: the
    /// operation was blocked before any request was sent.
    #[error("{0}")]
    Safety(#[from] SafetyError),
    #[error("{0}")]
    Api(#[from] super::client::ApiError),
}

impl From<AuthError> for ServiceError {
    fn from(e: AuthError) -> Self {
        ServiceError::Api(e.into())
    }
}

pub struct SpotifyService {
    client: SpotifyClient,
    policy: SafetyPolicy,
    me: Option<UserProfile>,
}

impl SpotifyService {
    pub fn new(client_id: &str, tokens_path: PathBuf) -> Self {
        let tokens = TokenManager::load(tokens_path, client_id);
        Self {
            client: SpotifyClient::new(tokens),
            policy: SafetyPolicy::new(),
            me: None,
        }
    }

    /// Swap credentials (Settings changed). The safety registry survives: a
    /// session playlist was still created by this session.
    pub fn reconfigure(&mut self, client_id: &str, tokens_path: PathBuf) {
        let tokens = TokenManager::load(tokens_path, client_id);
        self.client = SpotifyClient::new(tokens);
        self.me = None;
    }

    pub fn is_authenticated(&self) -> bool {
        self.client.is_authenticated()
    }

    /// Run the interactive browser PKCE flow and persist the tokens.
    pub async fn connect_interactive(
        &mut self,
        client_id: &str,
        port: u16,
        notify: impl Fn(String),
    ) -> Result<UserProfile, ServiceError> {
        let http = self.client.http().clone();
        let tokens = auth::run_pkce_flow(&http, client_id, port, notify).await?;
        self.client
            .tokens_mut()
            .set(tokens)
            .map_err(AuthError::Io)?;
        self.me = None;
        self.ensure_me().await
    }

    pub fn disconnect(&mut self) {
        self.client.tokens_mut().clear();
        self.me = None;
    }

    pub async fn ensure_me(&mut self) -> Result<UserProfile, ServiceError> {
        if let Some(me) = &self.me {
            return Ok(me.clone());
        }
        let me = self.client.me().await?;
        self.me = Some(me.clone());
        Ok(me)
    }

    /// Ungated read access. Mutating client methods are `pub(super)`, so
    /// handing out `&mut SpotifyClient` exposes reads only.
    pub fn reads(&mut self) -> &mut SpotifyClient {
        &mut self.client
    }

    // ------------------------------------------------------------------
    // Safety introspection
    // ------------------------------------------------------------------

    pub fn tier(&self, id: &PlaylistId) -> Tier {
        self.policy.tier(id)
    }

    pub fn session_playlists(&self) -> Vec<(PlaylistId, String)> {
        self.policy.session_playlists()
    }

    // ------------------------------------------------------------------
    // Creation — the only way a playlist enters the session tier
    // ------------------------------------------------------------------

    pub async fn create_playlist(
        &mut self,
        name: &str,
        description: &str,
        public: bool,
    ) -> Result<SimplifiedPlaylist, ServiceError> {
        let playlist = self
            .client
            .create_playlist(name, description, public)
            .await?;
        self.policy
            .note_created(PlaylistId(playlist.id.clone()), playlist.name.clone());
        Ok(playlist)
    }

    // ------------------------------------------------------------------
    // Gated content mutations (session playlists only)
    // ------------------------------------------------------------------

    pub async fn add_items(
        &mut self,
        id: &PlaylistId,
        display_name: &str,
        uris: &[String],
        progress: Progress<'_>,
    ) -> Result<(), ServiceError> {
        let grant = self.policy.authorize_content_edit(id, display_name)?;
        self.client
            .add_items(grant.id().as_str(), uris, progress)
            .await?;
        Ok(())
    }

    pub async fn replace_items(
        &mut self,
        id: &PlaylistId,
        display_name: &str,
        uris: &[String],
        progress: Progress<'_>,
    ) -> Result<(), ServiceError> {
        let grant = self.policy.authorize_content_edit(id, display_name)?;
        self.client
            .replace_items(grant.id().as_str(), uris, progress)
            .await?;
        Ok(())
    }

    pub async fn rename_playlist(
        &mut self,
        id: &PlaylistId,
        display_name: &str,
        new_name: Option<&str>,
        new_description: Option<&str>,
    ) -> Result<(), ServiceError> {
        let grant = self.policy.authorize_content_edit(id, display_name)?;
        self.client
            .change_playlist_details(grant.id().as_str(), new_name, new_description)
            .await?;
        if let Some(n) = new_name {
            self.policy.note_renamed(id, n);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Deletion
    // ------------------------------------------------------------------

    /// Free deletion for SESSION playlists only. For protected playlists
    /// this fails with `GuardedConfirmationRequired` without any request
    /// being sent.
    pub async fn delete_session_playlist(
        &mut self,
        id: &PlaylistId,
        display_name: &str,
    ) -> Result<String, ServiceError> {
        let grant = self.policy.authorize_session_delete(id, display_name)?;
        let route = self.client.delete_playlist(grant.id().as_str()).await?;
        tracing::info!("deleted session playlist via {route:?}");
        let name = grant.name().to_string();
        self.policy.note_deleted(id);
        Ok(name)
    }

    /// Arm the guarded deletion flow for a protected playlist. The returned
    /// record's `name` must be displayed prominently before confirmation.
    pub fn begin_guarded_delete(&mut self, id: PlaylistId, name: &str) -> PendingDeletion {
        self.policy.begin_guarded_delete(id, name)
    }

    pub fn cancel_guarded_delete(&mut self) -> bool {
        self.policy.cancel_pending_delete()
    }

    /// Execute the guarded deletion iff `typed` is exactly "delete" and the
    /// armed target matches `id`. Any mismatch cancels the flow.
    pub async fn confirm_guarded_delete(
        &mut self,
        id: &PlaylistId,
        typed: &str,
    ) -> Result<String, ServiceError> {
        let grant = self.policy.confirm_guarded_delete(id, typed)?;
        let route = self.client.delete_playlist(grant.id().as_str()).await?;
        tracing::info!("guarded deletion executed via {route:?}");
        let name = grant.name().to_string();
        self.policy.note_deleted(id);
        Ok(name)
    }
}
