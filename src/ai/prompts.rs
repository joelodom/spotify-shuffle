//! Prompt builders and response schemas for the playlist AI features.
//!
//! Every prompt demands strict JSON; parsing is nevertheless tolerant
//! (fenced blocks, surrounding prose) via [`crate::util::extract_json`].
//! Track resolution happens later against the Spotify search API — the model
//! only ever proposes (artist, title) pairs, it never touches the account.

use serde::Deserialize;

use crate::util::extract_json;

use super::{AiError, AiRequest};

#[derive(Deserialize, Clone, Debug)]
pub struct SuggestedTrack {
    pub artist: String,
    pub title: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct GeneratedPlaylistSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub tracks: Vec<SuggestedTrack>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct OrganizePlan {
    pub playlists: Vec<GeneratedPlaylistSpec>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Optional taste context distilled from the user's library.
#[derive(Clone, Debug, Default)]
pub struct TasteContext {
    pub top_artists: Vec<String>,
    pub genres: Vec<String>,
}

impl TasteContext {
    fn render(&self) -> String {
        if self.top_artists.is_empty() && self.genres.is_empty() {
            return String::new();
        }
        let mut s = String::from("\n\nListener taste context (from their Spotify history):\n");
        if !self.top_artists.is_empty() {
            s.push_str(&format!(
                "- Frequently played artists: {}\n",
                self.top_artists.join(", ")
            ));
        }
        if !self.genres.is_empty() {
            s.push_str(&format!(
                "- Genres they gravitate to: {}\n",
                self.genres.join(", ")
            ));
        }
        s.push_str(
            "Use this to calibrate familiarity vs. discovery, but the description below \
             always wins over taste context.",
        );
        s
    }
}

const CURATOR_SYSTEM: &str = "You are an expert music curator with deep knowledge of recorded \
music across every era and genre. You respond with STRICT JSON only: no markdown fences, no \
commentary before or after the JSON. Every track you propose must be a real, released recording \
likely to exist on Spotify. Use artist names exactly as they are credited on Spotify. Prefer \
original studio versions over live/remaster/karaoke variants unless asked otherwise. Never \
repeat the same artist+title pair.";

fn playlist_schema(count: usize) -> String {
    format!(
        r#"Respond with exactly this JSON shape:
{{
  "name": "<short evocative playlist name>",
  "description": "<one sentence, under 250 characters>",
  "tracks": [ {{ "artist": "<artist>", "title": "<track title>" }}, ... exactly {count} entries ... ]
}}"#
    )
}

/// AI playlist creation from a natural-language description.
pub fn generation_request(
    description: &str,
    count: usize,
    taste: Option<&TasteContext>,
) -> AiRequest {
    let taste_block = taste.map(TasteContext::render).unwrap_or_default();
    AiRequest {
        system: CURATOR_SYSTEM.to_string(),
        user: format!(
            "Create a playlist of exactly {count} tracks matching this description:\n\
             \"{description}\"{taste_block}\n\n{}",
            playlist_schema(count)
        ),
    }
}

/// AI refinement: revise an existing track list per an instruction.
pub fn refinement_request(
    playlist_name: &str,
    current: &[(String, String)],
    instruction: &str,
) -> AiRequest {
    let mut listing = String::new();
    for (i, (artist, title)) in current.iter().enumerate() {
        listing.push_str(&format!("{}. {artist} — {title}\n", i + 1));
    }
    let count_hint = current.len().max(5);
    AiRequest {
        system: CURATOR_SYSTEM.to_string(),
        user: format!(
            "Here is the playlist \"{playlist_name}\" as it stands ({} tracks):\n{listing}\n\
             Revise it according to this instruction:\n\"{instruction}\"\n\n\
             Return the COMPLETE revised track list (not a diff). Keep tracks that still fit, \
             drop the ones the instruction argues against, and add replacements. Stay near \
             {count_hint} tracks unless the instruction implies otherwise.\n\n{}",
            current.len(),
            playlist_schema(count_hint)
        ),
    }
}

/// Learn-from-library reorganization.
pub fn organize_request(library_digest: &str, goal: &str, max_playlists: usize) -> AiRequest {
    AiRequest {
        system: CURATOR_SYSTEM.to_string(),
        user: format!(
            "Below is a digest of a listener's Spotify library. Study it to understand their \
             taste, then design AT MOST {max_playlists} new playlists that accomplish this goal:\n\
             \"{goal}\"\n\n\
             Draw tracks primarily from the digest (copy artist and title EXACTLY as written \
             there, so they can be matched back to the library). You may add a few new \
             suggestions that clearly fit. 15 to 40 tracks per playlist.\n\n\
             === LIBRARY DIGEST ===\n{library_digest}\n=== END DIGEST ===\n\n\
             Respond with exactly this JSON shape:\n\
             {{\n  \"playlists\": [ {{ \"name\": \"...\", \"description\": \"...\", \
             \"tracks\": [ {{ \"artist\": \"...\", \"title\": \"...\" }} ] }} ],\n  \
             \"notes\": \"<one short paragraph explaining the organization>\"\n}}"
        ),
    }
}

pub fn parse_playlist_spec(text: &str) -> Result<GeneratedPlaylistSpec, AiError> {
    let json = extract_json(text)
        .ok_or_else(|| AiError::Parse(format!("no JSON found in: {}", snippet(text))))?;
    let spec: GeneratedPlaylistSpec = serde_json::from_str(&json)
        .map_err(|e| AiError::Parse(format!("{e} in: {}", snippet(&json))))?;
    if spec.tracks.is_empty() {
        return Err(AiError::Parse(
            "the model returned an empty track list".into(),
        ));
    }
    Ok(spec)
}

pub fn parse_organize_plan(text: &str) -> Result<OrganizePlan, AiError> {
    let json = extract_json(text)
        .ok_or_else(|| AiError::Parse(format!("no JSON found in: {}", snippet(text))))?;
    let plan: OrganizePlan = serde_json::from_str(&json)
        .map_err(|e| AiError::Parse(format!("{e} in: {}", snippet(&json))))?;
    if plan.playlists.is_empty() {
        return Err(AiError::Parse("the model returned no playlists".into()));
    }
    Ok(plan)
}

fn snippet(text: &str) -> String {
    let mut s: String = text.chars().take(160).collect();
    if s.len() < text.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json_spec() {
        let text = r#"{"name":"Test","description":"d","tracks":[{"artist":"A","title":"T"}]}"#;
        let spec = parse_playlist_spec(text).unwrap();
        assert_eq!(spec.name, "Test");
        assert_eq!(spec.tracks.len(), 1);
    }

    #[test]
    fn parses_fenced_spec_with_prose() {
        let text = "Here you go!\n```json\n{\"name\":\"Mix\",\"description\":\"\",\
                    \"tracks\":[{\"artist\":\"A\",\"title\":\"T\"},{\"artist\":\"B\",\"title\":\"U\"}]}\n```\nEnjoy.";
        let spec = parse_playlist_spec(text).unwrap();
        assert_eq!(spec.tracks.len(), 2);
    }

    #[test]
    fn rejects_empty_track_list() {
        let text = r#"{"name":"Empty","description":"","tracks":[]}"#;
        assert!(matches!(parse_playlist_spec(text), Err(AiError::Parse(_))));
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_playlist_spec("sorry, I can't").is_err());
    }

    #[test]
    fn parses_organize_plan() {
        let text = r#"{"playlists":[{"name":"P1","description":"","tracks":
                    [{"artist":"A","title":"T"}]}],"notes":"grouped by mood"}"#;
        let plan = parse_organize_plan(text).unwrap();
        assert_eq!(plan.playlists.len(), 1);
        assert_eq!(plan.notes.as_deref(), Some("grouped by mood"));
    }

    #[test]
    fn prompts_embed_key_content() {
        let req = generation_request("rainy day jazz", 25, None);
        assert!(req.user.contains("rainy day jazz"));
        assert!(req.user.contains("exactly 25"));
        assert!(req.system.contains("STRICT JSON"));

        let taste = TasteContext {
            top_artists: vec!["Radiohead".into()],
            genres: vec!["indie rock".into()],
        };
        let req = generation_request("focus music", 10, Some(&taste));
        assert!(req.user.contains("Radiohead"));

        let refine = refinement_request(
            "My Mix",
            &[("A".into(), "T".into())],
            "more upbeat, less sad",
        );
        assert!(refine.user.contains("more upbeat, less sad"));
        assert!(refine.user.contains("1. A — T"));
    }
}
