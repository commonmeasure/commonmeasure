//! Shared CLI integration fixtures. Readers retain each surface's log policy.
// Each integration test is a separate crate and uses only part of this module.
#![allow(dead_code)]

use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};
use std::path::Path;

/// Required NDJSON evidence, allowing blank lines but never a missing file.
pub fn session_records(home: &Path, session: &str) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session}.ndjson"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    parse_records(&text, true)
}

/// Hook and browser paths can deliberately leave no readable record.
/// Browser framing tests also require every line to contain a JSON value.
pub fn optional_records(home: &Path, session: &str, skip_blank_lines: bool) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session}.ndjson"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_records(&text, skip_blank_lines)
}

fn parse_records(text: &str, skip_blank_lines: bool) -> Vec<Value> {
    text.lines()
        .filter(|line| !skip_blank_lines || !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
        .collect()
}

/// The hosted transport uses the production reader, including its validation.
pub fn hosted_records(home: &Path, session: &str) -> Vec<Value> {
    commonmeasure_harness::SessionLog::read(
        &home.join("sessions").join(format!("{session}.ndjson")),
    )
    .expect("the session file reads")
}

/// Read the stdio fixtures' fixed session.
pub fn records(home: &Path) -> Vec<Value> {
    session_records(home, "test-session")
}

/// Crossing records in log order, excluding processor and lifecycle records.
pub fn crossings(home: &Path) -> Vec<Value> {
    records(home)
        .into_iter()
        .filter(|record| {
            record["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("crossing_"))
        })
        .collect()
}

/// Decode the JSON payload a tool result carries: its first text block
/// after the provenance lines, where the call recorded a crossing.
pub fn payload(response: &Value) -> Value {
    let text = commonmeasure_harness::provenance::payload_text(&response["result"])
        .expect("a tool result carries text");
    serde_json::from_str(text).expect("the payload is JSON")
}

/// Serve a fixed text body on loopback.
pub fn origin(body: &'static str) -> ServerHandle {
    text_origin(body.to_owned())
}

/// Serve an owned text body on loopback without leaking it.
pub fn text_origin(body: String) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| Response::text(200, &body))
        .expect("spawn")
}

/// Create the fixture home and write its source policy.
pub fn write_policy(home: &Path, policy: &str) {
    std::fs::create_dir_all(home).expect("home");
    std::fs::write(home.join("policy.json"), policy).expect("policy");
}

/// A stdio tool call using the fixtures' request id.
pub fn call(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
           "params": {"name": name, "arguments": arguments}})
}
