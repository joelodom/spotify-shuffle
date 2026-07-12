//! Resolve AI-suggested (artist, title) pairs to real Spotify track URIs.
//!
//! Search has been limited to 10 results per query since Feb 2026, so
//! resolution leans on precision: a field-filtered query first, a plain
//! query as fallback, and a scoring pass that requires both the title and
//! the artist to plausibly match before accepting a candidate.

use std::collections::HashMap;

use crate::ai::prompts::SuggestedTrack;
use crate::spotify::models::{PlayableItem, TrackInfo};
use crate::spotify::service::{ServiceError, SpotifyService};
use crate::util::normalize_for_match;

use super::{OpError, OpUpdate, Sink};

#[derive(Clone, Debug, Default)]
pub struct ResolutionReport {
    pub resolved: usize,
    pub reused_from_library: usize,
    pub unresolved: Vec<String>,
}

/// Key used to match suggestions against known library tracks.
pub fn match_key(artist: &str, title: &str) -> String {
    format!(
        "{}|{}",
        normalize_for_match(artist),
        normalize_for_match(title)
    )
}

/// Build a suggestion→URI lookup from tracks we already know (library
/// digests, current playlist contents), so resolution avoids search calls
/// and keeps the user's exact versions.
pub fn known_uris(tracks: &[TrackInfo]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for t in tracks {
        if !t.is_addable() {
            continue;
        }
        if let Some(primary) = t.artists.first() {
            map.entry(match_key(primary, &t.name))
                .or_insert_with(|| t.uri.clone());
        }
    }
    map
}

fn score_candidate(candidate: &PlayableItem, artist_n: &str, title_n: &str) -> i32 {
    let cand_title = normalize_for_match(&candidate.name);
    let title_score = if cand_title == title_n {
        3
    } else if cand_title.starts_with(title_n) || title_n.starts_with(&cand_title) {
        2
    } else if cand_title.contains(title_n) || title_n.contains(&cand_title) {
        1
    } else {
        0
    };
    let artist_score = candidate
        .artists
        .iter()
        .map(|a| {
            let cand_artist = normalize_for_match(&a.name);
            if cand_artist == artist_n {
                3
            } else if cand_artist.contains(artist_n) || artist_n.contains(&cand_artist) {
                2
            } else {
                0
            }
        })
        .max()
        .unwrap_or(0);
    title_score + artist_score
}

const ACCEPT_THRESHOLD: i32 = 4;

async fn best_match(
    svc: &mut SpotifyService,
    artist: &str,
    title: &str,
) -> Result<Option<String>, ServiceError> {
    let artist_n = normalize_for_match(artist);
    let title_n = normalize_for_match(title);

    let field_query = format!("track:\"{title}\" artist:\"{artist}\"");
    let plain_query = format!("{artist} {title}");

    for query in [field_query, plain_query] {
        let candidates = svc.reads().search_tracks(&query, 5).await?;
        let best = candidates
            .into_iter()
            .filter(|c| !c.uri.is_empty() && !c.is_local)
            .map(|c| (score_candidate(&c, &artist_n, &title_n), c))
            .max_by_key(|(score, _)| *score);
        if let Some((score, candidate)) = best
            && score >= ACCEPT_THRESHOLD
        {
            return Ok(Some(candidate.uri));
        }
    }
    Ok(None)
}

/// Resolve suggestions to URIs, deduplicating while preserving order.
pub async fn resolve_suggestions(
    svc: &mut SpotifyService,
    suggestions: &[SuggestedTrack],
    known: &HashMap<String, String>,
    sink: Sink<'_>,
) -> Result<(Vec<String>, ResolutionReport), OpError> {
    let mut uris: Vec<String> = Vec::with_capacity(suggestions.len());
    let mut seen = std::collections::HashSet::new();
    let mut report = ResolutionReport::default();
    let total = suggestions.len() as u64;

    for (i, s) in suggestions.iter().enumerate() {
        sink(OpUpdate::Progress {
            label: format!("Resolving '{} — {}'", s.artist, s.title),
            done: i as u64,
            total: Some(total),
        });
        let uri = match known.get(&match_key(&s.artist, &s.title)) {
            Some(uri) => {
                report.reused_from_library += 1;
                Some(uri.clone())
            }
            None => best_match(svc, &s.artist, &s.title).await?,
        };
        match uri {
            Some(uri) => {
                if seen.insert(uri.clone()) {
                    uris.push(uri);
                    report.resolved += 1;
                }
            }
            None => report
                .unresolved
                .push(format!("{} — {}", s.artist, s.title)),
        }
    }
    sink(OpUpdate::Progress {
        label: "Resolution complete".into(),
        done: total,
        total: Some(total),
    });
    if !report.unresolved.is_empty() {
        sink(OpUpdate::Log(format!(
            "{} suggestion(s) could not be found on Spotify: {}",
            report.unresolved.len(),
            report.unresolved.join("; ")
        )));
    }
    Ok((uris, report))
}
