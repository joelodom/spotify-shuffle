//! Small shared helpers: JSON extraction from model output and string
//! normalization for track matching.

/// Extract the first JSON value embedded in free-form model output.
///
/// Preference order:
/// 1. a fenced ```json ... ``` block,
/// 2. any fenced ``` ... ``` block that starts with `{` or `[`,
/// 3. the first balanced `{...}` or `[...]` region in the raw text.
pub fn extract_json(text: &str) -> Option<String> {
    if let Some(block) = fenced_block(text, "```json") {
        return Some(block);
    }
    if let Some(block) = fenced_block(text, "```") {
        let trimmed = block.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Some(block);
        }
    }
    balanced_json(text).map(|s| s.to_string())
}

fn fenced_block(text: &str, opener: &str) -> Option<String> {
    let start = text.find(opener)? + opener.len();
    let rest = &text[start..];
    // Skip the remainder of the opener line (e.g. "```json\n").
    let body_start = rest.find('\n')? + 1;
    let body = &rest[body_start..];
    let end = body.find("```")?;
    Some(body[..end].trim().to_string())
}

/// Scan for the first balanced JSON object or array, respecting strings and
/// escape sequences.
fn balanced_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find(['{', '['])?;
    let mut depth: i64 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Normalize a track title or artist name for fuzzy matching: lowercase,
/// drop parenthesized/bracketed qualifiers ("(feat. X)", "[Remastered]"),
/// drop a trailing " - ..." qualifier (e.g. "- 2011 Remaster"), strip
/// punctuation, collapse whitespace.
pub fn normalize_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut paren_depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => paren_depth += 1,
            ')' | ']' => paren_depth = paren_depth.saturating_sub(1),
            _ if paren_depth == 0 => out.push(c),
            _ => {}
        }
    }
    // Trailing version qualifiers are almost always after the LAST " - ".
    // Only strip when the tail looks like a qualifier, so legitimate " - "
    // titles survive.
    if let Some(idx) = out.rfind(" - ") {
        let tail = out[idx + 3..].to_lowercase();
        const QUALIFIERS: [&str; 10] = [
            "remaster",
            "remastered",
            "live",
            "mono",
            "stereo",
            "single version",
            "radio edit",
            "deluxe",
            "bonus",
            "edit",
        ];
        if QUALIFIERS.iter().any(|q| tail.contains(q)) {
            out.truncate(idx);
        }
    }
    let lowered = out.to_lowercase();
    let mut norm = String::with_capacity(lowered.len());
    let mut last_was_space = true;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            norm.push(c);
            last_was_space = false;
        } else if !last_was_space {
            norm.push(' ');
            last_was_space = true;
        }
    }
    norm.trim_end().to_string()
}

/// `mm:ss` / `h:mm:ss` rendering for track durations.
pub fn format_duration_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_json() {
        let text = "Here you go:\n```json\n{\"a\": 1}\n```\nEnjoy!";
        assert_eq!(extract_json(text).unwrap(), "{\"a\": 1}");
    }

    #[test]
    fn extracts_plain_fenced_block_when_json_shaped() {
        let text = "```\n[1, 2, 3]\n```";
        assert_eq!(extract_json(text).unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn extracts_balanced_object_from_prose() {
        let text = "Sure! The plan is {\"tracks\": [{\"t\": \"a } b\"}]} — hope that helps.";
        assert_eq!(
            extract_json(text).unwrap(),
            "{\"tracks\": [{\"t\": \"a } b\"}]}"
        );
    }

    #[test]
    fn handles_escaped_quotes_in_strings() {
        let text = r#"prefix {"name": "he said \"hi\" {ok}"} suffix"#;
        assert_eq!(
            extract_json(text).unwrap(),
            r#"{"name": "he said \"hi\" {ok}"}"#
        );
    }

    #[test]
    fn returns_none_without_json() {
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("unbalanced { oops"), None);
    }

    #[test]
    fn normalization_strips_qualifiers() {
        assert_eq!(
            normalize_for_match("Hey Jude - Remastered 2015"),
            "hey jude"
        );
        assert_eq!(
            normalize_for_match("Lose Yourself (From \"8 Mile\")"),
            "lose yourself"
        );
        assert_eq!(normalize_for_match("HUMBLE. [Explicit]"), "humble");
        assert_eq!(
            normalize_for_match("Don't Stop Me Now"),
            "don t stop me now"
        );
        // A legitimate " - " title without a qualifier tail survives.
        assert_eq!(
            normalize_for_match("Untitled - Part One"),
            "untitled part one"
        );
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration_ms(61_000), "1:01");
        assert_eq!(format_duration_ms(3_600_000), "1:00:00");
        assert_eq!(format_duration_ms(500), "0:00");
    }
}
