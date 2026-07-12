//! Serde models for the subset of the Spotify Web API this app uses, plus
//! the owned `TrackInfo` shape the rest of the app works with.
//!
//! Deserialization is deliberately permissive (`#[serde(default)]`
//! everywhere it is safe): playlists can contain `null` items, local files,
//! and podcast episodes, and the API occasionally omits fields. A malformed
//! item should degrade, not fail the whole page.

use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct UserProfile {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl UserProfile {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub items: Vec<T>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SimplifiedPlaylist {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner: Option<PlaylistOwner>,
    #[serde(default)]
    pub collaborative: bool,
    /// Feb 2026 renamed the playlist object's `tracks` field to `items`;
    /// accept either spelling.
    #[serde(default, alias = "items")]
    pub tracks: Option<TracksRef>,
}

impl SimplifiedPlaylist {
    pub fn owner_label(&self) -> String {
        self.owner
            .as_ref()
            .map(|o| o.display_name.clone().unwrap_or_else(|| o.id.clone()))
            .unwrap_or_default()
    }
    pub fn total_tracks(&self) -> u64 {
        self.tracks.as_ref().map(|t| t.total).unwrap_or(0)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct PlaylistOwner {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TracksRef {
    #[serde(default)]
    pub total: u64,
}

/// One entry of `GET /playlists/{id}/tracks`.
#[derive(Deserialize, Clone, Debug)]
pub struct PlaylistItem {
    #[serde(default)]
    pub added_at: Option<String>,
    /// `null` for tracks that are no longer available.
    #[serde(default)]
    pub track: Option<PlayableItem>,
}

/// A track or podcast episode as it appears inside playlists, saved tracks,
/// top lists, and search results. Episode objects lack `artists`/`album`;
/// the permissive defaults absorb that.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct PlayableItem {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub album: Option<AlbumRef>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default, rename = "type")]
    pub item_type: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ArtistRef {
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AlbumRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub release_date: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SavedTrackItem {
    #[serde(default)]
    pub added_at: Option<String>,
    #[serde(default)]
    pub track: Option<PlayableItem>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FullArtist {
    #[serde(default)]
    pub name: String,
    /// Deprecated by Spotify and frequently empty — displayed only when
    /// present.
    #[serde(default)]
    pub genres: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PlayHistoryItem {
    #[serde(default)]
    pub track: PlayableItem,
    #[serde(default)]
    pub played_at: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SearchResponse {
    #[serde(default)]
    pub tracks: Option<Page<PlayableItem>>,
}

/// Owned, display-ready track used across ops and the UI.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub release_date: Option<String>,
    pub duration_ms: u64,
    pub is_local: bool,
    pub is_episode: bool,
    pub added_at: Option<String>,
}

impl TrackInfo {
    pub fn from_playable(p: PlayableItem, added_at: Option<String>) -> Option<TrackInfo> {
        if p.uri.is_empty() {
            return None;
        }
        Some(TrackInfo {
            uri: p.uri,
            name: p.name,
            artists: p.artists.into_iter().map(|a| a.name).collect(),
            album: p.album.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
            release_date: p.album.and_then(|a| a.release_date),
            duration_ms: p.duration_ms,
            is_local: p.is_local,
            is_episode: p.item_type.as_deref() == Some("episode"),
            added_at,
        })
    }

    pub fn artist_line(&self) -> String {
        self.artists.join(", ")
    }

    /// URI usable in playlist-mutation requests. Local files
    /// (`spotify:local:...`) cannot be added back through the Web API.
    pub fn is_addable(&self) -> bool {
        !self.is_local && self.uri.starts_with("spotify:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_item_with_null_track_deserializes() {
        let item: PlaylistItem =
            serde_json::from_str(r#"{"added_at": "2020-01-01T00:00:00Z", "track": null}"#).unwrap();
        assert!(item.track.is_none());
    }

    #[test]
    fn episode_item_deserializes_without_artists() {
        let raw = r#"{"id":"ep1","uri":"spotify:episode:ep1","name":"Some Episode",
                      "duration_ms":100,"type":"episode"}"#;
        let p: PlayableItem = serde_json::from_str(raw).unwrap();
        let info = TrackInfo::from_playable(p, None).unwrap();
        assert!(info.is_episode);
        assert!(info.artists.is_empty());
        assert!(info.is_addable());
    }

    #[test]
    fn local_files_are_not_addable() {
        let p = PlayableItem {
            uri: "spotify:local:artist:album:title:180".into(),
            is_local: true,
            ..Default::default()
        };
        assert!(!TrackInfo::from_playable(p, None).unwrap().is_addable());
    }

    #[test]
    fn track_without_uri_is_dropped() {
        assert!(TrackInfo::from_playable(PlayableItem::default(), None).is_none());
    }
}
