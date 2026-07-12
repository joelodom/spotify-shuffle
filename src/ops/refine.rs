//! AI refinement: "more like this, less like that".
//!
//! Refining a PROTECTED source always produces a NEW playlist. Refining a
//! SESSION playlist may edit it in place when `in_place` is requested — and
//! even then the service re-validates the tier before any write.

use crate::ai::AiProvider;
use crate::ai::prompts;
use crate::safety::Tier;
use crate::spotify::service::SpotifyService;

use super::resolve::{ResolutionReport, known_uris, resolve_suggestions};
use super::{
    APP_SIGNATURE, OpError, OpUpdate, Sink, TrackSource, WriteDestination, fetch_source_tracks,
    write_output,
};

/// Cap on how many current tracks are shown to the model.
const PROMPT_TRACK_CAP: usize = 200;

#[derive(Clone, Debug)]
pub struct RefineOutcome {
    pub destination: WriteDestination,
    pub report: ResolutionReport,
}

pub async fn refine_playlist(
    svc: &mut SpotifyService,
    ai: &dyn AiProvider,
    source: &TrackSource,
    instruction: &str,
    in_place: bool,
    public: bool,
    sink: Sink<'_>,
) -> Result<RefineOutcome, OpError> {
    let tracks = fetch_source_tracks(svc, source, sink).await?;
    if tracks.is_empty() {
        return Err(OpError::Other("the source has no tracks to refine".into()));
    }

    let mut listing: Vec<(String, String)> = tracks
        .iter()
        .map(|t| {
            (
                t.artists.first().cloned().unwrap_or_default(),
                t.name.clone(),
            )
        })
        .collect();
    if listing.len() > PROMPT_TRACK_CAP {
        sink(OpUpdate::Log(format!(
            "Source has {} tracks; showing the first {PROMPT_TRACK_CAP} to the model",
            listing.len()
        )));
        listing.truncate(PROMPT_TRACK_CAP);
    }

    sink(OpUpdate::Log(format!(
        "Asking {} to revise the playlist…",
        ai.describe()
    )));
    let request = prompts::refinement_request(&source.label(), &listing, instruction);
    let response = ai.complete(&request).await?;
    let spec = prompts::parse_playlist_spec(&response)?;
    sink(OpUpdate::Log(format!(
        "AI returned {} tracks; matching them on Spotify…",
        spec.tracks.len()
    )));

    // Tracks kept from the source reuse their exact URIs — no search needed.
    let known = known_uris(&tracks);
    let (uris, report) = resolve_suggestions(svc, &spec.tracks, &known, sink).await?;
    if uris.is_empty() {
        return Err(OpError::Other(
            "the revision resolved to zero tracks".into(),
        ));
    }

    // In-place is only meaningful for a session playlist target. This check
    // is UX-level; the authoritative gate lives in SpotifyService.
    let in_place_target = match (in_place, source) {
        (true, TrackSource::Playlist { id, name }) if svc.tier(id) == Tier::Session => {
            Some((id.clone(), name.clone()))
        }
        (true, _) => {
            sink(OpUpdate::Log(
                "In-place refinement is only allowed for playlists created this session; \
                 writing to a new playlist instead."
                    .into(),
            ));
            None
        }
        (false, _) => None,
    };

    let new_name = if spec.name.trim().is_empty() || spec.name == source.label() {
        format!("{} (refined)", source.label())
    } else {
        spec.name.clone()
    };
    let description = format!(
        "{} · Refined from '{}' · {APP_SIGNATURE}",
        spec.description, // may be empty; harmless
        source.label()
    );

    let destination = write_output(
        svc,
        in_place_target
            .as_ref()
            .map(|(id, name)| (id, name.as_str())),
        &new_name,
        &description,
        &uris,
        public,
        sink,
    )
    .await?;

    Ok(RefineOutcome {
        destination,
        report,
    })
}
