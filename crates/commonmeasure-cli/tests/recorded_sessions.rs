//! The committed real sessions (`host-sessions/`), read by the real
//! binary: the records a real Claude Code session and a real Pi session
//! left through the registrations, and the report `commonmeasure session`
//! prints over them. The figures asserted here are the ones those sessions
//! produced; a change in the reader that moves them is a change in what
//! the report says about a real session.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CLAUDE_SESSION: &str = "0e680a01-b902-4e86-85dc-4130d88ff14e";
const CLAUDE_MEDIATED_SESSION: &str = "local-1788698405319-2755715";
const PI_SESSION: &str = "01a076b1-f309-7744-9398-6b4cad94412e";
const CODEX_SESSION: &str = "local-1789354791021-86116";
const M365_SESSION: &str = "hosted-1789607934904-fa06389d36cfa7f00a6a67cff1e32859";

/// The maintainer's recorded fixtures (`COMMONMEASURE_PRIVATE_EVIDENCE`,
/// resolved by `build.rs`).
fn evidence() -> PathBuf {
    PathBuf::from(env!("COMMONMEASURE_EVIDENCE_DIR"))
}

/// A home holding copies of one recorded directory's logs.
fn home_with(directory: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    for entry in std::fs::read_dir(evidence().join("host-sessions").join(directory))
        .expect("the recorded directory exists")
    {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|e| e == "ndjson") {
            std::fs::copy(entry.path(), sessions.join(entry.file_name())).unwrap();
        }
    }
    home
}

fn session_report(home: &Path, session: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", session])
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join("sessions").join(format!("{session}.ndjson")))
        .expect("the log was copied")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("NDJSON"))
        .collect()
}

/// The real Copilot paired request shared one authenticated MCP session.
/// This reads recorded evidence; it does not replay Microsoft's transport.
#[test]
#[cfg_attr(
    not(evidence_host_sessions),
    ignore = "needs the recorded host sessions: host-sessions/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_recorded_copilot_session_preserves_admission_refusal_and_oauth_identity() {
    let home = home_with("m365-copilot");
    let recorded = records(home.path(), M365_SESSION);
    let client = &recorded
        .iter()
        .find(|r| r["event"] == "client_identified")
        .unwrap()["payload"];
    assert_eq!(client["client"]["name"], "Sydney");
    assert_eq!(client["negotiated_protocol_version"], "2025-11-25");
    let admitted: Vec<_> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_mediated")
        .collect();
    let refused: Vec<_> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_refused")
        .collect();
    assert_eq!(admitted.len(), 1);
    assert_eq!(refused.len(), 1);
    let admitted = &admitted[0]["payload"];
    let refused = &refused[0]["payload"];
    for crossing in [admitted, refused] {
        assert_eq!(crossing["host"], "m365-copilot");
        assert_eq!(crossing["session_id"], M365_SESSION);
        assert_eq!(crossing["authentication_basis"], "oauth_subject");
    }
    assert_eq!(admitted["principal"], refused["principal"]);
    assert_eq!(admitted["policy_identity"], refused["policy_identity"]);
    assert_eq!(admitted["url"], "https://commonmeasure.ai/");
    assert_eq!(admitted["http_status"], 200);
    assert_eq!(
        admitted["content_hash"],
        "sha256:a9d21dfc3be9196c00e7b533a1536fc0f2787b02d2196830e678c38f688ae9f0"
    );
    assert_eq!(refused["url"], "https://example.com/");
    assert_eq!(refused["refusal"], "The job denies host example.com.");
    assert_eq!(refused["grounded"], false);
    assert!(refused.get("content_hash").is_none());
    let report = session_report(home.path(), M365_SESSION);
    assert!(
        report.contains("crossings  0 observed, 1 mediated, 1 refused, 0 reconstructed"),
        "{report}"
    );
}

/// The Claude Code session: the hook records of four turns with a
/// compaction, and the report's comparison between the four boundaries.
#[test]
#[cfg_attr(
    not(evidence_host_sessions),
    ignore = "needs the recorded host sessions: host-sessions/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_recorded_claude_code_session_reports_four_boundaries_and_one_compaction() {
    let home = home_with("claude-code");
    let recorded = records(home.path(), CLAUDE_SESSION);
    let count = |event: &str| recorded.iter().filter(|r| r["event"] == event).count();
    assert_eq!(count("nudge_issued"), 6);
    assert_eq!(count("turn_started"), 4);
    assert_eq!(count("turn_completed"), 4);
    assert_eq!(count("crossing_observed"), 9);
    assert_eq!(count("context_snapshot"), 4);
    for record in recorded
        .iter()
        .filter(|r| r["event"] == "turn_started" || r["event"] == "turn_completed")
    {
        assert_eq!(record["payload"]["privacy_level"], "minimal");
        assert!(
            record["payload"]["turn_id"].is_string(),
            "the host supplied a prompt id on every boundary: {record}"
        );
    }
    let grounded: Vec<&str> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_observed" && r["payload"]["grounded"] == true)
        .filter_map(|r| r["payload"]["url"].as_str())
        .collect();
    assert_eq!(grounded, ["https://www.iana.org/help/example-domains"]);
    let last = &recorded
        .iter()
        .rfind(|r| r["event"] == "context_snapshot")
        .unwrap()["payload"];
    assert_eq!(last["inventory"]["compactions"], 1);
    assert_eq!(last["context_tokens"], 28557);
    assert_eq!(last["model"], "claude-fable-5-1");
    let text = std::fs::read_to_string(
        home.path()
            .join("sessions")
            .join(format!("{CLAUDE_SESSION}.ndjson")),
    )
    .unwrap();
    assert!(
        !text.contains("answer in one sentence") && !text.contains("example domains are"),
        "no prompt or answer text is in the log"
    );

    let report = session_report(home.path(), CLAUDE_SESSION);
    assert!(
        report.contains("crossings  9 observed, 0 mediated, 0 refused, 0 reconstructed"),
        "{report}"
    );
    assert!(
        report.contains("grounded   1 put page text into the model's context; 8 named a URL"),
        "{report}"
    );
    assert!(
        report.contains("4 snapshot(s) at Stop boundaries"),
        "{report}"
    );
    assert!(report.contains("compactions: 1"), "{report}");
    for line in [
        "2026-09-06T12:39:59.964Z → 2026-09-06T12:43:18.542Z: +2766 tokens API-reported",
        "host scaffolding +432 (system_prompt, instructions, skills, agents, tool_listing, tool_definitions_loaded, local_files, other_host_records), conversation +78, acquired content +1351; witnessed crossings between the boundaries ~0; remainder +905 not attributed",
        "2026-09-06T12:43:18.542Z → 2026-09-06T12:43:30.027Z: +1314 tokens API-reported",
        "remainder +326 not attributed",
        "2026-09-06T12:43:30.027Z → 2026-09-06T12:43:38.770Z: +896 tokens API-reported",
        "host scaffolding -1156",
        "remainder +2220 not attributed (1 compaction(s) rebuilt the window in this interval)",
    ] {
        assert!(report.contains(line), "expected {line:?} in\n{report}");
    }
}

/// The same conversation's mediated crossing sits in the MCP server's own
/// log, under the server's session id, because the host starts the server
/// without one.
#[test]
#[cfg_attr(
    not(evidence_host_sessions),
    ignore = "needs the recorded host sessions: host-sessions/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_recorded_claude_code_mediated_crossing_is_in_the_servers_own_log() {
    let home = home_with("claude-code");
    let recorded = records(home.path(), CLAUDE_MEDIATED_SESSION);
    let mediated: Vec<&Value> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_mediated")
        .collect();
    assert_eq!(mediated.len(), 1);
    assert_eq!(mediated[0]["payload"]["url"], "https://example.com/");
    assert_eq!(mediated[0]["payload"]["host"], "claude-code");
    assert_eq!(mediated[0]["payload"]["grounded"], true);
    assert!(mediated[0]["payload"]["principal"].is_string());
    assert_eq!(
        recorded
            .iter()
            .filter(|r| r["event"] == "processor_invoked")
            .count(),
        2
    );
    let report = session_report(home.path(), CLAUDE_MEDIATED_SESSION);
    assert!(
        report.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{report}"
    );
}

/// The Pi session: the extension passed Pi's own session id to the server,
/// so the mediated crossing is recorded as `host: pi` under that id.
#[test]
#[cfg_attr(
    not(evidence_host_sessions),
    ignore = "needs the recorded host sessions: host-sessions/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_recorded_pi_session_holds_one_mediated_crossing_under_pis_session_id() {
    let home = home_with("pi");
    let recorded = records(home.path(), PI_SESSION);
    let mediated: Vec<&Value> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_mediated")
        .collect();
    assert_eq!(mediated.len(), 1);
    assert_eq!(mediated[0]["payload"]["host"], "pi");
    assert_eq!(mediated[0]["payload"]["session_id"], PI_SESSION);
    assert_eq!(mediated[0]["payload"]["url"], "https://example.com/");
    assert_eq!(mediated[0]["payload"]["grounded"], true);
    assert!(
        recorded.iter().all(|r| r["event"] != "crossing_observed"),
        "Pi has nothing to observe"
    );
    let report = session_report(home.path(), PI_SESSION);
    assert!(
        report.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{report}"
    );
    assert!(
        report.contains("mediated   grounded  https://example.com/"),
        "{report}"
    );
}

/// The Codex CLI session: the client named itself, and the one mediated
/// crossing carries that name beside the host the registration passed.
#[test]
#[cfg_attr(
    not(evidence_host_sessions),
    ignore = "needs the recorded host sessions: host-sessions/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_recorded_codex_session_names_its_client_and_holds_one_mediated_crossing() {
    let home = home_with("codex");
    let recorded = records(home.path(), CODEX_SESSION);
    let identified: Vec<&Value> = recorded
        .iter()
        .filter(|r| r["event"] == "client_identified")
        .collect();
    assert_eq!(identified.len(), 1);
    assert_eq!(identified[0]["payload"]["host"], "codex");
    assert_eq!(
        identified[0]["payload"]["client"]["name"],
        "codex-mcp-client"
    );
    assert_eq!(identified[0]["payload"]["client"]["version"], "0.154.0");
    assert_eq!(identified[0]["payload"]["protocol_version"], "2025-06-18");
    let mediated: Vec<&Value> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_mediated")
        .collect();
    assert_eq!(mediated.len(), 1);
    assert_eq!(mediated[0]["payload"]["host"], "codex");
    assert_eq!(mediated[0]["payload"]["client"]["name"], "codex-mcp-client");
    assert_eq!(mediated[0]["payload"]["url"], "https://example.com");
    assert_eq!(mediated[0]["payload"]["grounded"], true);
    assert_eq!(mediated[0]["payload"]["http_status"], 200);
    assert!(
        recorded.iter().all(|r| r["event"] != "crossing_observed"),
        "Codex registers no hook"
    );
    let report = session_report(home.path(), CODEX_SESSION);
    assert!(
        report.contains("crossings  0 observed, 1 mediated, 0 refused, 0 reconstructed"),
        "{report}"
    );
    assert!(
        report.contains("client     codex-mcp-client 0.154.0 via host codex"),
        "{report}"
    );
}
