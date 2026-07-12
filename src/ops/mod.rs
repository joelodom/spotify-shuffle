//! Feature operations ("ops") — the pipelines behind every button in the UI.
//!
//! Ops read anything but can only WRITE through `SpotifyService`, which
//! enforces the two-tier safety model. By construction, every op that
//! transforms a protected source writes its output to a NEW playlist; the
//! `in_place` variants are only honored for session playlists (and the
//! service re-checks that independently of the UI).

pub mod generate;
pub mod import_export;
pub mod insights;
pub mod organize;
pub mod playlist_tools;
pub mod refine;
pub mod resolve;

use crate::ai::AiError;
use crate::safety::PlaylistId;
use crate::spotify::models::TrackInfo;
use crate::spotify::service::{ServiceError, SpotifyService};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Spotify(#[from] ServiceError),
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("{0}")]
    Other(String),
}

/// Streamed feedback from a running op to the UI.
#[derive(Clone, Debug)]
pub enum OpUpdate {
    Log(String),
    Progress {
        label: String,
        done: u64,
        total: Option<u64>,
    },
}

pub type Sink<'a> = &'a mut dyn FnMut(OpUpdate);

/// Where an op reads tracks from. Liked Songs is not a playlist and can
/// therefore never be a mutation target — only ever a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackSource {
    LikedSongs,
    Playlist { id: PlaylistId, name: String },
}

impl TrackSource {
    pub fn label(&self) -> String {
        match self {
            TrackSource::LikedSongs => "Liked Songs".to_string(),
            TrackSource::Playlist { name, .. } => name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreatedPlaylist {
    pub name: String,
    pub track_count: usize,
}

/// Result of writing an op's output.
#[derive(Clone, Debug)]
pub enum WriteDestination {
    NewPlaylist(CreatedPlaylist),
    EditedInPlace { name: String, track_count: usize },
}

impl WriteDestination {
    pub fn summary(&self) -> String {
        match self {
            WriteDestination::NewPlaylist(p) => {
                format!(
                    "created new playlist '{}' with {} tracks",
                    p.name, p.track_count
                )
            }
            WriteDestination::EditedInPlace {
                name, track_count, ..
            } => {
                format!("updated session playlist '{name}' in place ({track_count} tracks)")
            }
        }
    }
    pub fn created(&self) -> Option<&CreatedPlaylist> {
        match self {
            WriteDestination::NewPlaylist(p) => Some(p),
            WriteDestination::EditedInPlace { .. } => None,
        }
    }
}

/// Fetch every track of a source as display-ready `TrackInfo`.
pub async fn fetch_source_tracks(
    svc: &mut SpotifyService,
    source: &TrackSource,
    sink: Sink<'_>,
) -> Result<Vec<TrackInfo>, OpError> {
    let label = source.label();
    sink(OpUpdate::Log(format!("Fetching tracks from '{label}'…")));
    let mut progress = |done: u64, total: Option<u64>| {
        sink(OpUpdate::Progress {
            label: format!("Reading '{label}'"),
            done,
            total,
        });
    };
    let tracks = match source {
        TrackSource::LikedSongs => svc
            .reads()
            .saved_tracks(&mut progress)
            .await
            .map_err(ServiceError::from)?
            .into_iter()
            .filter_map(|item| {
                let added = item.added_at.clone();
                item.track.and_then(|t| TrackInfo::from_playable(t, added))
            })
            .collect::<Vec<_>>(),
        TrackSource::Playlist { id, .. } => svc
            .reads()
            .playlist_items(id.as_str(), &mut progress)
            .await
            .map_err(ServiceError::from)?
            .into_iter()
            .filter_map(|item| {
                let added = item.added_at.clone();
                item.track.and_then(|t| TrackInfo::from_playable(t, added))
            })
            .collect::<Vec<_>>(),
    };
    sink(OpUpdate::Log(format!(
        "'{label}': {} playable tracks",
        tracks.len()
    )));
    Ok(tracks)
}

/// URIs that can be written back through the API (drops local files and
/// anything without a proper Spotify URI).
pub fn addable_uris(tracks: &[TrackInfo], sink: Sink<'_>) -> Vec<String> {
    let uris: Vec<String> = tracks
        .iter()
        .filter(|t| t.is_addable())
        .map(|t| t.uri.clone())
        .collect();
    let skipped = tracks.len() - uris.len();
    if skipped > 0 {
        sink(OpUpdate::Log(format!(
            "{skipped} item(s) skipped (local files can't be re-added through the Web API)"
        )));
    }
    uris
}

/// Write `uris` either to a brand-new playlist or — for session playlists
/// only — in place. The safety policy is enforced inside `SpotifyService`;
/// a protected in-place target fails loudly BEFORE any request is sent.
pub async fn write_output(
    svc: &mut SpotifyService,
    in_place: Option<(&PlaylistId, &str)>,
    new_name: &str,
    new_description: &str,
    uris: &[String],
    public: bool,
    sink: Sink<'_>,
) -> Result<WriteDestination, OpError> {
    if uris.is_empty() {
        return Err(OpError::Other(
            "nothing to write — no playable tracks".into(),
        ));
    }
    match in_place {
        Some((id, name)) => {
            sink(OpUpdate::Log(format!(
                "Replacing contents of session playlist '{name}'…"
            )));
            let mut progress = |done: u64, total: Option<u64>| {
                sink(OpUpdate::Progress {
                    label: format!("Writing '{name}'"),
                    done,
                    total,
                });
            };
            svc.replace_items(id, name, uris, &mut progress).await?;
            Ok(WriteDestination::EditedInPlace {
                name: name.to_string(),
                track_count: uris.len(),
            })
        }
        None => {
            sink(OpUpdate::Log(format!(
                "Creating new playlist '{new_name}'…"
            )));
            let playlist = svc
                .create_playlist(new_name, new_description, public)
                .await?;
            let id = PlaylistId(playlist.id.clone());
            let mut progress = |done: u64, total: Option<u64>| {
                sink(OpUpdate::Progress {
                    label: format!("Adding tracks to '{new_name}'"),
                    done,
                    total,
                });
            };
            svc.add_items(&id, &playlist.name, uris, &mut progress)
                .await?;
            Ok(WriteDestination::NewPlaylist(CreatedPlaylist {
                name: playlist.name,
                track_count: uris.len(),
            }))
        }
    }
}

pub fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

pub const APP_SIGNATURE: &str = "Created with Playlist Studio";
