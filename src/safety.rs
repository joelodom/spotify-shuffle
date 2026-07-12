//! The two-tier playlist safety model.
//!
//! Every playlist visible to this app falls into exactly one of two tiers:
//!
//! * **Session** — created *by this app during the current process lifetime*.
//!   Full freedom: contents may be edited (add / remove / reorder / rename)
//!   and the playlist may be deleted without confirmation.
//! * **Protected** — everything else: the user's longstanding playlists AND
//!   playlists this app created in any *previous* run. Contents are
//!   read-only. Any transformation of a protected playlist must be written to
//!   a NEW playlist. The single permitted destructive action is deletion, and
//!   only through the guarded confirmation flow (the user must type the exact
//!   word `delete`).
//!
//! The tier registry is deliberately **in-memory only**. Persisting it would
//! let a stale file re-arm write access to playlists from an earlier session,
//! which the model forbids: the moment a session ends, its playlists become
//! protected forever. An unknown or ambiguous playlist is always protected —
//! [`SafetyPolicy::tier`] defaults to [`Tier::Protected`] for any id it has
//! not itself recorded a creation for.
//!
//! Enforcement is structural, not conventional: the HTTP methods that mutate
//! or delete playlists (in `spotify::client`) are private to the `spotify`
//! module, and the public service wrappers each demand a proof token —
//! [`EditGrant`] or [`DeleteGrant`] — whose constructors are private to this
//! module. The only mints are the `authorize_*` / `confirm_*` methods below,
//! so a compile-time path from "feature code" to "mutating HTTP call" that
//! bypasses the policy does not exist. Grants carry the playlist id they were
//! minted for, and the HTTP layer uses *the grant's* id, so a grant for one
//! playlist cannot be replayed against another.

use std::collections::HashMap;
use std::fmt;

/// A Spotify playlist id (the base-62 id, not the full URI).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PlaylistId(pub String);

impl PlaylistId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlaylistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The exact confirmation text required by the guarded deletion flow.
/// Comparison is byte-exact: case-sensitive, no trimming.
pub const DELETE_CONFIRMATION_WORD: &str = "delete";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Created by this app during the current process lifetime.
    Session,
    /// Everything else. Contents read-only; deletion only via guarded flow.
    Protected,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Session => "session",
            Tier::Protected => "protected",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum SafetyError {
    #[error(
        "'{name}' is a protected playlist; its contents are read-only. \
         Write the result to a new playlist instead."
    )]
    ProtectedContentEdit { id: PlaylistId, name: String },

    #[error(
        "'{name}' is a protected playlist; deleting it requires the guarded \
         confirmation flow (type '{DELETE_CONFIRMATION_WORD}')."
    )]
    GuardedConfirmationRequired { id: PlaylistId, name: String },

    #[error("no guarded deletion is pending; nothing was deleted")]
    NoPendingDeletion,

    #[error(
        "the pending guarded deletion is for '{pending_name}', not the \
         requested playlist; the deletion was cancelled"
    )]
    PendingDeletionMismatch { pending_name: String },

    #[error(
        "confirmation text did not exactly match '{DELETE_CONFIRMATION_WORD}'; \
         the deletion was cancelled"
    )]
    ConfirmationMismatch,
}

/// Proof that editing the contents/details of the playlist in `id` is
/// permitted (i.e. it is a session playlist). Only [`SafetyPolicy`] can mint
/// one.
#[derive(Debug, PartialEq, Eq)]
pub struct EditGrant {
    id: PlaylistId,
}

impl EditGrant {
    pub fn id(&self) -> &PlaylistId {
        &self.id
    }
}

/// Proof that deleting the playlist in `id` is permitted — either because it
/// is a session playlist, or because the guarded confirmation flow completed
/// with the exact confirmation word. Only [`SafetyPolicy`] can mint one.
#[derive(Debug, PartialEq, Eq)]
pub struct DeleteGrant {
    id: PlaylistId,
    name: String,
}

impl DeleteGrant {
    pub fn id(&self) -> &PlaylistId {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A guarded deletion that has been armed but not yet confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDeletion {
    pub id: PlaylistId,
    pub name: String,
}

/// The authority on playlist tiers and mutation rights for one app session.
///
/// Single-owner by design: it lives inside the worker that serializes all
/// Spotify operations, so tier checks and the HTTP calls they authorize
/// cannot race.
#[derive(Default)]
pub struct SafetyPolicy {
    /// Playlists created by this process: id → last known name.
    session_created: HashMap<PlaylistId, String>,
    /// At most one guarded deletion may be armed at a time.
    pending_delete: Option<PendingDeletion>,
}

impl SafetyPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that this app just created `id`. This is the ONLY way a
    /// playlist becomes a session playlist. Call it exclusively from the
    /// create-playlist path, with the id returned by the Spotify API.
    pub fn note_created(&mut self, id: PlaylistId, name: impl Into<String>) {
        self.session_created.insert(id, name.into());
    }

    /// Record that `id` no longer exists (was deleted through this app).
    pub fn note_deleted(&mut self, id: &PlaylistId) {
        self.session_created.remove(id);
        if self.pending_delete.as_ref().is_some_and(|p| &p.id == id) {
            self.pending_delete = None;
        }
    }

    /// Keep the display name fresh after a rename (session playlists only;
    /// renames of protected playlists are impossible through this app).
    pub fn note_renamed(&mut self, id: &PlaylistId, new_name: impl Into<String>) {
        if let Some(name) = self.session_created.get_mut(id) {
            *name = new_name.into();
        }
    }

    /// Tier of `id`. Anything this policy did not record a creation for —
    /// including every playlist from previous sessions and any id we are
    /// unsure about — is `Protected`.
    pub fn tier(&self, id: &PlaylistId) -> Tier {
        if self.session_created.contains_key(id) {
            Tier::Session
        } else {
            Tier::Protected
        }
    }

    /// (Part of the policy's public surface; exercised by the test suite.)
    #[allow(dead_code)]
    pub fn is_session(&self, id: &PlaylistId) -> bool {
        self.tier(id) == Tier::Session
    }

    /// Snapshot of the playlists created this session (id, name).
    pub fn session_playlists(&self) -> Vec<(PlaylistId, String)> {
        let mut v: Vec<_> = self
            .session_created
            .iter()
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    /// Mint an [`EditGrant`] for a content/details mutation of `id`.
    /// Fails for anything that is not a session playlist.
    pub fn authorize_content_edit(
        &self,
        id: &PlaylistId,
        display_name: &str,
    ) -> Result<EditGrant, SafetyError> {
        match self.tier(id) {
            Tier::Session => Ok(EditGrant { id: id.clone() }),
            Tier::Protected => Err(SafetyError::ProtectedContentEdit {
                id: id.clone(),
                name: display_name.to_string(),
            }),
        }
    }

    /// Mint a [`DeleteGrant`] for a *session* playlist (no confirmation
    /// needed). For protected playlists this fails and the caller must run
    /// the guarded flow instead.
    pub fn authorize_session_delete(
        &self,
        id: &PlaylistId,
        display_name: &str,
    ) -> Result<DeleteGrant, SafetyError> {
        match self.tier(id) {
            Tier::Session => Ok(DeleteGrant {
                id: id.clone(),
                name: self
                    .session_created
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| display_name.to_string()),
            }),
            Tier::Protected => Err(SafetyError::GuardedConfirmationRequired {
                id: id.clone(),
                name: display_name.to_string(),
            }),
        }
    }

    /// Arm the guarded deletion flow for `id`. Replaces any previously armed
    /// deletion (only one can be pending). Returns the pending record whose
    /// `name` MUST be displayed prominently to the user before confirmation.
    pub fn begin_guarded_delete(
        &mut self,
        id: PlaylistId,
        name: impl Into<String>,
    ) -> PendingDeletion {
        let pending = PendingDeletion {
            id,
            name: name.into(),
        };
        self.pending_delete = Some(pending.clone());
        pending
    }

    /// (Part of the policy's public surface; exercised by the test suite.)
    #[allow(dead_code)]
    pub fn pending_delete(&self) -> Option<&PendingDeletion> {
        self.pending_delete.as_ref()
    }

    /// Cancel any armed guarded deletion. Returns true if one was armed.
    pub fn cancel_pending_delete(&mut self) -> bool {
        self.pending_delete.take().is_some()
    }

    /// Confirm the armed guarded deletion.
    ///
    /// Succeeds only when ALL hold:
    /// 1. a deletion is armed,
    /// 2. it is armed for exactly `id` (the playlist shown in the prompt),
    /// 3. `typed` is byte-for-byte [`DELETE_CONFIRMATION_WORD`].
    ///
    /// Any failure — including an empty or mismatched confirmation — consumes
    /// the pending record: the flow is cancelled and must be restarted from
    /// scratch. The returned grant is bound to the armed id, so the deletion
    /// can never touch any playlist other than the one named in the prompt.
    pub fn confirm_guarded_delete(
        &mut self,
        id: &PlaylistId,
        typed: &str,
    ) -> Result<DeleteGrant, SafetyError> {
        // Taken unconditionally: every outcome disarms the flow.
        let pending = self
            .pending_delete
            .take()
            .ok_or(SafetyError::NoPendingDeletion)?;
        if &pending.id != id {
            return Err(SafetyError::PendingDeletionMismatch {
                pending_name: pending.name,
            });
        }
        if typed != DELETE_CONFIRMATION_WORD {
            return Err(SafetyError::ConfirmationMismatch);
        }
        Ok(DeleteGrant {
            id: pending.id,
            name: pending.name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PlaylistId {
        PlaylistId(s.to_string())
    }

    #[test]
    fn unknown_playlists_are_protected_by_default() {
        let policy = SafetyPolicy::new();
        assert_eq!(policy.tier(&pid("abc")), Tier::Protected);
        assert!(!policy.is_session(&pid("abc")));
    }

    #[test]
    fn created_playlists_are_session_tier() {
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("new1"), "My Session Mix");
        assert_eq!(policy.tier(&pid("new1")), Tier::Session);
        assert_eq!(
            policy.session_playlists(),
            vec![(pid("new1"), "My Session Mix".to_string())]
        );
    }

    #[test]
    fn a_fresh_session_treats_previous_sessions_playlists_as_protected() {
        // Session 1 creates a playlist.
        let mut session1 = SafetyPolicy::new();
        session1.note_created(pid("older"), "From Last Time");
        assert_eq!(session1.tier(&pid("older")), Tier::Session);

        // Session 2 is a brand-new policy (the registry is never persisted):
        // the same id is now protected.
        let session2 = SafetyPolicy::new();
        assert_eq!(session2.tier(&pid("older")), Tier::Protected);
        assert!(
            session2
                .authorize_content_edit(&pid("older"), "From Last Time")
                .is_err()
        );
    }

    #[test]
    fn content_edit_grant_only_for_session_playlists() {
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("s1"), "Session One");

        let grant = policy
            .authorize_content_edit(&pid("s1"), "Session One")
            .unwrap();
        assert_eq!(grant.id(), &pid("s1"));

        let err = policy
            .authorize_content_edit(&pid("longstanding"), "Road Trip 2019")
            .unwrap_err();
        assert_eq!(
            err,
            SafetyError::ProtectedContentEdit {
                id: pid("longstanding"),
                name: "Road Trip 2019".to_string()
            }
        );
    }

    #[test]
    fn session_delete_is_free_but_protected_delete_requires_guarded_flow() {
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("s1"), "Session One");

        let grant = policy
            .authorize_session_delete(&pid("s1"), "Session One")
            .unwrap();
        assert_eq!(grant.id(), &pid("s1"));

        let err = policy
            .authorize_session_delete(&pid("keeper"), "Keeper")
            .unwrap_err();
        assert!(matches!(
            err,
            SafetyError::GuardedConfirmationRequired { .. }
        ));
    }

    #[test]
    fn guarded_flow_happy_path_requires_exact_word() {
        let mut policy = SafetyPolicy::new();
        policy.begin_guarded_delete(pid("victim"), "Old Playlist");
        assert!(policy.pending_delete().is_some());

        let grant = policy
            .confirm_guarded_delete(&pid("victim"), "delete")
            .unwrap();
        assert_eq!(grant.id(), &pid("victim"));
        assert_eq!(grant.name(), "Old Playlist");
        // Consumed: a second confirmation has nothing to act on.
        assert_eq!(
            policy.confirm_guarded_delete(&pid("victim"), "delete"),
            Err(SafetyError::NoPendingDeletion)
        );
    }

    #[test]
    fn guarded_flow_rejects_and_cancels_on_any_mismatched_text() {
        for wrong in [
            "", "Delete", "DELETE", " delete", "delete ", "del", "yes", "delete!",
        ] {
            let mut policy = SafetyPolicy::new();
            policy.begin_guarded_delete(pid("victim"), "Old Playlist");
            assert_eq!(
                policy.confirm_guarded_delete(&pid("victim"), wrong),
                Err(SafetyError::ConfirmationMismatch),
                "input {wrong:?} must be rejected"
            );
            // The mismatch cancelled the flow entirely.
            assert!(policy.pending_delete().is_none());
            assert_eq!(
                policy.confirm_guarded_delete(&pid("victim"), "delete"),
                Err(SafetyError::NoPendingDeletion),
                "flow must require re-arming after a mismatch"
            );
        }
    }

    #[test]
    fn guarded_flow_is_bound_to_the_armed_playlist_only() {
        let mut policy = SafetyPolicy::new();
        policy.begin_guarded_delete(pid("armed"), "Armed Playlist");
        // Even a perfect confirmation word cannot delete a different id.
        let err = policy
            .confirm_guarded_delete(&pid("other"), "delete")
            .unwrap_err();
        assert_eq!(
            err,
            SafetyError::PendingDeletionMismatch {
                pending_name: "Armed Playlist".to_string()
            }
        );
        // And the attempt disarmed the flow.
        assert!(policy.pending_delete().is_none());
    }

    #[test]
    fn arming_twice_keeps_only_the_latest_target() {
        let mut policy = SafetyPolicy::new();
        policy.begin_guarded_delete(pid("first"), "First");
        policy.begin_guarded_delete(pid("second"), "Second");
        assert_eq!(
            policy.confirm_guarded_delete(&pid("first"), "delete"),
            Err(SafetyError::PendingDeletionMismatch {
                pending_name: "Second".to_string()
            })
        );
    }

    #[test]
    fn cancel_disarms_the_flow() {
        let mut policy = SafetyPolicy::new();
        policy.begin_guarded_delete(pid("victim"), "Old Playlist");
        assert!(policy.cancel_pending_delete());
        assert!(!policy.cancel_pending_delete());
        assert_eq!(
            policy.confirm_guarded_delete(&pid("victim"), "delete"),
            Err(SafetyError::NoPendingDeletion)
        );
    }

    #[test]
    fn deleting_a_session_playlist_removes_it_from_the_registry() {
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("s1"), "Session One");
        policy.note_deleted(&pid("s1"));
        assert_eq!(policy.tier(&pid("s1")), Tier::Protected);
        assert!(policy.session_playlists().is_empty());
    }

    #[test]
    fn rename_updates_registry_name() {
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("s1"), "Draft");
        policy.note_renamed(&pid("s1"), "Final Name");
        assert_eq!(
            policy.session_playlists(),
            vec![(pid("s1"), "Final Name".to_string())]
        );
        // Renaming an unknown id is a no-op, not an implicit registration.
        policy.note_renamed(&pid("ghost"), "Spooky");
        assert_eq!(policy.tier(&pid("ghost")), Tier::Protected);
    }

    #[test]
    fn guarded_flow_also_works_for_session_playlists_stricter_is_fine() {
        // The UI never routes session playlists through the guarded flow, but
        // if it ever did, the stricter path must still be safe and correct.
        let mut policy = SafetyPolicy::new();
        policy.note_created(pid("s1"), "Session One");
        policy.begin_guarded_delete(pid("s1"), "Session One");
        let grant = policy.confirm_guarded_delete(&pid("s1"), "delete").unwrap();
        assert_eq!(grant.id(), &pid("s1"));
    }
}
