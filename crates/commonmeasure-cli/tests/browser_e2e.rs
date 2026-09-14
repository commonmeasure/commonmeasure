//! The browser surfaces driven as Chrome drives them: the extension's answer
//! messages in native messaging framing on stdin, the real binary, the real
//! session log.
//!
//! The ChatGPT message is `browser/test/fixtures/chatgpt-search.message.json`,
//! the file the extension's own tests assert its parser produces, so the
//! message the extension builds and the message the binary reads are one file.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const CHATGPT_MESSAGE: &str =
    include_str!("../../../browser/test/fixtures/chatgpt-search.message.json");

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut framed = u32::try_from(body.len()).unwrap().to_ne_bytes().to_vec();
    framed.extend_from_slice(body.as_bytes());
    framed
}

/// Start the binary with `arguments`, write `input` to its stdin and close it,
/// as Chrome does after its last message.
fn run(home: &Path, arguments: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(arguments)
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    child.stdin.as_mut().unwrap().write_all(input).unwrap();
    drop(child.stdin.take());
    child.wait_with_output().expect("wait")
}

/// The framed replies on stdout, each parsed; anything else on stdout would
/// corrupt Chrome's reading of the protocol, so trailing bytes fail the test.
fn replies(output: &Output) -> Vec<Value> {
    let mut rest = output.stdout.as_slice();
    let mut found = Vec::new();
    while !rest.is_empty() {
        let (length, body) = rest.split_at(4);
        let length = u32::from_ne_bytes(length.try_into().unwrap()) as usize;
        found.push(serde_json::from_slice(&body[..length]).expect("each reply is JSON"));
        rest = &body[length..];
    }
    found
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session}.ndjson"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON"))
        .collect()
}

/// Chrome starts the host with the extension's origin as the only argument.
/// A ChatGPT answer is recorded under the conversation id as one observed
/// crossing per source, retrieved and never grounded, with no hash, no tool
/// and no working directory; the status request is answered in the same
/// process.
#[test]
fn a_chatgpt_answer_is_recorded_as_observed_retrieved_crossings_under_its_conversation() {
    let home = tempfile::tempdir().expect("tempdir");
    let message: Value = serde_json::from_str(CHATGPT_MESSAGE).unwrap();
    let mut input = frame(&message);
    input.extend(frame(&json!({"status": true})));
    let output = run(
        home.path(),
        &["chrome-extension://hojjbnoeobjkjklcdhhnncmmojmcneig/"],
        &input,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = replies(&output);
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0], json!({"recorded": 4, "session": "conv-abc"}));
    assert_eq!(replies[1]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        replies[1]["recording"]
            .as_str()
            .is_some_and(|line| line.contains("is writable")),
        "{:?}",
        replies[1]
    );

    let recorded = records(home.path(), "conv-abc");
    assert_eq!(
        recorded
            .iter()
            .map(|record| record["payload"]["url"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "https://www.aljazeera.com/news/2026/6/8/iran/",
            "https://www.axios.com/2026/06/07/iran/",
            "https://www.reuters.com/world/deal/",
            "https://www.theguardian.com/world/live/",
        ]
    );
    for record in &recorded {
        let payload = &record["payload"];
        assert_eq!(record["event"], "crossing_observed");
        assert_eq!(payload["mode"], "observed");
        assert_eq!(payload["host"], "chatgpt-web");
        assert_eq!(payload["grounded"], false);
        assert_eq!(payload["licence"]["state"], "unknown");
        for absent in [
            "content_hash",
            "estimated_tokens",
            "tool",
            "cwd",
            "turn_id",
            "policy_identity",
        ] {
            assert!(payload.get(absent).is_none(), "{absent} in {payload}");
        }
    }
}

/// Google and Bing give no session identity, so each answer is a session the
/// binary names; a private address in the answer stays out of the record.
#[test]
fn a_search_answer_without_a_session_gets_a_generated_one_and_the_floor_holds() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(
        home.path(),
        &["native-host"],
        &frame(&json!({
            "host": "google-ai-overview",
            "retrieved": ["https://www.gov.uk/a", "http://10.0.0.7/intranet"],
            "cited": ["https://www.gov.uk/a"]
        })),
    );
    assert!(output.status.success());
    let reply = &replies(&output)[0];
    assert_eq!(reply["recorded"], 1);
    let session = reply["session"].as_str().expect("a session");
    assert!(session.starts_with("local-"), "{session}");
    let recorded = records(home.path(), session);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["payload"]["host"], "google-ai-overview");
    assert_eq!(recorded[0]["payload"]["url"], "https://www.gov.uk/a");
}

/// A message that cannot be recorded is answered with the reason, the next
/// message is still served, and nothing is written outside the sessions
/// directory: a session id is a file name, and a page controls it.
#[test]
fn an_unrecordable_message_is_answered_with_the_reason_and_writes_nothing() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut input = frame(&json!({
        "host": "chatgpt-web", "session": "../escaped", "retrieved": ["https://www.gov.uk/a"]
    }));
    input.extend(frame(
        &json!({"host": "claude-code", "retrieved": ["https://www.gov.uk/a"]}),
    ));
    input.extend(frame(
        &json!({"host": "bing-copilot-search", "retrieved": "not a list"}),
    ));
    input.extend({
        let mut raw = 5u32.to_ne_bytes().to_vec();
        raw.extend_from_slice(b"{nope");
        raw
    });
    input.extend(frame(&json!({
        "host": "bing-copilot-search", "session": "s-bing", "retrieved": ["https://www.gov.uk/b"]
    })));
    let output = run(home.path(), &["native-host"], &input);
    assert!(output.status.success());
    let replies = replies(&output);
    assert_eq!(replies.len(), 5, "{replies:?}");
    for (reply, reason) in replies[..4].iter().zip([
        "not a plain identifier",
        "not a browser surface",
        "not an answer",
        "not JSON",
    ]) {
        assert_eq!(reply["recorded"], 0);
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|error| error.contains(reason)),
            "{reply}"
        );
    }
    assert_eq!(replies[4], json!({"recorded": 1, "session": "s-bing"}));
    assert!(!home.path().join("escaped.ndjson").exists());
    let written: Vec<_> = std::fs::read_dir(home.path().join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(written, vec!["s-bing.ndjson"]);
}

/// A declared length beyond what the host accepts ends the conversation with
/// an answer rather than an attempt to allocate it.
#[test]
fn an_oversized_length_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(home.path(), &["native-host"], &u32::MAX.to_ne_bytes());
    assert!(output.status.success());
    let replies = replies(&output);
    assert_eq!(replies.len(), 1);
    assert!(replies[0]["error"].as_str().unwrap().contains("exceeds"));
}

/// The hook command reads the same message for a browser surface, recorded
/// at `post-tool-use` and nowhere else; a session-start prints no nudge,
/// because no browser surface adds hook output to a model's context.
#[test]
fn the_hook_command_reads_the_same_message_for_a_browser_surface() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(
        home.path(),
        &["hook", "post-tool-use", "--host", "chatgpt-web"],
        CHATGPT_MESSAGE.as_bytes(),
    );
    assert!(output.status.success());
    assert_eq!(records(home.path(), "conv-abc").len(), 4);

    let output = run(
        home.path(),
        &["hook", "session-start", "--host", "bing-copilot-search"],
        br#"{"session": "s-start"}"#,
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "no nudge for a browser surface");
    assert!(records(home.path(), "s-start").is_empty());

    let output = run(
        home.path(),
        &["hook", "post-tool-use", "--host", "google-ai-overview"],
        CHATGPT_MESSAGE.as_bytes(),
    );
    assert!(output.status.success());
    assert_eq!(
        records(home.path(), "conv-abc").len(),
        4,
        "a message naming another surface than the command is refused"
    );
}
