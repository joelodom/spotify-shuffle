//! Power-user playlist tools: unbiased shuffle, dedupe, merge, sort.
//!
//! All of these read any source (protected sources are fine to READ) and
//! write results either to a NEW playlist or — only for session playlists,
//! only when asked — in place.

use std::collections::HashSet;

use crate::safety::{PlaylistId, Tier};
use crate::shuffle::unbiased_shuffled;
use crate::spotify::models::TrackInfo;
use crate::spotify::service::SpotifyService;
use crate::util::normalize_for_match;

use super::{
    APP_SIGNATURE, OpError, OpUpdate, Sink, TrackSource, WriteDestination, addable_uris,
    fetch_source_tracks, timestamp, write_output,
};

/// Create a NEW playlist containing the source's tracks in a statistically
/// unbiased Fisher–Yates order (ChaCha20 CSPRNG). The source is never
/// touched — this is the spiritual successor of the original
/// spotify-shuffle.py script.
pub async fn shuffle_to_new_playlist(
    svc: &mut SpotifyService,
    source: &TrackSource,
    public: bool,
    sink: Sink<'_>,
) -> Result<WriteDestination, OpError> {
    let tracks = fetch_source_tracks(svc, source, sink).await?;
    let uris = addable_uris(&tracks, sink);
    if uris.is_empty() {
        return Err(OpError::Other("the source has no addable tracks".into()));
    }
    sink(OpUpdate::Log(format!(
        "Shuffling {} tracks with a CSPRNG-seeded Fisher–Yates pass…",
        uris.len()
    )));
    let shuffled = unbiased_shuffled(&uris);
    let name = format!("{} (unbiased shuffle {})", source.label(), timestamp());
    let description = format!(
        "Statistically unbiased shuffle of '{}' — {} tracks · {APP_SIGNATURE}",
        source.label(),
        shuffled.len()
    );
    write_output(svc, None, &name, &description, &shuffled, public, sink).await
}

#[derive(Clone, Debug)]
pub struct DedupeOutcome {
    pub destination: Option<WriteDestination>,
    pub removed: Vec<String>,
}

/// Remove duplicates: exact URI repeats plus fuzzy repeats (same normalized
/// title + primary artist, e.g. album vs. single edition). First occurrence
/// wins; order is otherwise preserved.
pub async fn dedupe_playlist(
    svc: &mut SpotifyService,
    source: &TrackSource,
    in_place: bool,
    public: bool,
    sink: Sink<'_>,
) -> Result<DedupeOutcome, OpError> {
    let tracks = fetch_source_tracks(svc, source, sink).await?;
    let mut seen_uris: HashSet<String> = HashSet::new();
    let mut seen_fuzzy: HashSet<String> = HashSet::new();
    let mut kept: Vec<&TrackInfo> = Vec::new();
    let mut removed: Vec<String> = Vec::new();

    for t in &tracks {
        if !t.is_addable() {
            continue;
        }
        let fuzzy = format!(
            "{}|{}",
            normalize_for_match(t.artists.first().map(String::as_str).unwrap_or("")),
            normalize_for_match(&t.name)
        );
        if seen_uris.contains(&t.uri) || seen_fuzzy.contains(&fuzzy) {
            removed.push(format!("{} — {}", t.artist_line(), t.name));
        } else {
            seen_uris.insert(t.uri.clone());
            seen_fuzzy.insert(fuzzy);
            kept.push(t);
        }
    }

    if removed.is_empty() {
        sink(OpUpdate::Log(format!(
            "No duplicates found in '{}'",
            source.label()
        )));
        return Ok(DedupeOutcome {
            destination: None,
            removed,
        });
    }
    sink(OpUpdate::Log(format!(
        "Found {} duplicate(s): {}",
        removed.len(),
        removed
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    )));

    let uris: Vec<String> = kept.iter().map(|t| t.uri.clone()).collect();
    let in_place_target = in_place_target_for(svc, source, in_place, sink);
    let name = format!("{} (deduped)", source.label());
    let description = format!(
        "'{}' with {} duplicate(s) removed · {APP_SIGNATURE}",
        source.label(),
        removed.len()
    );
    let destination = write_output(
        svc,
        in_place_target.as_ref().map(|(id, n)| (id, n.as_str())),
        &name,
        &description,
        &uris,
        public,
        sink,
    )
    .await?;
    Ok(DedupeOutcome {
        destination: Some(destination),
        removed,
    })
}

/// Concatenate several sources into a NEW playlist, optionally deduped
/// (exact URI) and/or shuffled.
pub async fn merge_playlists(
    svc: &mut SpotifyService,
    sources: &[TrackSource],
    dedupe: bool,
    shuffle: bool,
    public: bool,
    sink: Sink<'_>,
) -> Result<WriteDestination, OpError> {
    if sources.len() < 2 {
        return Err(OpError::Other(
            "select at least two sources to merge".into(),
        ));
    }
    let mut uris: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for source in sources {
        let tracks = fetch_source_tracks(svc, source, sink).await?;
        for uri in addable_uris(&tracks, sink) {
            if !dedupe || seen.insert(uri.clone()) {
                uris.push(uri);
            }
        }
    }
    if uris.is_empty() {
        return Err(OpError::Other(
            "the selected sources have no addable tracks".into(),
        ));
    }
    if shuffle {
        uris = unbiased_shuffled(&uris);
    }
    let mut name = format!(
        "Merged: {}",
        sources
            .iter()
            .map(TrackSource::label)
            .collect::<Vec<_>>()
            .join(" + ")
    );
    if name.chars().count() > 90 {
        name = name.chars().take(89).collect::<String>() + "…";
    }
    let description = format!(
        "Merge of {} sources ({} tracks{}{}) · {APP_SIGNATURE}",
        sources.len(),
        uris.len(),
        if dedupe { ", deduped" } else { "" },
        if shuffle { ", shuffled" } else { "" },
    );
    write_output(svc, None, &name, &description, &uris, public, sink).await
}

/// Metadata sort keys. Audio-feature sorting (BPM/energy/…) is impossible on
/// today's API — those endpoints were removed for new apps in Nov 2024 — and
/// `popularity` is stripped in development mode, so the keys below are what
/// genuinely works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Title,
    Artist,
    Album,
    ReleaseDate,
    Duration,
    DateAdded,
}

impl SortKey {
    pub const ALL: [SortKey; 6] = [
        SortKey::Title,
        SortKey::Artist,
        SortKey::Album,
        SortKey::ReleaseDate,
        SortKey::Duration,
        SortKey::DateAdded,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::ReleaseDate => "Release date",
            SortKey::Duration => "Duration",
            SortKey::DateAdded => "Date added",
        }
    }
}

pub async fn sort_playlist(
    svc: &mut SpotifyService,
    source: &TrackSource,
    key: SortKey,
    descending: bool,
    in_place: bool,
    public: bool,
    sink: Sink<'_>,
) -> Result<WriteDestination, OpError> {
    let mut tracks: Vec<TrackInfo> = fetch_source_tracks(svc, source, sink)
        .await?
        .into_iter()
        .filter(|t| t.is_addable())
        .collect();
    if tracks.is_empty() {
        return Err(OpError::Other("the source has no addable tracks".into()));
    }
    tracks.sort_by(|a, b| {
        let ord = match key {
            SortKey::Title => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Artist => a
                .artist_line()
                .to_lowercase()
                .cmp(&b.artist_line().to_lowercase()),
            SortKey::Album => a.album.to_lowercase().cmp(&b.album.to_lowercase()),
            // ISO-8601 date strings compare correctly lexicographically.
            SortKey::ReleaseDate => a.release_date.cmp(&b.release_date),
            SortKey::Duration => a.duration_ms.cmp(&b.duration_ms),
            SortKey::DateAdded => a.added_at.cmp(&b.added_at),
        };
        if descending { ord.reverse() } else { ord }
    });
    let uris: Vec<String> = tracks.iter().map(|t| t.uri.clone()).collect();

    let in_place_target = in_place_target_for(svc, source, in_place, sink);
    let direction = if descending {
        "descending"
    } else {
        "ascending"
    };
    let name = format!(
        "{} (by {} {direction})",
        source.label(),
        key.label().to_lowercase()
    );
    let description = format!(
        "'{}' sorted by {} ({direction}) · {APP_SIGNATURE}",
        source.label(),
        key.label().to_lowercase()
    );
    write_output(
        svc,
        in_place_target.as_ref().map(|(id, n)| (id, n.as_str())),
        &name,
        &description,
        &uris,
        public,
        sink,
    )
    .await
}

/// UX-level in-place gate (the service holds the authoritative one):
/// in-place is honored only for playlists created this session.
fn in_place_target_for(
    svc: &SpotifyService,
    source: &TrackSource,
    requested: bool,
    sink: Sink<'_>,
) -> Option<(PlaylistId, String)> {
    match (requested, source) {
        (true, TrackSource::Playlist { id, name }) if svc.tier(id) == Tier::Session => {
            Some((id.clone(), name.clone()))
        }
        (true, _) => {
            sink(OpUpdate::Log(
                "In-place editing is only allowed for playlists created this session; \
                 writing to a new playlist instead."
                    .into(),
            ));
            None
        }
        (false, _) => None,
    }
}
