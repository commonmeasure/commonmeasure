//! Reconstructing crossings from host transcripts, for work done before
//! Common Measure was installed.
//!
//! This is the weakest evidence the store holds and it is labelled as such:
//! every crossing produced here is [`CrossingMode::Reconstructed`] and names
//! the transcript it came from. A host writes its transcript for its own
//! purposes — it can be compacted, a tool result can be truncated in it, and
//! nothing was watching at the time. A hash taken from a transcript is a claim
//! about the transcript, not about what entered the model's context.
//!
//! Only what the transcript actually evidences is imported. A host that reached
//! the network by shelling out leaves a command string, not a crossing: whether
//! the bytes arrived, and what they were, is not in the record, so nothing is
//! written. Importing those would manufacture exactly the evidence this product
//! exists to refuse.

use commonmeasure_types::canonical::sha256_digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use commonmeasure_types::LicenceState;
use serde_json::Value;

use crate::grounding;
use crate::hook::HostSurface;
use crate::session::{Crossing, CrossingMode};

/// What one host's history offers an importer.
pub struct HostHistory {
    pub host: HostSurface,
    /// Where the transcripts live.
    pub root: PathBuf,
    /// Whether crossings can be reconstructed at all, and why not when they
    /// cannot. Stated rather than left as an empty result, because "this host
    /// records nothing we can use" and "this host did no external work" are
    /// different facts and a reader must not have to guess which they got.
    pub importable: Result<(), &'static str>,
}

/// The hosts an import knows about.
///
/// Codex and Pi are listed with their reason rather than omitted, because
/// "this host's history yields nothing" and "this importer never considered
/// that host" are different facts and a reader must not have to guess which
/// they got.
pub fn known_hosts() -> Vec<HostHistory> {
    let home = std::env::var("HOME").unwrap_or_default();
    let home = Path::new(&home);
    vec![
        HostHistory {
            host: HostSurface::ClaudeCode,
            root: home.join(".claude/projects"),
            importable: Ok(()),
        },
        HostHistory {
            host: HostSurface::Codex,
            root: home.join(".codex/sessions"),
            importable: Err(
                "Codex reaches the network by shelling out, so its transcript records command \
                 text rather than crossings. A URL inside a command is not evidence that \
                 anything was retrieved, and no response survives to hash.",
            ),
        },
        HostHistory {
            host: HostSurface::Pi,
            root: home.join(".pi/agent/sessions"),
            importable: Err(
                "Pi has no web tools of its own, so its transcripts hold no observable \
                 crossings to reconstruct. The exceptions are a handful of ledger_fetch \
                 results, but those were mediated crossings, recorded at the time by the \
                 server that mediated them; reading them back from a transcript would add \
                 a weaker copy of a record that already exists.",
            ),
        },
    ]
}

/// The identity of a reconstructed crossing, for import idempotence: everything
/// observable about it. Two crossings with equal keys are the same transcript
/// fact, not two facts.
type CrossingKey = (
    DateTime<Utc>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn key_of(crossing: &Crossing) -> CrossingKey {
    (
        crossing.timestamp,
        crossing.url.clone(),
        crossing.tool.clone(),
        crossing.content_hash.clone(),
        crossing.derived_from.clone(),
        crossing.agent_id.clone(),
    )
}

fn key_of_record(payload: &Value) -> Option<CrossingKey> {
    let timestamp = payload["timestamp"]
        .as_str()
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())?
        .with_timezone(&Utc);
    let field = |name: &str| payload[name].as_str().map(str::to_owned);
    Some((
        timestamp,
        field("url")?,
        field("tool"),
        field("content_hash"),
        field("derived_from"),
        field("agent_id"),
    ))
}

/// How long an import waits for another one to finish before giving up. Long
/// enough for a full pass over a large transcript tree, short enough that a
/// wedged importer is reported rather than waited on forever.
const IMPORT_WAIT: Duration = Duration::from_secs(60);

/// Exclusive use of one home's evidence store, held across processes for as
/// long as the guard lives.
///
/// Deciding what a log is owed and appending it are one step or they are
/// nothing: two importers that both read before either writes each find the
/// log missing the same crossings and each appends them, and the log is
/// append-only by contract, so the doubled account is permanent — and a third
/// import reads the doubled multiset as already recorded and reports nothing
/// wrong. A cron import beside a manual one is enough to produce it, and an
/// in-process mutex cannot see the second console, so the hold is a file lock
/// exactly as the rule file's is.
pub struct Store {
    home: PathBuf,
    /// Released when dropped, and by the kernel if this process dies, so a
    /// crash mid-import cannot wedge the next one.
    _lock: std::fs::File,
}

impl Store {
    /// Take the home's import lock, waiting up to [`IMPORT_WAIT`] for whoever
    /// holds it.
    pub fn open(home: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(home)?;
        let path = home.join("import.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        let expires = Instant::now() + IMPORT_WAIT;
        loop {
            match lock.try_lock() {
                Ok(()) => {
                    return Ok(Self {
                        home: home.to_owned(),
                        _lock: lock,
                    });
                }
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < expires => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!(
                            "another import has held {} for over {IMPORT_WAIT:?}",
                            path.display()
                        ),
                    ));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
        }
    }

    /// Which of `crossings` this store's log for `session_id` does not already
    /// hold.
    ///
    /// `commonmeasure import` may run again — after new work, or simply twice —
    /// and a reconstructed crossing appended a second time would double the
    /// store's account of what happened. Only `crossing_reconstructed` records
    /// participate: an observed or mediated record of the same URL is a
    /// different grade of evidence and never absorbs a reconstructed one. Keys
    /// are counted as a multiset, so a crossing that genuinely happened twice
    /// imports twice.
    ///
    /// Returns the crossings still owed to the log, and how many were already
    /// there. A log line this cannot parse simply does not match anything,
    /// which errs towards a duplicate rather than towards a dropped record.
    pub fn not_yet_recorded(
        &self,
        session_id: &str,
        crossings: Vec<Crossing>,
    ) -> (Vec<Crossing>, usize) {
        let path = self
            .home
            .join("sessions")
            .join(format!("{session_id}.ndjson"));
        let mut held: HashMap<CrossingKey, usize> = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let Ok(record) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if record["event"] != "crossing_reconstructed" {
                    continue;
                }
                if let Some(key) = key_of_record(&record["payload"]) {
                    *held.entry(key).or_default() += 1;
                }
            }
        }
        let mut fresh = Vec::new();
        let mut already = 0usize;
        for crossing in crossings {
            match held.get_mut(&key_of(&crossing)) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    already += 1;
                }
                _ => fresh.push(crossing),
            }
        }
        (fresh, already)
    }
}

/// Reconstruct crossings from one Claude Code transcript.
///
/// The session identity is the identifier the live hooks would have recorded,
/// so an imported session and a later live one join up — and so the session is
/// what other stores join on. For a main transcript that is the file stem. A
/// subagent transcript lives under `<session>/subagents/agent-*.jsonl` and
/// belongs to that session: the agent file name is the subagent's own identity
/// and is carried in `agent_id`, exactly as a live hook payload carries it,
/// never as a session of its own.
pub fn from_claude_transcript(path: &Path) -> Result<Vec<Crossing>, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    let (agent_type, agent_id) = subagent_identity(path);
    let session_dir = agent_id
        .is_some()
        .then(|| path.ancestors().nth(2))
        .flatten();
    let session_id = session_dir
        .unwrap_or(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown-session".to_owned());
    let derived_from = path.display().to_string();

    // A tool result arrives in a later entry, linked by `tool_use_id`. Both
    // halves are needed: the call carries the URL, the result carries the text
    // that entered context.
    let mut pending: Vec<(String, PendingCall)> = Vec::new();
    let mut results: Vec<(String, String)> = Vec::new();

    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let timestamp = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
            .map(|stamp| stamp.with_timezone(&Utc));

        for block in blocks(&entry) {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    // An entry with no readable timestamp cannot date its
                    // crossing. Stamping it with the import's own clock would
                    // put a time no transcript recorded into the evidence, so
                    // the call is left unreconstructed instead.
                    let Some(timestamp) = timestamp else {
                        continue;
                    };
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let kind = grounding::ToolKind::classify(name);
                    if !matches!(
                        kind,
                        grounding::ToolKind::WebFetch | grounding::ToolKind::WebSearch
                    ) {
                        continue;
                    }
                    let Some(id) = block.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    pending.push((
                        id.to_owned(),
                        PendingCall {
                            tool: name.to_owned(),
                            url: block
                                .pointer("/input/url")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            timestamp,
                        },
                    ));
                }
                Some("tool_result") => {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                        results.push((id.to_owned(), grounding::ingested_text(&block["content"])));
                    }
                }
                _ => {}
            }
        }
    }

    let mut crossings = Vec::new();
    for (id, call) in pending {
        let result = results
            .iter()
            .find(|(result_id, _)| *result_id == id)
            .map(|(_, text)| text.as_str());
        // A call with no result in the transcript never demonstrably completed.
        let Some(result) = result else { continue };

        let build =
            |url: &str, hash: Option<String>, tokens: Option<u64>, grounded: bool| Crossing {
                session_id: session_id.clone(),
                timestamp: call.timestamp,
                mode: CrossingMode::Reconstructed,
                host: HostSurface::ClaudeCode.id().to_owned(),
                tool: Some(call.tool.clone()),
                agent_type: agent_type.clone(),
                agent_id: agent_id.clone(),
                turn_id: None,
                // A transcript records what the host chose to; a directory
                // read back from one is a claim, not a witnessed fact.
                cwd: None,
                url: url.to_owned(),
                host_name: grounding::host_of(url),
                // Import keeps the unconditional floor and honours no
                // prefixes, so nothing it records is internal supply.
                internal: false,
                content_hash: hash,
                retrieved_hash: None,
                estimated_tokens: tokens,
                grounded,
                licence: LicenceState::Unknown,
                refusal: None,
                policy_scope: None,
                principal: None,
                authentication_basis: None,
                breach: None,
                derived_from: Some(derived_from.clone()),
                http_status: None,
                failure: None,
                declarations: None,
                named_by: None,
                manifest_record: None,
                content_telemetry_id: None,
                supplier: None,
                identity: None,
                challenge: None,
                allowance: None,
            };

        match grounding::ToolKind::classify(&call.tool) {
            grounding::ToolKind::WebFetch => {
                let Some(url) = call.url.as_deref().filter(|url| grounding::recordable(url)) else {
                    continue;
                };
                crossings.push(build(
                    url,
                    Some(sha256_digest(result.as_bytes())),
                    Some(grounding::estimate_tokens(result)),
                    true,
                ));
            }
            // A search names pages; it does not put them in context.
            grounding::ToolKind::WebSearch => {
                for url in grounding::extract_urls(&Value::String(result.to_owned()))
                    .into_iter()
                    .filter(|url| grounding::recordable(url))
                {
                    crossings.push(build(&url, None, None, false));
                }
            }
            _ => {}
        }
    }
    Ok(crossings)
}

struct PendingCall {
    tool: String,
    url: Option<String>,
    timestamp: DateTime<Utc>,
}

/// Every Claude Code transcript under `root`, main agents and subagents alike.
pub fn claude_transcripts(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return found;
    };
    for project in projects.filter_map(Result::ok) {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                found.push(path.clone());
            }
            // `<stem>/subagents/agent-*.jsonl`, where a subagent's crossings
            // live. Attributing them to the main agent would lose exactly the
            // distinction the live hooks preserve.
            let subagents = path.join("subagents");
            if let Ok(children) = std::fs::read_dir(&subagents) {
                found.extend(
                    children
                        .filter_map(Result::ok)
                        .map(|child| child.path())
                        .filter(|child| {
                            child
                                .extension()
                                .is_some_and(|extension| extension == "jsonl")
                        }),
                );
            }
        }
    }
    found.sort();
    found
}

fn subagent_identity(path: &Path) -> (Option<String>, Option<String>) {
    let is_subagent = path
        .parent()
        .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "subagents"));
    if !is_subagent {
        return (None, None);
    }
    let id = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned());
    // The transcript names the instance but not the kind. `subagent` is what is
    // known; inventing a kind from the file name would be a guess.
    (Some("subagent".to_owned()), id)
}

fn blocks(entry: &Value) -> Vec<&Value> {
    entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn transcript(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join("0f17311c-2996-42b6-b4ef-7a7e1aa25ba5.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    /// The shapes below are taken from a real transcript in `~/.claude/projects`.
    /// One JSON object per line, exactly as the host writes them.
    #[test]
    fn a_webfetch_and_its_result_become_one_grounded_crossing() {
        let (_dir, path) = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"WebFetch","input":{"url":"https://brave.com/search/api","prompt":"pricing?"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-15T09:00:04.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Brave Search API pricing."}]}}"#,
        ]);

        let crossings = from_claude_transcript(&path).expect("parse");
        assert_eq!(crossings.len(), 1);
        let crossing = &crossings[0];
        assert_eq!(crossing.mode, CrossingMode::Reconstructed);
        assert_eq!(crossing.session_id, "0f17311c-2996-42b6-b4ef-7a7e1aa25ba5");
        assert_eq!(crossing.host_name, "brave.com");
        assert!(crossing.grounded);
        assert_eq!(
            crossing.content_hash.as_deref(),
            Some(sha256_digest(b"Brave Search API pricing.").as_str())
        );
        assert!(
            crossing
                .derived_from
                .as_deref()
                .unwrap()
                .ends_with(".jsonl"),
            "a reconstructed crossing names the transcript it came from"
        );
        assert_eq!(
            crossing.timestamp.to_rfc3339(),
            "2026-07-15T09:00:00+00:00",
            "the crossing is dated when it happened, not when it was imported"
        );
    }

    /// A call the transcript never shows completing is not a crossing. The
    /// agent may have been interrupted, or the transcript compacted.
    #[test]
    fn a_call_without_a_result_is_not_imported() {
        let (_dir, path) = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"WebFetch","input":{"url":"https://example.com/a"}}]}}"#,
        ]);
        assert!(from_claude_transcript(&path).expect("parse").is_empty());
    }

    #[test]
    fn a_websearch_yields_its_result_links_ungrounded() {
        let (_dir, path) = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_2","name":"WebSearch","input":{"query":"pricing"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-15T09:00:02.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"Links: https://a.example/x and https://b.example/y"}]}}"#,
        ]);

        let crossings = from_claude_transcript(&path).expect("parse");
        assert_eq!(crossings.len(), 2);
        assert!(
            crossings
                .iter()
                .all(|c| !c.grounded && c.content_hash.is_none()),
            "a search names pages; it does not put them in context"
        );
    }

    /// Local work is not a crossing, and neither is a private address. The Bash
    /// entry is the Codex case in miniature: a URL inside a command is not
    /// evidence that anything was retrieved.
    #[test]
    fn ordinary_tools_and_private_addresses_are_skipped() {
        let (_dir, path) = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"curl https://x.example"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-15T09:00:01.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"done"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:02.000Z","message":{"content":[{"type":"tool_use","id":"t2","name":"WebFetch","input":{"url":"http://localhost:3000/x"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-15T09:00:03.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"local"}]}}"#,
        ]);
        assert!(from_claude_transcript(&path).expect("parse").is_empty());
    }

    /// A transcript with lines this cannot read yields what it can, rather than
    /// failing the whole import.
    #[test]
    fn unreadable_lines_do_not_abort_a_transcript() {
        let (_dir, path) = transcript(&[
            "not json at all",
            r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"WebFetch","input":{"url":"https://ok.example/a"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"body"}]}}"#,
        ]);
        assert_eq!(from_claude_transcript(&path).expect("parse").len(), 1);
    }

    /// A subagent's crossings belong to the session that spawned it, exactly as
    /// a live hook records them: the host session id with the agent's own
    /// identity beside it. Filed under a session of their own they would leave
    /// the host session's account incomplete and create a session no host ever
    /// ran.
    #[test]
    fn a_subagent_transcript_joins_its_host_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session = "0f17311c-2996-42b6-b4ef-7a7e1aa25ba5";
        let subagents = dir.path().join(session).join("subagents");
        std::fs::create_dir_all(&subagents).expect("subagents dir");
        let path = subagents.join("agent-a10141dc1ff71b603.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-07-15T09:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"WebFetch","input":{"url":"https://example.com/a"}}]}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-07-15T09:00:01.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"text"}]}}"#,
                "\n",
            ),
        )
        .expect("write");

        let crossings = from_claude_transcript(&path).expect("parse");
        assert_eq!(crossings.len(), 1);
        assert_eq!(crossings[0].session_id, session);
        assert_eq!(crossings[0].agent_type.as_deref(), Some("subagent"));
        assert_eq!(
            crossings[0].agent_id.as_deref(),
            Some("agent-a10141dc1ff71b603")
        );
    }

    /// Codex and Pi are known about and explicitly not importable, which is a
    /// different statement from finding nothing.
    #[test]
    fn codex_and_pi_are_declared_unimportable_with_their_reasons() {
        let codex = known_hosts()
            .into_iter()
            .find(|entry| entry.host == HostSurface::Codex)
            .expect("codex is a known host");
        let reason = codex.importable.expect_err("codex is not importable");
        assert!(reason.contains("shelling out"), "{reason}");

        let pi = known_hosts()
            .into_iter()
            .find(|entry| entry.host == HostSurface::Pi)
            .expect("pi is a known host");
        let reason = pi.importable.expect_err("pi is not importable");
        assert!(reason.contains("ledger_fetch"), "{reason}");
    }
}
