//! The hash a grounding carries for a page fetched whole and for a page
//! read in parts, measured on crossings the production fetch path recorded.
//!
//! The fetch is the real `context_fetch` of an in-process MCP server against
//! a loopback origin. A loopback crossing is kept home by the privacy floor,
//! so the projection step restates the recorded URL as a public host;
//! nothing else in the record is changed.

use std::path::Path;

use commonmeasure_harness::mcp::McpServer;
use commonmeasure_harness::policy::SessionPolicy;
use commonmeasure_harness::session::SessionLog;
use commonmeasure_http::{Response, Server};
use commonmeasure_relay::project::project_session;
use commonmeasure_relay::wire::WireEventKind;
use commonmeasure_supply::credentials::CredentialsStatus;
use serde_json::{Value, json};

const SHORT: &str = "a short page, delivered whole in one result";
const LONG: &str = "the first part of a long page, then the second part of it";

fn records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join(format!("sessions/{session}.ndjson")))
        .expect("the session log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
        .collect()
}

// Catches: a grounding that carries the whole text's hash on each part of a
// page read in parts; and one that, for a page delivered whole, projects
// anything but the `content_hash` it projected before parts were recorded.
#[test]
fn a_whole_page_is_grounded_under_its_content_hash_and_each_part_under_its_own() {
    let mut origin = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| match request.target.as_str() {
            "/short" => Response::text(200, SHORT),
            "/long" => Response::text(200, LONG),
            _ => Response::text(404, "not found"),
        })
        .expect("spawn");
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    )
    .expect("policy");
    let mut server = McpServer::new(
        SessionLog::open(home.path(), "parts-session").expect("session log"),
        SessionPolicy::load(home.path(), None).expect("policy loads"),
        "claude-code",
        None,
        CredentialsStatus {
            path: home.path().join("credentials.env"),
            loaded: None,
        },
    );
    let half = (LONG.chars().count() / 2) as u64;
    let calls = [
        json!({"url": format!("{}/short", origin.url())}),
        json!({"url": format!("{}/long", origin.url()), "max_chars": half}),
        json!({"url": format!("{}/long", origin.url()), "offset": half, "max_chars": half + 1}),
    ];
    for (id, arguments) in calls.iter().enumerate() {
        let answer = server
            .handle_message_text(
                &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                        "params": {"name": "context_fetch", "arguments": arguments}})
                .to_string(),
            )
            .expect("an answer");
        assert!(answer["result"]["isError"] != json!(true), "{answer}");
    }
    origin.stop();

    let mut recorded = records(home.path(), "parts-session");
    let crossings: Vec<usize> = recorded
        .iter()
        .enumerate()
        .filter(|(_, record)| record["event"] == "crossing_mediated")
        .map(|(position, _)| position)
        .collect();
    assert_eq!(crossings.len(), 3);
    let payload = |index: usize| recorded[crossings[index]]["payload"].clone();

    // As recorded: the page delivered whole has a part equal to its whole
    // text, and the two parts of the long page share the whole text's hash
    // and differ in their own.
    let short = payload(0);
    assert_eq!(
        short["delivered"]["total_chars"],
        short["delivered"]["chars"]
    );
    assert_eq!(short["delivered"]["hash"], short["content_hash"]);
    let (first, second) = (payload(1), payload(2));
    assert_eq!(first["content_hash"], second["content_hash"]);
    assert_ne!(first["delivered"]["hash"], second["delivered"]["hash"]);
    for part in [&first, &second] {
        assert_ne!(part["delivered"]["hash"], part["content_hash"], "{part}");
    }

    for position in &crossings {
        recorded[*position]["payload"]["url"] = json!("https://publisher.example/page");
    }
    let projected = project_session(None, "parts-session", &recorded, &[], &|_| true);
    let grounded: Vec<Option<&Value>> = projected
        .batches
        .iter()
        .flat_map(|batch| &batch.events)
        .filter(|event| event.kind == WireEventKind::ContentGrounded)
        .map(|event| event.data.get("content_hash"))
        .collect();
    assert_eq!(
        grounded,
        vec![
            Some(&short["content_hash"]),
            Some(&first["delivered"]["hash"]),
            Some(&second["delivered"]["hash"]),
        ]
    );
}
