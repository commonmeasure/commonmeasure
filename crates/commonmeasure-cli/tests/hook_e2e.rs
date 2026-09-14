//! The observed path driven as Claude Code drives it: the hook payload on
//! stdin, the real binary, the real session log.
//!
//! The payload shapes are the ones a live host sends, including the `WebFetch`
//! wrapper whose `result` field — not the wrapper — is what enters the model's
//! context.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn hook(home: &Path, event: &str, payload: Value) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", event])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");
    writeln!(child.stdin.as_mut().unwrap(), "{payload}").unwrap();
    child.wait_with_output().expect("wait")
}

/// The same, naming the host whose payload shape is on stdin.
fn hook_as(home: &Path, host: &str, event: &str, payload: Value) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", event, "--host", host])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");
    writeln!(child.stdin.as_mut().unwrap(), "{payload}").unwrap();
    child.wait_with_output().expect("wait")
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session}.ndjson"));
    let Ok(file) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    file.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
        .collect()
}

#[test]
fn a_webfetch_is_recorded_with_the_hash_of_what_entered_context() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-1",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/energy-price-cap"},
            "tool_response": {
                "bytes": 559, "code": 200, "codeText": "OK",
                "result": "The cap is set quarterly.",
                "durationMs": 1486, "url": "https://www.ofgem.gov.uk/energy-price-cap"
            }
        }),
    );
    assert!(output.status.success());

    let recorded = records(home.path(), "s-1");
    assert_eq!(recorded.len(), 1);
    let payload = &recorded[0]["payload"];
    assert_eq!(recorded[0]["event"], "crossing_observed");
    assert_eq!(payload["mode"], "observed");
    assert_eq!(payload["grounded"], true);
    assert_eq!(payload["host_name"], "www.ofgem.gov.uk");
    assert_eq!(payload["licence"]["state"], "unknown");
    assert_eq!(
        payload["token_basis"], "characters/4",
        "an estimate must carry its basis, not pass as a tokeniser's count"
    );

    // The hash covers the text the model saw, not the transport wrapper. If it
    // covered the wrapper it could never be matched against a transcript.
    let expected = format!(
        "sha256:{:x}",
        <sha2::Sha256 as sha2::Digest>::digest("The cap is set quarterly.".as_bytes())
    );
    assert_eq!(payload["content_hash"], expected);
}

/// A crossing records the directory the session was working in — the fact the
/// sink's attribution rules resolve at read time. Sessions cross worktrees, so
/// it is on the crossing itself, not only on turn boundaries; and it is a
/// fact, not a label — no engagement name ever enters the evidence log.
#[test]
fn a_crossing_records_the_cwd_the_host_supplied_and_only_the_cwd() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-cwd",
            "hook_event_name": "PostToolUse",
            "cwd": "/home/operator/code/ozone",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.gov.uk/a"},
            "tool_response": {"result": "text"}
        }),
    );

    let recorded = records(home.path(), "s-cwd");
    assert_eq!(recorded[0]["payload"]["cwd"], "/home/operator/code/ozone");
    assert!(
        recorded[0]["payload"].get("engagement").is_none(),
        "attribution is a read-time projection, never written into evidence"
    );
    assert!(
        recorded[0]["payload"].get("policy_scope").is_none(),
        "no scope governed an observed crossing; it had already happened"
    );
}

/// A search result is retrieved, not grounded: the model saw titles and
/// snippets, and claiming the pages entered context would overstate it.
#[test]
fn a_websearch_records_each_result_and_grounds_none() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-2",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebSearch",
            "tool_input": {"query": "ofgem price cap"},
            "tool_response": {"result":
                "See https://www.ofgem.gov.uk/a and https://www.gov.uk/b and http://localhost:9/x"}
        }),
    );

    let recorded = records(home.path(), "s-2");
    assert_eq!(recorded.len(), 2, "the loopback URL must not be recorded");
    assert!(
        recorded.iter().all(|record| {
            record["payload"]["grounded"] == false && record["payload"]["content_hash"].is_null()
        }),
        "a search result is not evidence that a page entered context"
    );
}

/// The shipped binary excludes this plugin's own tools under the spelling
/// the host actually delivers. A session with the installed plugin names
/// `context_fetch` as `mcp__plugin_commonmeasure_commonmeasure__context_fetch` —
/// the plugin-managed namespace, not the direct-install `mcp__commonmeasure__`
/// spelling. Payload shape and tool name are the
/// live session's, not hand-derived; a crossing recorded here would be the
/// mediated fetch's second appearance, plus one observed row per URL the
/// response body happens to contain.
#[test]
fn the_plugin_namespaced_self_tool_is_not_observed() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-plugin-ns",
            "hook_event_name": "PostToolUse",
            "tool_name": "mcp__plugin_commonmeasure_commonmeasure__context_fetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/energy-price-cap"},
            "tool_response": {"content": [{"type": "text", "text":
                "{\"url\":\"https://www.ofgem.gov.uk/energy-price-cap\",\"content\":\"see https://elsewhere.example/x\"}"}]}
        }),
    );
    assert!(output.status.success());
    assert_eq!(
        records(home.path(), "s-plugin-ns").len(),
        0,
        "the mediated path already recorded this crossing"
    );
}

/// Several hook processes append to one session log, because a session spans
/// many of them.
#[test]
fn separate_hook_processes_append_to_one_session() {
    let home = tempfile::tempdir().expect("tempdir");
    for index in 0..3 {
        hook(
            home.path(),
            "post-tool-use",
            json!({
                "session_id": "s-3",
                "hook_event_name": "PostToolUse",
                "tool_name": "WebFetch",
                "tool_input": {"url": format!("https://example.com/{index}")},
                "tool_response": {"result": format!("page {index}")}
            }),
        );
    }
    hook(
        home.path(),
        "stop",
        json!({"session_id": "s-3", "hook_event_name": "Stop"}),
    );

    let recorded = records(home.path(), "s-3");
    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[3]["event"], "turn_completed");
    let sequences: Vec<u64> = recorded
        .iter()
        .map(|record| record["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(
        sequences,
        [1, 2, 3, 4],
        "numbering continues across processes"
    );
}

/// The privacy floor is a default, not a ceiling: a prefix the operator named
/// in `policy.json` is recorded by the real binary, and the same payload's
/// unlisted private URLs stay out. This is the internal-supply decision
/// (`docs/contracts/session-evidence.md`) driven end to end.
#[test]
fn a_named_internal_prefix_is_recorded_and_unlisted_private_space_is_not() {
    let home = tempfile::tempdir().expect("tempdir");
    let rag_result = json!({
        "session_id": "s-7",
        "hook_event_name": "PostToolUse",
        "tool_name": "mcp__corp-rag__search",
        "tool_input": {"query": "leave policy"},
        "tool_response": {"result":
            "See https://rag.corp.internal/kb/leave and https://wiki.corp.internal/private \
             and http://localhost:9/x"}
    });

    // Without a policy file the floor holds unconditionally.
    hook(home.path(), "post-tool-use", rag_result.clone());
    assert!(
        records(home.path(), "s-7").is_empty(),
        "no crossing may be recorded before the operator names the prefix"
    );

    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe",
            "record_internal_prefixes":["https://rag.corp.internal/"]}"#,
    )
    .expect("policy");
    let output = hook(home.path(), "post-tool-use", rag_result);
    assert!(output.status.success());

    let recorded = records(home.path(), "s-7");
    assert_eq!(
        recorded.len(),
        1,
        "the named corpus is recorded; the unlisted wiki and loopback are not"
    );
    assert_eq!(recorded[0]["event"], "crossing_observed");
    assert_eq!(
        recorded[0]["payload"]["url"],
        "https://rag.corp.internal/kb/leave"
    );
    assert_eq!(
        recorded[0]["payload"]["grounded"], false,
        "an MCP result under a named prefix is retrieved, not grounded"
    );
}

/// A broken policy lowers nothing: the floor is the default. The hook is the
/// one place a load error may not interrupt anybody, so it exits zero — but
/// exiting zero must not mean falling through to a policy that records the
/// internal supply nobody could read consent for.
#[test]
fn an_unreadable_policy_records_nothing_private_and_still_exits_zero() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("policy.json"), "{ not json").expect("policy");

    let output = hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-8",
            "hook_event_name": "PostToolUse",
            "tool_name": "mcp__corp-rag__search",
            "tool_input": {"query": "leave policy"},
            "tool_response": {"result":
                "See https://rag.corp.internal/kb/leave and https://wiki.corp.internal/private \
                 and http://localhost:9/x"}
        }),
    );
    assert!(
        output.status.success(),
        "a policy the hook cannot read must not break the agent"
    );
    assert!(
        records(home.path(), "s-8").is_empty(),
        "an unreadable policy is no consent at all, so the floor holds everywhere"
    );
}

/// Capture must never break the agent. Nothing the host can send makes this
/// exit non-zero, and nothing unreadable is invented into a record.
#[test]
fn no_payload_makes_the_hook_fail_the_tool_call() {
    let home = tempfile::tempdir().expect("tempdir");
    for payload in [
        json!({}),
        json!({"session_id": "s-4", "hook_event_name": "PostToolUse", "tool_name": "Bash"}),
        json!({"session_id": "s-4", "hook_event_name": "PostToolUse",
               "tool_name": "WebFetch", "tool_input": "not an object"}),
        json!("a bare string"),
    ] {
        let output = hook(home.path(), "post-tool-use", payload.clone());
        assert!(
            output.status.success(),
            "payload {payload} made the hook fail, which would break the agent"
        );
    }
    assert!(
        records(home.path(), "s-4").is_empty(),
        "nothing understood, so nothing recorded"
    );
}

/// The account a person reads. It distinguishes the two grades of evidence,
/// because only one of them could have been refused.
#[test]
fn session_reports_what_was_observed() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-5",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/cap"},
            "tool_response": {"result": "text"}
        }),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", "s-5"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("session should run");
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("1 observed, 0 mediated, 0 refused"), "{text}");
    assert!(text.contains("https://www.ofgem.gov.uk/cap"), "{text}");
    assert!(text.contains("grounded"), "{text}");
}

/// Grounding a transcript claims is not grounding this runtime witnessed, and
/// the session report must not total the two together.
#[test]
fn reconstructed_grounding_is_not_totalled_with_witnessed_grounding() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-6",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/cap"},
            "tool_response": {"result": "text"}
        }),
    );
    // A crossing a prior import reconstructed from a transcript, appended the
    // way `commonmeasure import` appends it.
    let log = home.path().join("sessions/s-6.ndjson");
    let mut existing = std::fs::read_to_string(&log).expect("the hook recorded");
    existing.push_str(
        r#"{"seq":2,"timestamp":"2026-07-15T09:00:00.000Z","event":"crossing_reconstructed","payload":{"session_id":"s-6","timestamp":"2026-07-15T09:00:00Z","mode":"reconstructed","host":"claude-code","tool":"WebFetch","url":"https://a.example/x","host_name":"a.example","content_hash":"sha256:abc","grounded":true,"licence":{"state":"unknown"},"derived_from":"transcript.jsonl"}}"#,
    );
    existing.push('\n');
    std::fs::write(&log, existing).expect("append");

    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", "s-6"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("session should run");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("1 put page text into the model's context"),
        "{text}"
    );
    assert!(
        text.contains("1 more are claimed only by transcripts"),
        "{text}"
    );
}

/// A Stop with a transcript records a context snapshot: the host's own usage
/// counters for the turn's last model call, carried with their basis and with
/// what the host does not report named unavailable. Two turns give the two
/// boundary observations the context-assembly inventory needs, and the session report shows the
/// witnessed-acquisition join without claiming it explains the host totals.
#[test]
fn a_stop_with_a_transcript_records_a_context_snapshot_with_its_basis() {
    let home = tempfile::tempdir().expect("tempdir");
    let transcript = home.path().join("transcript.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- design: canvas","skillCount":1,"names":["design"]}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta","addedNames":["WebFetch","WebSearch"],"addedLines":["WebFetch","WebSearch"],"removedNames":[]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"read the cap page"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-08-06T09:00:00.000Z","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50,"output_tokens":40}}}"#,
            "\n",
        ),
    )
    .expect("transcript");

    // One grounded fetch, so the report has witnessed acquisition to join.
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-8",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/cap"},
            "tool_response": {"result": "The cap is set quarterly."}
        }),
    );
    hook(
        home.path(),
        "stop",
        json!({
            "session_id": "s-8",
            "hook_event_name": "Stop",
            "transcript_path": transcript.display().to_string()
        }),
    );
    // A second turn: the transcript has grown, and the boundary observes it.
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- design: canvas","skillCount":1,"names":["design"]}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta","addedNames":["WebFetch","WebSearch"],"addedLines":["WebFetch","WebSearch"],"removedNames":[]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"read the cap page"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-08-06T09:00:00.000Z","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50,"output_tokens":40}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"WebFetch","input":{"url":"https://www.ofgem.gov.uk/cap"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"The cap is set quarterly."}]}}"#,
            "\n",
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_record","entries":[{"name":"WebFetch","description":"Fetches a URL"}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-08-06T09:05:00.000Z","message":{"model":"claude-fable-5","usage":{"input_tokens":3,"cache_read_input_tokens":1200,"cache_creation_input_tokens":60,"output_tokens":80}}}"#,
            "\n",
        ),
    )
    .expect("transcript grows");
    hook(
        home.path(),
        "stop",
        json!({
            "session_id": "s-8",
            "hook_event_name": "Stop",
            "transcript_path": transcript.display().to_string()
        }),
    );

    let recorded = records(home.path(), "s-8");
    let snapshots: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["event"] == "context_snapshot")
        .collect();
    assert_eq!(snapshots.len(), 2, "one snapshot per Stop boundary");
    let first = &snapshots[0]["payload"];
    assert_eq!(first["context_tokens"], 1052);
    assert_eq!(first["model"], "claude-fable-5");
    assert_eq!(first["observed_at"], "2026-08-06T09:00:00.000Z");
    assert!(
        first["basis"].as_str().unwrap().contains("usage counters"),
        "a snapshot without its basis would read as a tokeniser's measurement"
    );
    assert_eq!(first["unavailable"][0], "context_limit");
    let second = &snapshots[1]["payload"];
    assert_eq!(
        second["context_tokens"], 1263,
        "the second boundary observes the later call"
    );
    // The inventory: the host's own records by category, with the three
    // states of a capability apart. WebSearch was listed, never loaded and
    // never invoked; WebFetch was listed, loaded and invoked once.
    let inventory = &second["inventory"];
    assert_eq!(inventory["token_basis"], "characters/4");
    assert_eq!(inventory["categories"]["skills"]["records"], 1);
    assert_eq!(inventory["categories"]["tool_listing"]["records"], 1);
    assert_eq!(inventory["categories"]["acquired_content"]["records"], 1);
    assert_eq!(
        inventory["categories"]["acquired_content"]["estimated_tokens"],
        7
    );
    assert_eq!(
        inventory["available_on_demand"]["tools"],
        json!(["WebFetch", "WebSearch"])
    );
    assert_eq!(inventory["definitions_loaded"], json!(["WebFetch"]));
    assert_eq!(inventory["invoked"], json!({"WebFetch": 1}));
    assert_eq!(
        first["inventory"]["definitions_loaded"],
        json!([]),
        "nothing had been loaded at the first boundary"
    );
    let unavailable = second["unavailable"].to_string();
    assert!(
        unavailable.contains("free_capacity") && unavailable.contains("system_prompt"),
        "what no basis can see stays named: {unavailable}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", "s-8"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("session should run");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("2 snapshot(s) at Stop boundaries"), "{text}");
    assert!(
        text.contains("1263 tokens sent to claude-fable-5"),
        "{text}"
    );
    assert!(
        text.contains("unavailable at this boundary: context_limit, free_capacity"),
        "{text}"
    );
    assert!(
        text.contains("not an explanation of the host totals"),
        "{text}"
    );
    assert!(text.contains("acquired_content"), "{text}");
    assert!(
        text.contains("definitions loaded on demand: WebFetch"),
        "{text}"
    );
    // The comparison between the two boundaries: the API delta, the three
    // estimated attributions, the witnessed crossings in the interval and
    // the remainder, printed rather than absorbed.
    assert!(
        text.contains(
            "2026-08-06T09:00:00.000Z → 2026-08-06T09:05:00.000Z: +211 tokens API-reported"
        ),
        "{text}"
    );
    assert!(
        text.contains("host scaffolding +") && text.contains("acquired content +7"),
        "{text}"
    );
    assert!(
        text.contains("witnessed crossings between the boundaries ~0"),
        "the fetch was recorded before the first boundary, not between them: {text}"
    );
    assert!(text.contains("not attributed"), "{text}");
}

/// A turn boundary carries the host's own turn identifier and declares the
/// one privacy level its contents can honour: `minimal`, because the record
/// holds no question, answer, intent or summary. A crossing in the same turn
/// carries the same identifier, so a reader can group the two without the
/// question ever entering the log.
#[test]
fn a_turn_boundary_declares_minimal_privacy_and_the_hosts_turn_id() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "user-prompt-submit",
        json!({
            "session_id": "s-turn",
            "hook_event_name": "UserPromptSubmit",
            "prompt_id": "prompt-42",
            "cwd": "/home/operator/code/ozone",
            "prompt": "what is the energy price cap?"
        }),
    );
    hook(
        home.path(),
        "post-tool-use",
        json!({
            "session_id": "s-turn",
            "hook_event_name": "PostToolUse",
            "prompt_id": "prompt-42",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.ofgem.gov.uk/cap"},
            "tool_response": {"result": "The cap is set quarterly."}
        }),
    );
    hook(
        home.path(),
        "stop",
        json!({
            "session_id": "s-turn",
            "hook_event_name": "Stop",
            "prompt_id": "prompt-42",
            "last_assistant_message": "The cap is set quarterly."
        }),
    );

    let recorded = records(home.path(), "s-turn");
    // The prompt's sources are recorded before the boundary, so a mediated
    // fetch in the same turn finds them; then the boundary, the crossing
    // and the end of the turn.
    assert_eq!(recorded.len(), 4, "{recorded:?}");
    assert_eq!(recorded[0]["event"], "prompt_sources");
    let started = &recorded[1];
    assert_eq!(started["event"], "turn_started");
    assert_eq!(started["payload"]["turn_id"], "prompt-42");
    assert_eq!(started["payload"]["privacy_level"], "minimal");
    assert!(
        started["payload"]["privacy_basis"]
            .as_str()
            .unwrap()
            .contains("no query text"),
        "{started}"
    );
    assert_eq!(recorded[2]["event"], "crossing_observed");
    assert_eq!(recorded[2]["payload"]["turn_id"], "prompt-42");
    let completed = &recorded[3];
    assert_eq!(completed["event"], "turn_completed");
    assert_eq!(completed["payload"]["turn_id"], "prompt-42");
    assert_eq!(completed["payload"]["privacy_level"], "minimal");
    let text = std::fs::read_to_string(home.path().join("sessions/s-turn.ndjson")).unwrap();
    assert!(
        !text.contains("energy price cap?") && !text.contains("set quarterly.\""),
        "no prompt or answer text may enter the log: {text}"
    );
}

/// The basis a boundary records is the sentence the contract shows, character
/// for character: a literal wrapped in the source must not put its
/// indentation into every record.
#[test]
fn the_recorded_privacy_basis_is_the_contracts_sentence() {
    let contract = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the repository root")
            .join("docs/contracts/session-evidence.md"),
    )
    .expect("the contract is readable");
    let basis = commonmeasure_harness::session::TURN_PRIVACY_BASIS;
    assert!(
        !basis.contains("  "),
        "no run of spaces may enter the record: {basis:?}"
    );
    assert!(
        contract.contains(&format!("\"privacy_basis\": \"{basis}\"")),
        "the contract's example must show the basis the record carries"
    );
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "stop",
        json!({"session_id": "s-basis", "hook_event_name": "Stop"}),
    );
    assert_eq!(
        records(home.path(), "s-basis")[0]["payload"]["privacy_basis"],
        basis
    );
}

/// A host that sends no turn identifier gets a boundary without one, never
/// an invented one, and the declared level does not depend on it.
#[test]
fn a_boundary_without_a_host_turn_id_carries_none() {
    let home = tempfile::tempdir().expect("tempdir");
    hook(
        home.path(),
        "stop",
        json!({"session_id": "s-noid", "hook_event_name": "Stop"}),
    );
    let recorded = records(home.path(), "s-noid");
    assert_eq!(recorded[0]["payload"]["privacy_level"], "minimal");
    assert!(recorded[0]["payload"]["turn_id"].is_null());
}

/// The mediation nudge is standing: the plugin's SessionStart hook
/// emits it on stdout — which the host adds to the session's context — and
/// records the issuance under its versioned identity, so a later review can
/// split sessions that were asked from sessions that were not.
#[test]
fn a_session_start_emits_the_mediation_nudge_and_records_the_issuance() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook(
        home.path(),
        "session-start",
        json!({
            "session_id": "s-nudge",
            "hook_event_name": "SessionStart",
            "source": "startup"
        }),
    );
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("context_fetch"), "{text}");
    assert!(text.contains("context_search"), "{text}");
    assert!(
        text.contains("This is a nudge, not enforcement"),
        "the emitted text must not overclaim: {text}"
    );

    let recorded = records(home.path(), "s-nudge");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "nudge_issued");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["nudge"], "mediation-nudge/3");
    assert_eq!(payload["source"], "startup");
    assert_eq!(payload["host"], "claude-code");
    assert!(
        payload["basis"].as_str().unwrap().contains("not witnessed"),
        "the record claims emission, never injection: {payload}"
    );
}

/// The nudge does not depend on the payload: delivery is the point, and an
/// unreadable payload costs only the issuance record. Nothing is invented —
/// no session file appears for a payload that named no session.
#[test]
fn an_unreadable_session_start_payload_still_delivers_the_nudge() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook(home.path(), "session-start", json!("not an object"));
    assert!(
        output.status.success(),
        "a session-start hook must never break the agent"
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("context_fetch"), "{text}");
    assert!(
        !home.path().join("sessions").exists(),
        "nothing understood, so nothing recorded"
    );
}

/// The capture contract holds at session start too: a home nothing can write
/// to costs the record, never the nudge and never the exit code.
#[test]
fn an_unwritable_home_costs_the_record_never_the_nudge_or_the_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let not_a_directory = dir.path().join("home-is-a-file");
    std::fs::write(&not_a_directory, "in the way").expect("write");

    let output = hook(
        &not_a_directory,
        "session-start",
        json!({
            "session_id": "s-nudge-blocked",
            "hook_event_name": "SessionStart",
            "source": "resume"
        }),
    );
    assert!(
        output.status.success(),
        "an unwritable home must not break the agent"
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("This is a nudge, not enforcement"),
        "delivery must survive the failed record: {text}"
    );
}

/// A Stop without a transcript records the turn boundary and nothing else:
/// a missing snapshot is a missing snapshot, never zeros.
#[test]
fn a_stop_without_a_transcript_records_no_snapshot() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook(
        home.path(),
        "stop",
        json!({"session_id": "s-9", "hook_event_name": "Stop"}),
    );
    assert!(output.status.success());
    let recorded = records(home.path(), "s-9");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "turn_completed");
}

/// A session boundary names the policy a mediated crossing in that
/// directory would meet, or says the policy could not be read; an older
/// record that names neither is reported as predating the field rather than
/// as "no policy" (`docs/contracts/session-evidence.md` §Policy identity).
#[test]
fn boundaries_name_the_policy_identity_or_why_it_is_unavailable_and_older_records_are_said_to_predate_it()
 {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"strict","scopes":[{"match":"clientwork","policy_mode":"observe"}]}"#,
    )
    .expect("policy");
    let workspace = tempfile::tempdir().expect("tempdir");
    let client = workspace.path().join("clientwork");
    std::fs::create_dir_all(&client).expect("client dir");

    hook(
        home.path(),
        "session-start",
        json!({"session_id": "s-id", "hook_event_name": "SessionStart", "source": "startup",
               "cwd": client.display().to_string()}),
    );
    hook(
        home.path(),
        "stop",
        json!({"session_id": "s-id", "hook_event_name": "Stop",
               "cwd": client.display().to_string()}),
    );
    hook(
        home.path(),
        "stop",
        json!({"session_id": "s-id", "hook_event_name": "Stop",
               "cwd": workspace.path().display().to_string()}),
    );
    let recorded = records(home.path(), "s-id");
    assert_eq!(recorded.len(), 3);
    let at_start = recorded[0]["payload"]["policy_identity"]
        .as_str()
        .expect("the session start names the policy it started under");
    assert!(at_start.starts_with("sha256:"), "{at_start}");
    assert_eq!(
        recorded[1]["payload"]["policy_identity"], at_start,
        "the same directory resolves to the same identity at the turn boundary"
    );
    let outside = recorded[2]["payload"]["policy_identity"]
        .as_str()
        .expect("a boundary outside the scope still names a policy");
    assert_ne!(
        outside, at_start,
        "a directory outside the scope meets the top-level policy, a different identity"
    );

    // An older record: the same event with neither field.
    let log = home.path().join("sessions").join("s-id.ndjson");
    let mut older = std::fs::read_to_string(&log).expect("log");
    older.push_str(
        r#"{"seq":4,"event":"turn_completed","payload":{"session_id":"s-id","host":"claude-code","timestamp":"2026-01-01T00:00:00.000Z","detail":{"cwd":null,"transcript":null}}}"#,
    );
    older.push('\n');
    std::fs::write(&log, older).expect("append an older record");

    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["session", "s-id"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("session should run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("policy identity"), "{text}");
    assert!(
        text.contains(&format!("{at_start}  2 record(s)")),
        "the session names each identity with how many records met it: {text}"
    );
    assert!(
        text.contains("1 record(s) predate the field and name no policy"),
        "an older record is said to predate the field, never read as no policy: {text}"
    );

    // A policy the hook cannot read is recorded as unavailable at the
    // boundary, never as the identity of a default the operator did not write.
    std::fs::write(home.path().join("policy.json"), "{ not json").expect("policy");
    hook(
        home.path(),
        "user-prompt-submit",
        json!({"session_id": "s-id", "hook_event_name": "UserPromptSubmit"}),
    );
    let recorded = records(home.path(), "s-id");
    let boundary = &recorded[recorded.len() - 1]["payload"];
    assert!(boundary["policy_identity"].is_null(), "{boundary}");
    assert!(
        boundary["policy_unavailable"]
            .as_str()
            .is_some_and(|reason| reason.contains("not a valid policy")),
        "{boundary}"
    );
}

/// The prompt-submission hook: which URLs the user named, as hashes, and the
/// statements pasted material carries, read by the real binary.
mod prompt_sources {
    use super::*;

    fn submit(home: &Path, session: &str, prompt: &str) -> Value {
        let output = hook(
            home,
            "user-prompt-submit",
            json!({
                "session_id": session,
                "hook_event_name": "UserPromptSubmit",
                "cwd": "/home/operator/code",
                "prompt": prompt,
            }),
        );
        assert!(
            output.status.success(),
            "the hook exits zero whatever happens"
        );
        records(home, session)
            .into_iter()
            .find(|record| record["event"] == "prompt_sources")
            .expect("a prompt_sources record")
    }

    fn sha256(text: &str) -> String {
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
        )
    }

    /// Every URL in the prompt is recorded as a hash with its basis, and no
    /// prompt text reaches the log. Plain text with nothing embedded is
    /// recorded as carrying no embedded statement.
    #[test]
    fn a_prompt_records_url_hashes_and_no_text_and_plain_text_records_no_statement() {
        let home = tempfile::tempdir().expect("tempdir");
        let record = submit(
            home.path(),
            "s-prompt",
            "Compare https://www.gov.uk/guidance/x with https://example.org/report. Thanks!",
        );
        let payload = &record["payload"];
        let hashes: Vec<&str> = payload["url_hashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h.as_str().unwrap())
            .collect();
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains(&sha256("https://www.gov.uk/guidance/x").as_str()));
        assert!(hashes.contains(&sha256("https://example.org/report").as_str()));
        assert_eq!(
            payload["url_hash_basis"],
            "sha256 over the URL text as written in the prompt, trailing punctuation removed"
        );
        assert_eq!(payload["embedded"], "no embedded statement");
        assert!(payload.get("statements").is_none());
        let log = std::fs::read_to_string(home.path().join("sessions/s-prompt.ndjson")).unwrap();
        assert!(
            !log.contains("gov.uk") && !log.contains("Compare"),
            "no URL text and no prompt text enters the record"
        );
        assert!(
            records(home.path(), "s-prompt")
                .iter()
                .any(|record| record["event"] == "turn_started"),
            "the turn boundary is still recorded"
        );
    }

    /// A pasted page with an inline RSL licence and a pasted response with a
    /// Content-Usage line each yield a statement naming its source.
    #[test]
    fn pasted_html_with_an_rsl_script_and_a_pasted_content_usage_line_are_read() {
        let home = tempfile::tempdir().expect("tempdir");
        let record = submit(
            home.path(),
            "s-paste",
            "Summarise what I pasted:\n\
             <html><head><script type=\"application/rsl+xml\">\n\
             <rsl xmlns=\"https://rslstandard.org/rsl\"><content url=\"\"><license>\n\
             <permits type=\"usage\">search</permits></license></content></rsl>\n\
             </script></head><body>Article.</body></html>\n\
             \n\
             HTTP/1.1 200 OK\n\
             Content-Type: text/html\n\
             Content-Usage: train-ai=n, search=y\n",
        );
        let payload = &record["payload"];
        assert_eq!(payload["embedded"], "statements read");
        let statements = payload["statements"].as_array().unwrap();
        assert!(
            statements.iter().any(|s| s["source"] == "pasted-rsl"
                && s["category"] == "ai-input"
                && s["preference"] == "disallow"),
            "a licence permitting search alone disallows AI input: {statements:?}"
        );
        assert!(
            statements
                .iter()
                .any(|s| s["source"] == "pasted-content-usage"
                    && s["category"] == "train-ai"
                    && s["preference"] == "disallow"),
            "{statements:?}"
        );
        assert!(
            payload["not_scanned"][0]
                .as_str()
                .unwrap()
                .starts_with("images:"),
            "what was not scanned is stated"
        );
    }

    /// A C2PA manifest embedded in pasted text under Annex A.8, signed with
    /// an ephemeral certificate, is read with the SDK: the
    /// training-and-data-mining entries become statements, the
    /// `constraint_info` URL is recorded as a reference to follow, and the
    /// validation state is recorded as the SDK reports it.
    #[test]
    fn a_c2pa_manifest_embedded_in_pasted_text_is_read_with_its_training_entries() {
        let text = "The quarterly figures were revised upward in March.";
        let mut builder = c2pa::Builder::from_context(c2pa::Context::new())
            .with_definition(
                json!({
                "claim_generator_info": [{"name": "commonmeasure-test", "version": "0"}],
                "assertions": [{
                    "label": "cawg.training-mining",
                    "data": {"entries": {
                        "cawg.ai_inference": {"use": "notAllowed"},
                        "cawg.ai_generative_training": {
                            "use": "constrained",
                            "constraint_info": "https://publisher.example/license.xml"
                        },
                        "cawg.data_mining": {"use": "allowed"}
                    }}
                }]
                })
                .to_string()
                .as_str(),
            )
            .expect("a manifest definition");
        let signer = c2pa::EphemeralSigner::new("commonmeasure-test.local").expect("a signer");
        // The data-hashed workflow: a placeholder puts the hash-binding
        // assertion in the definition, then the hash over the text is signed
        // in. The wrapper is appended after the text, so nothing is excluded.
        builder
            .data_hashed_placeholder(c2pa::Signer::reserve_size(&signer), "application/c2pa")
            .expect("a placeholder");
        let mut data_hash = c2pa::assertions::DataHash::new("jumbf manifest", "sha256");
        data_hash
            .gen_hash_from_stream(&mut std::io::Cursor::new(text.as_bytes()))
            .expect("hash the text");
        let manifest = builder
            .sign_data_hashed_embeddable(&signer, &data_hash, "application/c2pa")
            .expect("a signed manifest store");
        let pasted = c2pa_text::embed_manifest(text, &manifest);
        assert!(
            pasted.starts_with(text) && pasted.len() > text.len(),
            "the wrapper is invisible and appended"
        );

        let home = tempfile::tempdir().expect("tempdir");
        let record = submit(
            home.path(),
            "s-c2pa",
            &format!("What does this say?\n{pasted}"),
        );
        let payload = &record["payload"];
        assert_eq!(payload["embedded"], "statements read");
        let c2pa = &payload["c2pa"];
        assert_eq!(c2pa["method"], "annex-a8");
        assert!(c2pa["error"].is_null(), "the store reads: {c2pa}");
        assert_eq!(c2pa["claim_generator"], "commonmeasure-test/0");
        assert!(
            matches!(c2pa["validation_state"].as_str(), Some("valid" | "invalid")),
            "an ephemeral certificate is never trusted: {c2pa}"
        );
        assert_eq!(c2pa["training_mining"]["cawg.ai_inference"], "notAllowed");
        assert_eq!(c2pa["training_mining"]["cawg.data_mining"], "allowed");
        let statements = payload["statements"].as_array().unwrap();
        assert!(
            statements.iter().any(|s| s["source"] == "pasted-c2pa"
                && s["category"] == "ai-input"
                && s["preference"] == "disallow"),
            "{statements:?}"
        );
        assert!(
            statements.iter().any(|s| s["source"] == "pasted-c2pa"
                && s["category"] == "train-ai"
                && s["preference"] == "disallow"),
            "constrained is treated as notAllowed: {statements:?}"
        );
        let references = payload["references"].as_array().unwrap();
        assert!(
            references.iter().any(|r| r["kind"] == "constraint-info"
                && r["href"] == "https://publisher.example/license.xml"),
            "{references:?}"
        );
        let log = std::fs::read_to_string(home.path().join("sessions/s-c2pa.ndjson")).unwrap();
        assert!(
            !log.contains("quarterly figures"),
            "no pasted text enters the record"
        );

        // The same store carried inline in pasted HTML under Annex A.7 is
        // read the same way, and the record names the method.
        let html = format!(
            "<html><head>{}</head><body><p>{text}</p></body></html>",
            c2pa_text::html::build_html_script(&manifest)
        );
        let home = tempfile::tempdir().expect("tempdir");
        let record = submit(
            home.path(),
            "s-c2pa-html",
            &format!("And this page:\n{html}"),
        );
        let c2pa = &record["payload"]["c2pa"];
        assert_eq!(c2pa["method"], "annex-a7-script", "{c2pa}");
        assert!(c2pa["error"].is_null(), "{c2pa}");
        assert_eq!(c2pa["training_mining"]["cawg.ai_inference"], "notAllowed");
        assert_eq!(record["payload"]["embedded"], "statements read");
    }
}

/// A Cursor `postToolUse` payload, in Cursor's shape: the session is its
/// `conversation_id`, the turn its `generation_id`, the output JSON text.
/// A third-party MCP result is recorded as retrieved, our own tools under
/// Cursor's `MCP:` spelling are not, and Cursor's stdout is JSON.
#[test]
fn a_cursor_post_tool_use_records_under_the_conversation_id_and_answers_json() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook_as(
        home.path(),
        "cursor",
        "post-tool-use",
        json!({
            "conversation_id": "conv-42", "generation_id": "gen-3",
            "hook_event_name": "postToolUse", "cursor_version": "3.20.17",
            "workspace_roots": ["/work/project"], "cwd": "/work/project",
            "tool_name": "MCP:fetch_page",
            "tool_input": "{\"url\":\"https://www.example.org/report\"}",
            "tool_output": "{\"content\":[{\"type\":\"text\",\"text\":\"https://www.example.org/report says so\"}]}",
            "tool_use_id": "t-1", "duration": 40
        }),
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    let recorded = records(home.path(), "conv-42");
    let crossings: Vec<&Value> = recorded
        .iter()
        .filter(|r| r["event"] == "crossing_observed")
        .collect();
    assert_eq!(crossings.len(), 1, "{recorded:?}");
    let payload = &crossings[0]["payload"];
    assert_eq!(payload["host"], "cursor");
    assert_eq!(payload["session_id"], "conv-42");
    assert_eq!(payload["turn_id"], "gen-3");
    assert_eq!(payload["cwd"], "/work/project");
    assert_eq!(payload["tool"], "MCP:fetch_page");
    assert_eq!(payload["url"], "https://www.example.org/report");
    assert_eq!(payload["grounded"], false);

    let output = hook_as(
        home.path(),
        "cursor",
        "post-tool-use",
        json!({
            "conversation_id": "conv-42", "hook_event_name": "postToolUse",
            "tool_name": "MCP:context_fetch",
            "tool_input": {"url": "https://www.example.org/other"},
            "tool_output": "{\"url\":\"https://www.example.org/other\"}"
        }),
    );
    assert!(output.status.success());
    assert_eq!(
        records(home.path(), "conv-42")
            .iter()
            .filter(|r| r["event"] == "crossing_observed")
            .count(),
        1,
        "our own tool is not observed a second time"
    );
}

/// Cursor's session start receives the nudge as `additional_context` and
/// records the issuance; its prompt hook answers `continue` and leaves the
/// prompt's sources and a turn boundary; its stop leaves the other boundary.
#[test]
fn a_cursor_session_is_nudged_and_bounded_in_cursors_own_shapes() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = hook_as(
        home.path(),
        "cursor",
        "session-start",
        json!({"session_id": "conv-9", "conversation_id": "conv-9",
               "hook_event_name": "sessionStart", "is_background_agent": false,
               "composer_mode": "agent", "workspace_roots": ["/work/project"]}),
    );
    assert!(output.status.success());
    let answer: Value = serde_json::from_slice(&output.stdout).expect("JSON for Cursor");
    assert!(
        answer["additional_context"]
            .as_str()
            .is_some_and(|text| text.contains("context_fetch")),
        "{answer}"
    );
    let output = hook_as(
        home.path(),
        "cursor",
        "user-prompt-submit",
        json!({"conversation_id": "conv-9", "generation_id": "gen-1",
               "hook_event_name": "beforeSubmitPrompt",
               "prompt": "read https://www.example.org/report", "attachments": []}),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        r#"{"continue":true}"#
    );
    let output = hook_as(
        home.path(),
        "cursor",
        "stop",
        json!({"conversation_id": "conv-9", "generation_id": "gen-1",
               "hook_event_name": "stop", "status": "completed", "loop_count": 0}),
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    let events: Vec<String> = records(home.path(), "conv-9")
        .iter()
        .map(|r| r["event"].as_str().unwrap_or_default().to_owned())
        .collect();
    for expected in [
        "nudge_issued",
        "prompt_sources",
        "turn_started",
        "turn_completed",
    ] {
        assert!(events.contains(&expected.to_owned()), "{events:?}");
    }
    let boundary = records(home.path(), "conv-9")
        .into_iter()
        .find(|r| r["event"] == "turn_started")
        .unwrap();
    assert_eq!(boundary["payload"]["host"], "cursor");
    assert_eq!(boundary["payload"]["turn_id"], "gen-1");
}

/// Cursor and VS Code load Claude Code's hook file and run its commands with
/// their own payloads. A reader told it is reading Claude Code refuses those
/// shapes: exit zero, nothing recorded, nothing printed.
#[test]
fn the_claude_code_reader_refuses_another_hosts_payload_shape() {
    let home = tempfile::tempdir().expect("tempdir");
    let cursor = json!({
        "conversation_id": "conv-7", "generation_id": "gen-2", "hook_event_name": "postToolUse",
        "cursor_version": "3.20.17", "tool_name": "Shell",
        "tool_input": {"command": "curl https://www.example.org/"},
        "tool_output": "{\"exitCode\":0,\"stdout\":\"https://www.example.org/\"}"
    });
    let vscode = json!({
        "sessionId": "vs-1", "timestamp": 1704614400000i64, "cwd": "/work",
        "toolName": "web_fetch", "toolArgs": {"url": "https://www.example.org/"},
        "toolResult": {"resultType": "success", "textResultForLlm": "https://www.example.org/"}
    });
    for payload in [cursor.clone(), vscode] {
        let output = hook_as(home.path(), "claude-code", "post-tool-use", payload);
        assert!(output.status.success(), "the tool call is never failed");
        assert!(output.stdout.is_empty(), "Claude Code reads nothing here");
    }
    // A refused payload at session start gets no nudge either: plain text
    // on stdout would land in Cursor's JSON reader.
    let output = hook_as(home.path(), "claude-code", "session-start", cursor);
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "no nudge for a refused payload: {:?}",
        output.stdout
    );
    // Cursor's documentation does not say which shape the Claude Code hooks
    // it loads receive, so Cursor's environment alone is a refusal: a
    // Claude-shaped payload under CURSOR_PROJECT_DIR leaves no record and
    // no nudge.
    let claude_shaped = json!({
        "session_id": "s-under-cursor", "hook_event_name": "PostToolUse", "cwd": "/work",
        "tool_name": "WebFetch", "tool_input": {"url": "https://www.example.org/"},
        "tool_response": {"result": "Enough page text to count as grounded."}
    });
    for event in ["post-tool-use", "session-start"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["hook", event, "--host", "claude-code"])
            .env("COMMONMEASURE_HOME", home.path())
            .env("CURSOR_PROJECT_DIR", "/work")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary should start");
        writeln!(child.stdin.as_mut().unwrap(), "{claude_shaped}").unwrap();
        let output = child.wait_with_output().expect("wait");
        assert!(output.status.success());
        assert!(output.stdout.is_empty(), "{event}: {:?}", output.stdout);
    }
    assert!(
        !home.path().join("sessions/s-under-cursor.ndjson").exists(),
        "a Claude-shaped payload under Cursor's environment left a record"
    );
    assert!(
        !home.path().join("sessions").exists()
            || std::fs::read_dir(home.path().join("sessions"))
                .unwrap()
                .next()
                .is_none(),
        "a foreign payload must leave no record"
    );
}
