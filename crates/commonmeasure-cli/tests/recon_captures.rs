//! The committed recon captures must honour their own redaction claims.
//!
//! `demo/recon/` holds recorded provider exchanges whose NOTES declare two
//! invariants: credential and session values never enter git, and licensed
//! text is truncated to a lead plus an elision marker. Redaction applied by
//! authoring discipline alone misses a second carrier: a machine-readable
//! duplicate can ship full licensed transcripts and live session ids while
//! the human-readable carrier beside it is redacted. This sweep reads every
//! carrier, including strings that are themselves serialised JSON.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The elision convention the captures declare (`demo/recon/redpine/NOTES.md`):
/// a short lead followed by this marker. Both spellings appear depending on
/// the serialisation level: the character itself, or its JSON escape.
const ELISION_MARKS: &[&str] = &["\u{ab}REDACTED", "\\u00abREDACTED"];

/// A mention text longer than this without an elision marker is treated as
/// untruncated licensed text. The committed redacted texts are ~223 bytes
/// (lead plus marker); unredacted ones observed were 669–899.
const MENTION_TEXT_CAP: usize = 320;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn capture_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("demo/recon is readable");
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            capture_files(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            found.push(path);
        }
    }
}

/// `mcp_` followed by a run of at least sixteen hex digits is a session id
/// as Redpine issues them. The committed captures carry the redaction marker
/// where one was issued, never the value.
fn contains_session_id(text: &str) -> Option<String> {
    for (index, _) in text.match_indices("mcp_") {
        let hex_run: String = text[index + 4..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        if hex_run.len() >= 16 {
            return Some(format!("mcp_{hex_run}"));
        }
    }
    None
}

/// Every `Bearer ` in a capture must continue into a redaction marker or an
/// environment-variable name, never a token.
fn bearer_carries_a_value(text: &str) -> Option<String> {
    for (index, _) in text.match_indices("Bearer ") {
        let rest = &text[index + "Bearer ".len()..];
        let redacted = rest.starts_with('\u{ab}')
            || rest.starts_with("\\u00ab")
            || rest.starts_with('$')
            || rest.starts_with("\\u0024");
        if !redacted {
            return Some(rest.chars().take(24).collect());
        }
    }
    None
}

/// Provider key prefixes that are unambiguous outside a hex identifier. The
/// `fc-` rule mirrors `run_contract.rs`: Firecrawl's prefix is pure hex, so
/// it also occurs inside UUIDs, and only counts when it does not continue a
/// hex run.
fn contains_key_shape(text: &str) -> Option<String> {
    for prefix in ["sk-", "tvly-"] {
        for (index, _) in text.match_indices(prefix) {
            let tail: String = text[index + prefix.len()..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if tail.len() >= 12 {
                return Some(format!("{prefix}{tail}"));
            }
        }
    }
    for (index, _) in text.match_indices("fc-") {
        let preceded_by_hex = text[..index]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_hexdigit());
        let tail: String = text[index + 3..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if !preceded_by_hex && tail.len() >= 12 {
            return Some(format!("fc-{tail}"));
        }
    }
    None
}

/// Walk a value through every carrier: object fields, array elements, and
/// strings that are themselves serialised JSON, the carrier an unredacted
/// duplicate ships in.
fn each_mention_text(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(mentions)) = map.get("mentions") {
                for mention in mentions {
                    if let Some(text) = mention.get("text").and_then(Value::as_str) {
                        texts.push(text.to_owned());
                    }
                }
            }
            for nested in map.values() {
                each_mention_text(nested, texts);
            }
        }
        Value::Array(items) => {
            for nested in items {
                each_mention_text(nested, texts);
            }
        }
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text)
                && (parsed.is_object() || parsed.is_array())
            {
                each_mention_text(&parsed, texts);
            }
        }
        _ => {}
    }
}

#[test]
fn no_capture_carries_a_session_or_credential_value() {
    let mut files = Vec::new();
    capture_files(&repo_root().join("demo/recon"), &mut files);
    assert!(
        files.len() >= 10,
        "the sweep found {} captures; demo/recon has moved and this test is scanning nothing",
        files.len()
    );
    for file in &files {
        let text = std::fs::read_to_string(file).expect("capture is readable");
        let name = file.display();
        if let Some(id) = contains_session_id(&text) {
            panic!("{name} carries session id `{id}`; the convention is the issued-by marker");
        }
        if let Some(value) = bearer_carries_a_value(&text) {
            panic!("{name} carries `Bearer {value}…`, which is credential-shaped");
        }
        if let Some(key) = contains_key_shape(&text) {
            panic!("{name} carries `{key}`, which is credential-shaped");
        }
    }
}

#[test]
fn licensed_mention_text_is_truncated_in_every_carrier() {
    let mut files = Vec::new();
    capture_files(&repo_root().join("demo/recon/redpine"), &mut files);
    let mut mentions_seen = 0;
    for file in &files {
        let text = std::fs::read_to_string(file).expect("capture is readable");
        let capture: Value = serde_json::from_str(&text).expect("capture is JSON");
        let mut texts = Vec::new();
        each_mention_text(&capture, &mut texts);
        mentions_seen += texts.len();
        for mention in texts {
            let elided = ELISION_MARKS.iter().any(|mark| mention.contains(mark));
            assert!(
                elided || mention.len() <= MENTION_TEXT_CAP,
                "{} carries an untruncated mention text ({} bytes, no elision marker): {:?}…",
                file.display(),
                mention.len(),
                &mention[..mention.len().min(80)]
            );
        }
    }
    assert!(
        mentions_seen >= 3,
        "only {mentions_seen} mention texts found across every carrier; \
         the walk no longer reaches them and this test is asserting nothing"
    );
}
