//! Learn-from-library: digest the user's playlists, Liked Songs, and top
//! artists; let the AI design improved/reorganized playlists; create them as
//! NEW playlists. Protected originals are never touched.

use crate::ai::AiProvider;
use crate::ai::prompts;
use crate::spotify::models::TrackInfo;
use crate::spotify::service::{ServiceError, SpotifyService};

use super::resolve::{known_uris, resolve_suggestions};
use super::{APP_SIGNATURE, CreatedPlaylist, OpError, OpUpdate, Sink, write_output};

/// Caps keep the digest inside a sane token budget.
const MAX_PLAYLISTS_IN_DIGEST: usize = 25;
const SAMPLE_TRACKS_PER_PLAYLIST: usize = 15;
const LIKED_SAMPLE: usize = 150;

#[derive(Clone, Debug)]
pub struct OrganizeOutcome {
    pub created: Vec<CreatedPlaylist>,
    pub notes: Option<String>,
    pub unresolved_total: usize,
}

/// Build a text digest of the library plus a URI lookup for exact matches.
async fn build_digest(
    svc: &mut SpotifyService,
    sink: Sink<'_>,
) -> Result<(String, Vec<TrackInfo>), OpError> {
    let me = svc.ensure_me().await?;
    let mut digest = String::new();
    let mut all_tracks: Vec<TrackInfo> = Vec::new();

    sink(OpUpdate::Log("Listing your playlists…".into()));
    let mut progress = |done: u64, total: Option<u64>| {
        sink(OpUpdate::Progress {
            label: "Listing playlists".into(),
            done,
            total,
        });
    };
    let playlists = svc
        .reads()
        .my_playlists(&mut progress)
        .await
        .map_err(ServiceError::from)?;

    // Only owned/collaborative playlists have readable contents in
    // development mode (403 otherwise).
    let readable: Vec<_> = playlists
        .iter()
        .filter(|p| p.collaborative || p.owner.as_ref().map(|o| o.id == me.id).unwrap_or(false))
        .take(MAX_PLAYLISTS_IN_DIGEST)
        .collect();
    sink(OpUpdate::Log(format!(
        "Sampling {} of your playlists ({} total; only owned/collaborative ones are readable)",
        readable.len(),
        playlists.len()
    )));

    for (i, p) in readable.iter().enumerate() {
        sink(OpUpdate::Progress {
            label: format!("Sampling '{}'", p.name),
            done: i as u64,
            total: Some(readable.len() as u64),
        });
        // First page (50 tracks) is plenty for a taste sample.
        let items = match svc.reads().playlist_items_first_page(&p.id).await {
            Ok(items) => items,
            Err(e) => {
                sink(OpUpdate::Log(format!("Skipping '{}' ({e})", p.name)));
                continue;
            }
        };
        let tracks: Vec<TrackInfo> = items
            .into_iter()
            .filter_map(|it| {
                let added = it.added_at.clone();
                it.track.and_then(|t| TrackInfo::from_playable(t, added))
            })
            .collect();
        digest.push_str(&format!(
            "PLAYLIST: {} ({} tracks{})\n",
            p.name,
            p.total_tracks(),
            p.description
                .as_deref()
                .filter(|d| !d.is_empty())
                .map(|d| format!(" — {d}"))
                .unwrap_or_default()
        ));
        for t in tracks.iter().take(SAMPLE_TRACKS_PER_PLAYLIST) {
            digest.push_str(&format!("  - {} — {}\n", t.artist_line(), t.name));
        }
        all_tracks.extend(tracks);
    }

    sink(OpUpdate::Log("Sampling Liked Songs…".into()));
    let mut progress = |done: u64, total: Option<u64>| {
        sink(OpUpdate::Progress {
            label: "Reading Liked Songs".into(),
            done,
            total,
        });
    };
    let liked: Vec<TrackInfo> = svc
        .reads()
        .saved_tracks(&mut progress)
        .await
        .map_err(ServiceError::from)?
        .into_iter()
        .filter_map(|it| {
            let added = it.added_at.clone();
            it.track.and_then(|t| TrackInfo::from_playable(t, added))
        })
        .collect();
    let step = (liked.len() / LIKED_SAMPLE).max(1);
    digest.push_str(&format!(
        "\nLIKED SONGS (sample of {} out of {}):\n",
        liked.len().min(LIKED_SAMPLE),
        liked.len()
    ));
    for t in liked.iter().step_by(step).take(LIKED_SAMPLE) {
        digest.push_str(&format!("  - {} — {}\n", t.artist_line(), t.name));
    }
    all_tracks.extend(liked);

    if let Ok(top) = svc.reads().top_artists("medium_term").await
        && !top.is_empty()
    {
        digest.push_str("\nMOST PLAYED ARTISTS (last ~6 months): ");
        digest.push_str(
            &top.iter()
                .take(20)
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        digest.push('\n');
    }

    Ok((digest, all_tracks))
}

pub async fn organize_library(
    svc: &mut SpotifyService,
    ai: &dyn AiProvider,
    goal: &str,
    max_playlists: usize,
    public: bool,
    sink: Sink<'_>,
) -> Result<OrganizeOutcome, OpError> {
    let max_playlists = max_playlists.clamp(1, 10);
    let (digest, library_tracks) = build_digest(svc, sink).await?;
    if library_tracks.is_empty() {
        return Err(OpError::Other("your library appears to be empty".into()));
    }

    sink(OpUpdate::Log(format!(
        "Asking {} to design up to {max_playlists} playlists…",
        ai.describe()
    )));
    let request = prompts::organize_request(&digest, goal, max_playlists);
    let response = ai.complete(&request).await?;
    let plan = prompts::parse_organize_plan(&response)?;
    if let Some(notes) = &plan.notes {
        sink(OpUpdate::Log(format!("AI plan: {notes}")));
    }

    let known = known_uris(&library_tracks);
    let mut created = Vec::new();
    let mut unresolved_total = 0usize;
    for spec in plan.playlists.into_iter().take(max_playlists) {
        sink(OpUpdate::Log(format!(
            "Building '{}' ({} suggested tracks)…",
            spec.name,
            spec.tracks.len()
        )));
        let (uris, report) = resolve_suggestions(svc, &spec.tracks, &known, sink).await?;
        unresolved_total += report.unresolved.len();
        if uris.is_empty() {
            sink(OpUpdate::Log(format!(
                "Skipping '{}' — no tracks resolved",
                spec.name
            )));
            continue;
        }
        let description = format!("{} · {APP_SIGNATURE}", spec.description);
        let dest = write_output(svc, None, &spec.name, &description, &uris, public, sink).await?;
        if let Some(p) = dest.created() {
            created.push(p.clone());
        }
    }
    if created.is_empty() {
        return Err(OpError::Other(
            "no playlists could be created from the plan".into(),
        ));
    }
    Ok(OrganizeOutcome {
        created,
        notes: plan.notes,
        unresolved_total,
    })
}
