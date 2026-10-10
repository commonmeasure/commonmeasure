//! `commonmeasure session` over the two logs of one host session: the hook
//! log under the host's identifier and the MCP log under a minted one,
//! joined on the `host_process` pair and on nothing else
//! (`docs/contracts/session-evidence.md` §Host process).
//!
//! The logs are fixtures written here in the shape the binary writes; what
//! is under test is the real binary's joined read. That the binary writes
//! the records is `hook_e2e.rs` and `mediated_e2e.rs`.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const HOOK_LOG: &str = "8195e8c6-94a2-4f8d-b6ca-545246a50ea6";
const MCP_LOG: &str = "local-1789686515887-94668";
const REUSED_PID_LOG: &str = "local-1789772400000-94700";
const BARE_LOG: &str = "local-1789600000000-100";

fn write_log(home: &Path, session: &str, records: &[(&str, &str, Value)]) {
    let directory = home.join("sessions");
    std::fs::create_dir_all(&directory).unwrap();
    let text: String = records
        .iter()
        .enumerate()
        .map(|(index, (timestamp, event, payload))| {
            let mut payload = payload.clone();
            payload["session_id"] = json!(session);
            payload["timestamp"] = json!(timestamp);
            format!(
                "{}\n",
                json!({"seq": index + 1, "timestamp": timestamp, "event": event, "payload": payload})
            )
        })
        .collect();
    std::fs::write(directory.join(format!("{session}.ndjson")), text).unwrap();
}

fn host_process(path: &str, started_at: &str) -> Value {
    json!({
        "host": "claude-code", "path": path, "writer_pid": 99796,
        "host_process": {"pid": 94522, "started_at": started_at, "command": "claude"},
        "basis": "the first ancestor of the writing process that is not a shell or launcher wrapper"
    })
}

fn snapshot(observed_at: &str, context_tokens: u64) -> Value {
    json!({
        "host": "claude-code", "basis": "api_reported", "observed_at": observed_at,
        "model": "claude-fable-5-1", "context_tokens": context_tokens,
        "input_tokens": 10, "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0, "output_tokens": 5,
        "unavailable": [],
        "inventory": {"categories": {"conversation": {"records": 2, "estimated_chars": 400}},
                      "compactions": 0}
    })
}

fn mediated(url: &str, estimated_tokens: u64) -> Value {
    json!({
        "host": "claude-code", "mode": "mediated", "url": url, "host_name": "www.example.org",
        "content_hash": "sha256:00", "estimated_tokens": estimated_tokens,
        "token_basis": "characters/4", "grounded": true, "licence": {"state": "unknown"}
    })
}

/// One host session as Claude Code leaves it: turns, snapshots and an
/// observed crossing in the hook log; one mediated crossing, made between
/// the two snapshots, in the MCP log. Beside them a log whose record names
/// the same pid with another start time (a reused number) and holds a
/// mediated crossing of its own, and a log from before the record existed.
fn fixture(home: &Path) {
    let started = "2026-09-17T22:55:48Z";
    write_log(
        home,
        HOOK_LOG,
        &[
            (
                "2026-09-17T22:55:49.100Z",
                "host_process",
                host_process("hook", started),
            ),
            (
                "2026-09-17T22:56:00.000Z",
                "context_snapshot",
                snapshot("2026-09-17T22:56:00Z", 1000),
            ),
            (
                "2026-09-17T22:57:00.000Z",
                "crossing_observed",
                json!({"host": "claude-code", "mode": "observed", "tool": "WebFetch",
                       "url": "https://www.example.org/observed", "host_name": "www.example.org",
                       "content_hash": "sha256:11", "estimated_tokens": 12,
                       "token_basis": "characters/4", "grounded": true,
                       "licence": {"state": "unknown"}}),
            ),
            (
                "2026-09-17T22:59:00.000Z",
                "context_snapshot",
                snapshot("2026-09-17T22:59:00Z", 1100),
            ),
            (
                "2026-09-17T23:10:00.000Z",
                "session_ended",
                json!({"host": "claude-code", "reason": "clear"}),
            ),
        ],
    );
    write_log(
        home,
        MCP_LOG,
        &[
            (
                "2026-09-17T22:55:49.300Z",
                "host_process",
                host_process("mcp", started),
            ),
            (
                "2026-09-17T22:58:00.000Z",
                "crossing_mediated",
                mediated("https://www.example.org/mediated", 40),
            ),
        ],
    );
    write_log(
        home,
        REUSED_PID_LOG,
        &[
            (
                "2026-09-18T09:01:03.000Z",
                "host_process",
                host_process("mcp", "2026-09-18T09:01:02Z"),
            ),
            (
                "2026-09-18T09:02:00.000Z",
                "crossing_mediated",
                mediated("https://www.example.org/another-process", 7000),
            ),
        ],
    );
    write_log(
        home,
        BARE_LOG,
        &[(
            "2026-09-10T09:02:00.000Z",
            "crossing_mediated",
            mediated("https://www.example.org/older", 9),
        )],
    );
}

fn session(home: &Path, id: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", id])
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Given the hook log's identifier, the command reads the MCP log too, says
/// which log it joined and on what, counts the mediated crossing, and puts
/// it between the two snapshots it was made between.
#[test]
fn the_hook_logs_identifier_reads_the_mediated_crossings_of_the_joined_log() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let text = session(home.path(), HOOK_LOG);
    assert!(
        text.contains(&format!(
            "{MCP_LOG}.ndjson on host process claude (pid 94522, started 2026-09-17T22:55:48Z)"
        )),
        "{text}"
    );
    assert!(
        text.contains("crossings  1 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{text}"
    );
    assert!(
        text.contains("acquired content witnessed by crossings: ~52 tokens"),
        "{text}"
    );
    assert!(
        text.contains("witnessed crossings between them ~52"),
        "the mediated crossing falls between the two boundaries by timestamp: {text}"
    );
    assert!(text.contains("https://www.example.org/mediated"), "{text}");
    assert!(
        !text.contains("another-process") && !text.contains("/older"),
        "a reused pid and a log without the record join nothing: {text}"
    );
}

/// Either identifier of the pair reads both logs.
#[test]
fn the_mcp_logs_identifier_reads_the_same_pair() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let text = session(home.path(), MCP_LOG);
    assert!(
        text.contains(&format!(
            "joined     {}",
            home.path().join("sessions").display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("{HOOK_LOG}.ndjson on host process claude")),
        "{text}"
    );
    assert!(
        text.contains("crossings  1 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{text}"
    );
}

/// The same pid under another start time is another process: the log is
/// read alone and the command says no other log names its process.
#[test]
fn a_reused_pid_is_read_alone() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let text = session(home.path(), REUSED_PID_LOG);
    assert!(
        text.contains(
            "joined     nothing: no other log names host process claude (pid 94522, started \
             2026-09-18T09:01:02Z)"
        ),
        "{text}"
    );
    assert!(
        text.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{text}"
    );
}

/// A log from before the record existed joins nothing, and the command
/// says the join is unavailable; it is never grouped by directory or time.
#[test]
fn a_log_without_the_record_says_the_join_is_unavailable() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let text = session(home.path(), BARE_LOG);
    assert!(
        text.contains(
            "joined     nothing: this log names no host process, so the join is unavailable"
        ),
        "{text}"
    );
    assert!(
        text.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{text}"
    );
}

fn session_json(home: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session"])
        .args(args)
        .arg("--json")
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The JSON document carries the counts the report states, over both logs,
/// and the sources in the provenance line's vocabulary: the Claude Code
/// mod's sources line reads these, so its counts are the report's.
#[test]
fn the_json_counts_are_the_reports_and_name_each_source() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let document = session_json(home.path(), &[HOOK_LOG]);
    assert_eq!(
        document["joined"],
        json!([home
            .path()
            .join("sessions")
            .join(format!("{MCP_LOG}.ndjson"))
            .display()
            .to_string()])
    );
    let summary = &document["summary"];
    assert_eq!(
        (summary["observed"].as_u64(), summary["mediated"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(summary["refused"], 0);
    assert_eq!(summary["grounded_witnessed"], 2);
    let sources = &document["sources"];
    let crossings = sources["crossings"].as_array().unwrap();
    assert_eq!(crossings.len(), 2, "{sources:#}");
    assert_eq!(crossings[0]["grade"], "observed");
    assert_eq!(crossings[0]["ruling"], "not ruled");
    assert_eq!(crossings[0]["host"], "www.example.org");
    assert_eq!(crossings[0]["url"], "https://www.example.org/observed");
    assert_eq!(crossings[1]["grade"], "mediated");
    // The fixture's mediated record carries no delivery, so the edge's own
    // word for it is the line's: failed, and its host is withheld.
    assert_eq!(crossings[1]["ruling"], "failed");
    assert_eq!(crossings[1]["host"], "withheld");
    // Neither record holds declarations, so neither cost is known, and the
    // total is unknown rather than zero.
    assert_eq!(crossings[0]["cost"], "unknown");
    assert_eq!(sources["cost"], "unknown");
    assert_eq!(sources["receipts_owed"], 0);
}

/// `--since` keeps the records at or after the time in every joined log:
/// the turn the mediated crossing was made in, and nothing before it.
#[test]
fn since_keeps_one_window_across_the_joined_logs() {
    let home = tempfile::tempdir().expect("tempdir");
    fixture(home.path());
    let document = session_json(home.path(), &[HOOK_LOG, "--since", "2026-09-17T22:57:30Z"]);
    assert_eq!(document["since"], "2026-09-17T22:57:30.000Z");
    assert_eq!(document["summary"]["observed"], 0);
    assert_eq!(document["summary"]["mediated"], 1);
    assert_eq!(
        document["sources"]["crossings"][0]["url"],
        "https://www.example.org/mediated"
    );
    let text = {
        let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["session", HOOK_LOG, "--since", "2026-09-17T22:57:30Z"])
            .env("COMMONMEASURE_HOME", home.path())
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(
        text.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{text}"
    );
    let after = session_json(home.path(), &[HOOK_LOG, "--since", "2026-09-18T00:00:00Z"]);
    assert_eq!(after["sources"]["cost"], "none");
    assert_eq!(after["sources"]["crossings"], json!([]));

    let refused = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", HOOK_LOG, "--since", "yesterday"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("is not an RFC 3339 time"));
}
