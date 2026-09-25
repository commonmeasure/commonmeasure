//! What the retrieval `timestamp` of a crossing paced by `Crawl-delay` is,
//! measured on the production fetch path.
//!
//! The hub measures the interval between retrievals of one host across a
//! fleet with the event's `timestamp`, so it matters whether that is the
//! time the request was sent, the time the tool call began (before the
//! wait), or something later. The fetch here is the real `context_fetch` of
//! an in-process MCP server against a loopback origin: the real pacing
//! store, the real sleep and the real transport. The origin notes when each
//! request arrived and answers the page slowly, so the three instants are
//! apart.
//!
//! Before the second fetch the test takes the host's turn in the edge's
//! pacing store, as another server on the edge would, a little ahead of now,
//! so the second fetch usually finds the host inside its delay and waits. A
//! thread that stalls for longer than that lead plus the delay finds the
//! turn passed and goes `clear`, which is correct, so the test accepts
//! either outcome. That a turn inside the delay must wait is tested where
//! time is controlled: the pacing store's explicit-instant test in the
//! harness and the projection of a `waited` ruling in the relay.
//!
//! A loopback crossing is kept home by the privacy floor, so the projection
//! step restates the recorded URL and ruling host as a public host. Nothing
//! else in the record is changed; the timestamp and the ruling are the ones
//! the fetch recorded.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_harness::crawl_delay::{CrawlDelayStore, Pace, WAIT_BUDGET};
use commonmeasure_harness::grounding::host_of;
use commonmeasure_harness::mcp::McpServer;
use commonmeasure_harness::policy::SessionPolicy;
use commonmeasure_harness::session::SessionLog;
use commonmeasure_http::{Response, Server};
use commonmeasure_relay::project::{CRAWL_DELAY_FIELD, project_session};
use commonmeasure_relay::wire::WireEventKind;
use commonmeasure_supply::credentials::CredentialsStatus;
use serde_json::{Value, json};

/// Each request the origin received, with when it arrived.
type Arrivals = Arc<Mutex<Vec<(String, DateTime<Utc>)>>>;

/// How long the origin takes to answer the page once asked.
const PAGE_ANSWER: Duration = Duration::from_millis(600);

/// The host's `Crawl-delay`, as the origin's `robots.txt` states it.
const CRAWL_DELAY: Duration = Duration::from_secs(1);

/// How far ahead of now the host's turn is taken before the second fetch.
/// The second fetch waits if it reaches its own turn less than this plus
/// [`CRAWL_DELAY`] later, and waits at most that long; later, it is clear.
const TURN_AHEAD: Duration = Duration::from_secs(2);

fn records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join(format!("sessions/{session}.ndjson")))
        .expect("the session log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
        .collect()
}

fn fetch(server: &mut McpServer, id: u64, url: &str) -> Value {
    server
        .handle_message_text(
            &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                    "params": {"name": "context_fetch", "arguments": {"url": url}}})
            .to_string(),
        )
        .expect("an answer")
}

fn instant(raw: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw.as_str().expect("a timestamp"))
        .expect("RFC 3339")
        .with_timezone(&Utc)
}

// Catches: a retrieval timestamp taken when the tool call began, before the
// `Crawl-delay` wait, which would make two paced requests look closer
// together than the host saw them; and states what the timestamp is instead.
// The after-response bound holds for every crossing whatever turn it took.
#[test]
fn a_paced_retrieval_is_timestamped_when_the_crossing_is_recorded_after_the_response() {
    let arrivals: Arrivals = Arc::default();
    let log = Arc::clone(&arrivals);
    let mut origin = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock()
                .expect("lock")
                .push((request.target.clone(), Utc::now()));
            match request.target.as_str() {
                "/robots.txt" => Response::text(200, "User-agent: *\nCrawl-delay: 1\nAllow: /\n"),
                "/page" => {
                    std::thread::sleep(PAGE_ANSWER);
                    Response::text(200, "the page")
                }
                _ => Response::text(404, "not found"),
            }
        })
        .expect("spawn");
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    )
    .expect("policy");
    let mut server = McpServer::new(
        SessionLog::open(home.path(), "paced-session").expect("session log"),
        SessionPolicy::load(home.path(), None).expect("policy loads"),
        "claude-code",
        None,
        CredentialsStatus {
            path: home.path().join("credentials.env"),
            loaded: None,
        },
    );
    let page = format!("{}/page", origin.url());

    // Two fetches of one page. Before the second, the host's turn is taken
    // ahead of now in the store the server paces from, so the second fetch
    // finds the host inside its delay and waits unless the thread stalls.
    let store = CrawlDelayStore::open(home.path());
    let mut began = Vec::new();
    for id in 1..=2 {
        if id == 2 {
            let ahead = Utc::now() + chrono::Duration::from_std(TURN_AHEAD).unwrap();
            let seeded =
                store.take_turn(&host_of(&page), CRAWL_DELAY, WAIT_BUDGET, ahead, Pace::Own);
            assert!(seeded.sends(), "{seeded:?}");
        }
        began.push(Utc::now());
        let answer = fetch(&mut server, id, &page);
        assert!(answer["result"]["isError"] != json!(true), "{answer}");
    }
    origin.stop();

    let arrivals = arrivals.lock().expect("lock").clone();
    let page_arrivals: Vec<DateTime<Utc>> = arrivals
        .iter()
        .filter(|(target, _)| target == "/page")
        .map(|(_, at)| *at)
        .collect();
    let mut recorded = records(home.path(), "paced-session");
    let crossings: Vec<usize> = recorded
        .iter()
        .enumerate()
        .filter(|(_, record)| record["event"] == "crossing_mediated")
        .map(|(position, _)| position)
        .collect();
    assert_eq!(crossings.len(), 2, "{arrivals:?}");
    assert_eq!(page_arrivals.len(), 2, "{arrivals:?}");

    // Each crossing was recorded after its page answered, whatever turn it
    // took: the timestamp is not the time the request was sent.
    for (crossing, sent) in crossings.iter().zip(&page_arrivals) {
        let payload = &recorded[*crossing]["payload"];
        let timestamp = instant(&payload["timestamp"]);
        assert!(
            timestamp >= *sent + chrono::Duration::from_std(PAGE_ANSWER).unwrap(),
            "recorded {timestamp}, the page asked for at {sent}"
        );
        let delay = &payload["declarations"]["robots"]["delay"];
        assert_eq!(delay["delay_ms"], 1000, "{delay}");
        // Nothing in the record holds the send time: the ruling carries the
        // wait and not the instant the turn was taken.
        for member in ["sent_at", "send_at", "at", "turn_at"] {
            assert!(delay.get(member).is_none(), "{delay}");
        }
    }

    // The second request took the turn after the one taken ahead of it:
    // waited, or clear if the thread stalled past it. Where it waited, the
    // page was asked for after the wait: the timestamp is not the call's
    // start.
    let second = &recorded[crossings[1]]["payload"];
    let delay = &second["declarations"]["robots"]["delay"];
    match delay["outcome"].as_str() {
        Some("waited") => {
            let waited = chrono::Duration::milliseconds(delay["wait_ms"].as_i64().expect("a wait"));
            assert!(waited > chrono::Duration::zero(), "{delay}");
            let sent = page_arrivals[1];
            assert!(sent >= began[1] + waited, "sent {sent}, began {}", began[1]);
        }
        Some("clear") => {}
        _ => panic!("the second fetch was sent, so its turn was clear or waited: {delay}"),
    }
    let timestamp = instant(&second["timestamp"]);

    // Projected, the retrieval carries the recorded timestamp and the delay
    // the fetch kept. The privacy floor keeps a loopback crossing home, so
    // the host is restated as a public one.
    for position in &crossings {
        let payload = &mut recorded[*position]["payload"];
        payload["url"] = json!("https://publisher.example/page");
        if payload["declarations"]["robots"]["delay"].is_object() {
            payload["declarations"]["robots"]["delay"]["host"] = json!("publisher.example");
        }
    }
    let projected = project_session(None, "paced-session", &recorded, &[], &|_| true);
    let retrievals: Vec<_> = projected
        .batches
        .iter()
        .flat_map(|batch| &batch.events)
        .filter(|event| event.kind == WireEventKind::ContentRetrieved)
        .collect();
    assert_eq!(retrievals.len(), 2);
    let event = retrievals[1];
    assert_eq!(
        event.timestamp,
        timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
        "the event carries the recorded timestamp, to the millisecond"
    );
    for event in retrievals {
        assert_eq!(
            event.data.get(CRAWL_DELAY_FIELD),
            Some(&json!({"delay_ms": 1000}))
        );
    }
}
