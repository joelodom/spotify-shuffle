//! AI playlist creation: natural-language description → new playlist.

use std::collections::HashMap;

use crate::ai::AiProvider;
use crate::ai::prompts::{self, TasteContext};
use crate::spotify::service::SpotifyService;

use super::resolve::{ResolutionReport, resolve_suggestions};
use super::{APP_SIGNATURE, CreatedPlaylist, OpError, OpUpdate, Sink, write_output};

#[derive(Clone, Debug)]
pub struct GenerateOutcome {
    pub playlist: CreatedPlaylist,
    pub report: ResolutionReport,
}

/// Optionally gather light taste context (top artists + any genres Spotify
/// still exposes) to personalize generation.
pub async fn gather_taste(
    svc: &mut SpotifyService,
    sink: Sink<'_>,
) -> Result<TasteContext, OpError> {
    sink(OpUpdate::Log(
        "Collecting taste context from your top artists…".into(),
    ));
    let artists = svc
        .reads()
        .top_artists("medium_term")
        .await
        .map_err(crate::spotify::service::ServiceError::from)?;
    let mut genres: Vec<String> = Vec::new();
    for a in &artists {
        for g in &a.genres {
            if !genres.contains(g) {
                genres.push(g.clone());
            }
        }
    }
    genres.truncate(12);
    Ok(TasteContext {
        top_artists: artists.into_iter().map(|a| a.name).take(20).collect(),
        genres,
    })
}

pub async fn generate_playlist(
    svc: &mut SpotifyService,
    ai: &dyn AiProvider,
    description: &str,
    count: usize,
    personalize: bool,
    public: bool,
    sink: Sink<'_>,
) -> Result<GenerateOutcome, OpError> {
    let count = count.clamp(3, 150);
    let taste = if personalize {
        match gather_taste(svc, sink).await {
            Ok(t) => Some(t),
            Err(e) => {
                sink(OpUpdate::Log(format!(
                    "Skipping personalization (couldn't fetch top artists: {e})"
                )));
                None
            }
        }
    } else {
        None
    };

    sink(OpUpdate::Log(format!(
        "Asking {} for {count} tracks…",
        ai.describe()
    )));
    let request = prompts::generation_request(description, count, taste.as_ref());
    let response = ai.complete(&request).await?;
    let spec = prompts::parse_playlist_spec(&response)?;
    sink(OpUpdate::Log(format!(
        "AI proposed '{}' with {} tracks; matching them on Spotify…",
        spec.name,
        spec.tracks.len()
    )));

    let (uris, report) = resolve_suggestions(svc, &spec.tracks, &HashMap::new(), sink).await?;
    if uris.is_empty() {
        return Err(OpError::Other(
            "none of the suggested tracks could be found on Spotify".into(),
        ));
    }

    let description = if spec.description.is_empty() {
        format!("{APP_SIGNATURE} · {description}")
    } else {
        format!("{} · {APP_SIGNATURE}", spec.description)
    };
    let dest = write_output(svc, None, &spec.name, &description, &uris, public, sink).await?;
    let playlist = dest
        .created()
        .cloned()
        .expect("generation always writes a new playlist");
    Ok(GenerateOutcome { playlist, report })
}
