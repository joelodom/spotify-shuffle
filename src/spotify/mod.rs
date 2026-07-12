//! Spotify Web API integration.
//!
//! Layering (and how the safety model is enforced structurally):
//!
//! * [`client`] — the raw typed HTTP client. Read endpoints are `pub`;
//!   every MUTATING endpoint is `pub(super)`, i.e. callable only from inside
//!   this module tree.
//! * [`service`] — the only public gateway to mutations. Each mutating
//!   wrapper first obtains an [`crate::safety::EditGrant`] /
//!   [`crate::safety::DeleteGrant`] from the session's
//!   [`crate::safety::SafetyPolicy`] and passes the *grant's* playlist id to
//!   the client. Feature code and the UI therefore have no compile-time path
//!   to a mutating HTTP call that skips the policy.
//! * [`auth`] — Authorization Code + PKCE against the loopback address, with
//!   on-disk token persistence and refresh-token rotation.

pub mod auth;
pub mod client;
pub mod models;
pub mod service;
