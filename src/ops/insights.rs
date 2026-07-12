//! Listening insights from what the API still exposes in 2026: the
//! recently-played window (max 50 plays) and top artists/tracks over three
//! time ranges. (Full listening history is not available via the Web API.)

use std::collections::HashMap;

use chrono::{DateTime, Local, Utc};

use crate::spotify::service::{ServiceError, SpotifyService};

use super::{OpError, OpUpdate, Sink};

#[derive(Clone, Debug)]
pub struct RecentRow {
    pub when_local: String,
    pub title: String,
    pub artists: String,
}

#[derive(Clone, Debug)]
pub struct TopList {
    pub range_label: &'static str,
    /// Artist name plus genres when Spotify still supplies them (the field
    /// is deprecated and frequently empty).
    pub artists: Vec<String>,
    /// "Artist — Title" lines.
    pub tracks: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct InsightsData {
    pub recent: Vec<RecentRow>,
    pub recent_artist_counts: Vec<(String, usize)>,
    pub hour_histogram: [u32; 24],
    pub tops: Vec<TopList>,
}

const RANGES: [(&str, &str); 3] = [
    ("short_term", "Last ~4 weeks"),
    ("medium_term", "Last ~6 months"),
    ("long_term", "All-time-ish (~1 year+)"),
];

pub async fn gather_insights(
    svc: &mut SpotifyService,
    sink: Sink<'_>,
) -> Result<InsightsData, OpError> {
    let mut data = InsightsData::default();

    sink(OpUpdate::Log("Fetching recently played…".into()));
    let history = svc
        .reads()
        .recently_played()
        .await
        .map_err(ServiceError::from)?;
    let mut artist_counts: HashMap<String, usize> = HashMap::new();
    for item in &history {
        let played_utc: Option<DateTime<Utc>> = item.played_at.parse::<DateTime<Utc>>().ok();
        let local = played_utc.map(|t| t.with_timezone(&Local));
        if let Some(t) = &local {
            let hour = t.format("%H").to_string().parse::<usize>().unwrap_or(0) % 24;
            data.hour_histogram[hour] += 1;
        }
        let artists = item
            .track
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        for a in &item.track.artists {
            *artist_counts.entry(a.name.clone()).or_insert(0) += 1;
        }
        data.recent.push(RecentRow {
            when_local: local
                .map(|t| t.format("%a %b %-d, %H:%M").to_string())
                .unwrap_or_else(|| item.played_at.clone()),
            title: item.track.name.clone(),
            artists,
        });
    }
    let mut counts: Vec<(String, usize)> = artist_counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.truncate(10);
    data.recent_artist_counts = counts;

    for (range, label) in RANGES {
        sink(OpUpdate::Log(format!("Fetching top items ({label})…")));
        let artists = svc
            .reads()
            .top_artists(range)
            .await
            .map_err(ServiceError::from)?
            .into_iter()
            .map(|a| {
                if a.genres.is_empty() {
                    a.name
                } else {
                    format!("{} ({})", a.name, a.genres.join(", "))
                }
            })
            .take(25)
            .collect();
        let tracks = svc
            .reads()
            .top_tracks(range)
            .await
            .map_err(ServiceError::from)?
            .into_iter()
            .map(|t| {
                let artists = t
                    .artists
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{artists} — {}", t.name)
            })
            .take(25)
            .collect();
        data.tops.push(TopList {
            range_label: label,
            artists,
            tracks,
        });
    }

    Ok(data)
}
