//! Export playlists/Liked Songs to CSV or JSON files; import a pasted track
//! list into a new playlist.

use std::path::Path;

use serde::Serialize;

use crate::ai::prompts::SuggestedTrack;
use crate::spotify::service::SpotifyService;

use super::resolve::{ResolutionReport, resolve_suggestions};
use super::{
    APP_SIGNATURE, OpError, OpUpdate, Sink, TrackSource, WriteDestination, fetch_source_tracks,
    write_output,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

#[derive(Serialize)]
struct ExportedTrack<'a> {
    position: usize,
    title: &'a str,
    artists: String,
    album: &'a str,
    release_date: Option<&'a str>,
    duration_ms: u64,
    added_at: Option<&'a str>,
    uri: &'a str,
    is_local: bool,
}

#[derive(Serialize)]
struct ExportEnvelope<'a> {
    exported_from: String,
    exported_at: String,
    exported_by: &'static str,
    track_count: usize,
    tracks: Vec<ExportedTrack<'a>>,
}

pub async fn export_source(
    svc: &mut SpotifyService,
    source: &TrackSource,
    format: ExportFormat,
    path: &Path,
    sink: Sink<'_>,
) -> Result<String, OpError> {
    let tracks = fetch_source_tracks(svc, source, sink).await?;
    if tracks.is_empty() {
        return Err(OpError::Other("nothing to export".into()));
    }
    let rows: Vec<ExportedTrack> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| ExportedTrack {
            position: i + 1,
            title: &t.name,
            artists: t.artist_line(),
            album: &t.album,
            release_date: t.release_date.as_deref(),
            duration_ms: t.duration_ms,
            added_at: t.added_at.as_deref(),
            uri: &t.uri,
            is_local: t.is_local,
        })
        .collect();

    match format {
        ExportFormat::Csv => {
            let mut writer = csv::Writer::from_path(path)
                .map_err(|e| OpError::Other(format!("could not open {path:?}: {e}")))?;
            for row in &rows {
                writer
                    .serialize(row)
                    .map_err(|e| OpError::Other(format!("CSV write failed: {e}")))?;
            }
            writer
                .flush()
                .map_err(|e| OpError::Other(format!("CSV write failed: {e}")))?;
        }
        ExportFormat::Json => {
            let envelope = ExportEnvelope {
                exported_from: source.label(),
                exported_at: chrono::Utc::now().to_rfc3339(),
                exported_by: APP_SIGNATURE,
                track_count: rows.len(),
                tracks: rows,
            };
            let text = serde_json::to_string_pretty(&envelope)
                .map_err(|e| OpError::Other(format!("JSON encoding failed: {e}")))?;
            std::fs::write(path, text)
                .map_err(|e| OpError::Other(format!("could not write {path:?}: {e}")))?;
        }
    }
    let summary = format!(
        "Exported {} tracks from '{}' to {}",
        tracks.len(),
        source.label(),
        path.display()
    );
    sink(OpUpdate::Log(summary.clone()));
    Ok(summary)
}

/// Parse pasted text into (artist, title) suggestions. Accepted per line:
/// `Artist - Title`, `Artist — Title`, or `Artist<TAB>Title`. Lines starting
/// with `#` and blank lines are skipped.
pub fn parse_import_lines(text: &str) -> Vec<SuggestedTrack> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (artist, title) = if let Some((a, t)) = line.split_once('\t') {
            (a, t)
        } else if let Some((a, t)) = line.split_once(" — ") {
            (a, t)
        } else if let Some((a, t)) = line.split_once(" - ") {
            (a, t)
        } else {
            continue;
        };
        let (artist, title) = (artist.trim(), title.trim());
        if !artist.is_empty() && !title.is_empty() {
            out.push(SuggestedTrack {
                artist: artist.to_string(),
                title: title.to_string(),
            });
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct ImportOutcome {
    pub destination: WriteDestination,
    pub report: ResolutionReport,
    pub skipped_lines: usize,
}

pub async fn import_tracks(
    svc: &mut SpotifyService,
    name: &str,
    text: &str,
    public: bool,
    sink: Sink<'_>,
) -> Result<ImportOutcome, OpError> {
    let suggestions = parse_import_lines(text);
    let total_lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    if suggestions.is_empty() {
        return Err(OpError::Other(
            "no parsable lines — use one `Artist - Title` per line".into(),
        ));
    }
    sink(OpUpdate::Log(format!(
        "Parsed {} track(s); matching them on Spotify…",
        suggestions.len()
    )));
    let (uris, report) =
        resolve_suggestions(svc, &suggestions, &std::collections::HashMap::new(), sink).await?;
    if uris.is_empty() {
        return Err(OpError::Other(
            "none of the lines matched a Spotify track".into(),
        ));
    }
    let name = if name.trim().is_empty() {
        "Imported playlist"
    } else {
        name.trim()
    };
    let description = format!(
        "Imported from text ({} tracks) · {APP_SIGNATURE}",
        uris.len()
    );
    let destination = write_output(svc, None, name, &description, &uris, public, sink).await?;
    Ok(ImportOutcome {
        destination,
        report,
        skipped_lines: total_lines.saturating_sub(suggestions.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dash_em_dash_and_tab_lines() {
        let text = "# comment\nDaft Punk - Around the World\nBjörk — Hyperballad\nNina\tSimone Song\n\nnot a track line\n";
        let parsed = parse_import_lines(text);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].artist, "Daft Punk");
        assert_eq!(parsed[0].title, "Around the World");
        assert_eq!(parsed[1].artist, "Björk");
        assert_eq!(parsed[2].artist, "Nina");
        assert_eq!(parsed[2].title, "Simone Song");
    }

    #[test]
    fn hyphenated_titles_survive() {
        // Only the FIRST " - " splits; hyphens without spaces never split.
        let parsed = parse_import_lines("Jay-Z - 99 Problems");
        assert_eq!(parsed[0].artist, "Jay-Z");
        assert_eq!(parsed[0].title, "99 Problems");
    }
}
