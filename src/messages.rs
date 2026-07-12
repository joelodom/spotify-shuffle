//! Message types between the UI thread and the background worker.
//!
//! The UI never touches the network: it sends [`Command`]s and renders
//! [`Event`]s. The worker processes commands strictly one at a time, which
//! also serializes every safety-policy check with the mutation it guards.

use std::path::PathBuf;

use crate::config::AppConfig;
use crate::ops::TrackSource;
use crate::ops::import_export::ExportFormat;
use crate::ops::insights::InsightsData;
use crate::ops::playlist_tools::SortKey;
use crate::safety::{PlaylistId, Tier};

#[derive(Clone, Debug)]
pub enum Command {
    Connect,
    Disconnect,
    RefreshPlaylists,
    LoadTracks(TrackSource),

    Generate {
        description: String,
        count: usize,
        personalize: bool,
    },
    Refine {
        source: TrackSource,
        instruction: String,
        in_place: bool,
    },
    Organize {
        goal: String,
        max_playlists: usize,
    },

    Shuffle {
        source: TrackSource,
    },
    Dedupe {
        source: TrackSource,
        in_place: bool,
    },
    Merge {
        sources: Vec<TrackSource>,
        dedupe: bool,
        shuffle: bool,
    },
    Sort {
        source: TrackSource,
        key: SortKey,
        descending: bool,
        in_place: bool,
    },
    Import {
        name: String,
        text: String,
    },
    Export {
        source: TrackSource,
        format: ExportFormat,
        path: PathBuf,
    },

    RenameSession {
        id: PlaylistId,
        current_name: String,
        new_name: String,
    },
    DeleteSession {
        id: PlaylistId,
        name: String,
    },
    ArmGuardedDelete {
        id: PlaylistId,
        name: String,
    },
    ConfirmGuardedDelete {
        id: PlaylistId,
        typed: String,
    },
    CancelGuardedDelete,

    FetchInsights,
    /// Free connectivity check (binary/auth/key presence) — no generation.
    CheckAi,
    /// Full round-trip test — performs one tiny generation.
    TestAi,
    ApplyConfig(Box<AppConfig>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct PlaylistRow {
    pub id: PlaylistId,
    pub name: String,
    pub owner: String,
    pub total: u64,
    pub tier: Tier,
    /// Contents readable? (owned or collaborative — dev-mode restriction)
    pub readable: bool,
}

#[derive(Clone, Debug)]
pub struct TrackRow {
    pub title: String,
    pub artists: String,
    pub album: String,
    pub duration: String,
    pub release_date: String,
    pub added_at: String,
    pub is_local: bool,
    pub is_episode: bool,
}

#[derive(Clone, Debug)]
pub struct AuthInfo {
    pub connected: bool,
    pub user: Option<String>,
    pub provider_desc: String,
}

#[derive(Clone, Debug)]
pub enum Event {
    Log(LogLevel, String, String), // level, time, message
    Auth(AuthInfo),
    Playlists(Vec<PlaylistRow>),
    SessionPlaylists(Vec<(PlaylistId, String)>),
    Tracks {
        source_label: String,
        rows: Vec<TrackRow>,
    },
    BusyStarted {
        label: String,
    },
    BusyProgress {
        label: String,
        done: u64,
        total: Option<u64>,
    },
    BusyFinished,
    Insights(Box<InsightsData>),
    /// The worker armed the guarded deletion flow; the UI must now show the
    /// warning dialog displaying exactly this name.
    GuardedDeleteArmed {
        id: PlaylistId,
        name: String,
    },
    /// The guarded flow ended (confirmed, mismatched, or cancelled).
    GuardedDeleteResolved,
    AiTest {
        ok: bool,
        message: String,
    },
}
