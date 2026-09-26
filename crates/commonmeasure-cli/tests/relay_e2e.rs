//! The relay driven as an operator uses it: evidence recorded by the real
//! hook path, the real binary projecting and delivering, and the console's
//! status block read back over real HTTP.
//!
//! The offline tests here use a real loopback receiver (`commonmeasure_http::Server`)
//! to observe the bytes the binary actually posts. The one test against a
//! conforming receiver is ignored by default so the offline gates stay
//! green; see `a_live_receiver_accepts_the_projection_and_isolates_owners`
//! for how to run it.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Request, send};
use serde_json::{Value, json};

/// Record one observed crossing through the real hook binary.
fn record_crossing(home: &Path, session: &str, url: &str) {
    record_crossing_in(home, session, "/work/personal", url);
}

/// The same, in a stated working directory: which policy scope governs a
/// crossing — and so which engagement's clearance it needs — is resolved from
/// this.
fn record_crossing_in(home: &Path, session: &str, cwd: &str, url: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .spawn()
        .expect("hook should start");
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        json!({
            "session_id": session,
            "cwd": cwd,
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": url},
            "tool_response": {"result": "Enough page text to count as grounded."}
        })
    )
    .unwrap();
    assert!(child.wait().expect("wait").success());
}

/// Record one refused crossing straight into a session log, in the shape the
/// mediated server writes, in a stated working directory.
fn record_refused_in(home: &Path, session: &str, cwd: &str, url: &str) {
    let sessions = home.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sessions.join(format!("{session}.ndjson")))
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({
            "seq": 99,
            "event": "crossing_refused",
            "payload": {
                "session_id": session,
                "timestamp": "2026-09-13T10:00:00.000Z",
                "mode": "mediated",
                "host": "claude-code",
                "cwd": cwd,
                "url": url,
                "host_name": "www.example.com",
                "grounded": false,
                "licence": {"state": "unknown"},
                "refusal": "access rule 1 (*) refuses host www.example.com.",
            }
        })
    )
    .unwrap();
}

fn clear_personal_egress(home: &Path) {
    std::fs::write(
        home.join("policy.json"),
        r#"{"scopes":[{"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true}]}"#,
    )
    .unwrap();
}

fn relay(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("relay")
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary should run")
}

struct Console {
    child: Child,
    base: String,
}

impl Console {
    fn start(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("COMMONMEASURE_HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary should start");
        let stdout = child.stdout.as_mut().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let base = loop {
            let line = lines
                .next()
                .expect("serve announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on ") {
                break address.trim().to_owned();
            }
        };
        Self { child, base }
    }

    fn status(&self) -> Value {
        self.json("/api/status")
    }

    fn json(&self, path: &str) -> Value {
        let response = send(&format!("{}{path}", self.base), Request::get("/"))
            .expect("the request should complete");
        assert_eq!(response.status, 200, "GET {path}");
        serde_json::from_slice(&response.body).expect("the API answers JSON")
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn no_default_egress_the_relay_refuses_and_the_console_says_nothing_leaves() {
    let home = tempfile::tempdir().unwrap();
    record_crossing(home.path(), "s-egress-e2e", "https://www.example.com/page");

    let output = relay(home.path(), &[]);
    assert!(
        !output.status.success(),
        "an unconfigured relay must fail rather than pick a destination"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no telemetry receiver is configured"),
        "the refusal names the missing configuration, got: {stderr}"
    );
    assert!(
        !home.path().join("relay").exists(),
        "refusing must leave no relay state behind"
    );

    let console = Console::start(home.path());
    let status = console.status();
    assert_eq!(status["egress"]["receiver"], Value::Null);
    assert_eq!(status["egress"]["delivered"], 0);
    assert!(
        status["egress"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing leaves this machine"),
        "got: {}",
        status["egress"]["detail"]
    );
}

#[test]
fn the_console_reports_the_configured_receiver_and_delivered_counts_truthfully() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(
        home.path(),
        "s-status-e2e",
        "https://www.example.com/reported",
    );

    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = bodies.clone();
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut receiver = server
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            captured.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap();

    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": receiver.url()}).to_string(),
    )
    .unwrap();
    let output = relay(home.path(), &[]);
    assert!(
        output.status.success(),
        "relay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let delivered = bodies.lock().unwrap();
    assert_eq!(delivered.len(), 1, "one session, one batch");
    let events = delivered[0]["events"].as_array().expect("events").len();
    assert_eq!(events, 2, "one grounded crossing: retrieval and grounding");
    drop(delivered);

    let console = Console::start(home.path());
    let status = console.status();
    assert_eq!(status["egress"]["receiver"], json!(receiver.url()));
    assert_eq!(status["egress"]["delivered"], 2);
    assert_eq!(status["egress"]["pending"], 0);
    receiver.stop();
}

/// The two engagement identities over one recorded crossing, both driven by
/// the components that own them: the real relay clears the crossing under the
/// governing engagement its policy scope declares and delivers it to a real
/// receiver, while the real console reports the same work under the different
/// engagement its attribution rules resolve.
///
/// Before the reconciliation this proves, nothing on any surface said so: the
/// session table read `commonmeasure`, the relay cleared `personal`, and an
/// operator could only find that out by opening both files.
#[test]
fn the_console_names_the_engagement_the_relay_cleared_when_attribution_reports_another() {
    let home = tempfile::tempdir().unwrap();
    // The governing engagement, and the only name any enforcing seam can read.
    clear_personal_egress(home.path());
    // The reported engagement, resolved at read time over the same directory.
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[{"match":"/work/personal","engagement":"commonmeasure"}]}"#,
    )
    .unwrap();
    record_crossing(home.path(), "s-two-names", "https://www.example.com/page");

    let delivered: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = delivered.clone();
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut receiver = server
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            captured.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": receiver.url()}).to_string(),
    )
    .unwrap();

    // The crossing leaves the machine, cleared by the governing engagement.
    let output = relay(home.path(), &[]);
    assert!(
        output.status.success(),
        "relay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        delivered.lock().unwrap().len(),
        1,
        "the cleared crossing was projected"
    );

    let console = Console::start(home.path());
    // The same crossing is reported under the other name.
    assert_eq!(
        console.json("/api/sessions")[0]["engagement"],
        "commonmeasure"
    );

    let policy = console.json("/api/policy");
    let engagements = policy["engagements"].as_array().expect("engagements");
    assert_eq!(engagements.len(), 1);
    assert_eq!(engagements[0]["engagement"], "commonmeasure");
    let stance = &engagements[0]["stances"][0];
    assert_eq!(stance["governing_engagement"], "personal");
    assert_eq!(stance["allow_telemetry_egress"], true);
    assert_eq!(stance["engagement_diverges"], true);

    let divergence = &policy["engagement_divergences"][0];
    assert_eq!(divergence["reported"], "commonmeasure");
    assert_eq!(divergence["governing"], "personal");
    assert_eq!(divergence["scope"], "/work/personal");
    assert_eq!(divergence["allow_telemetry_egress"], true);
    receiver.stop();
}

/// The figure the report states beneath the delivery line for one clearance.
/// Reading the number rather than matching the sentence: the claim under test
/// is that the engagement is named with its own count, not that a particular
/// wording survives.
fn events_under(report: &str, clearance: &str) -> Option<u64> {
    report
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .find(|line| line.contains(clearance))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|count| count.parse().ok())
}

/// The moment of egress, made legible: the relay's own report names the
/// governing engagement whose clearance let each delivered event leave, and
/// states the session that stayed home because nothing in it was cleared.
///
/// The fixture is the divergence one — a governing `personal` cleared, a
/// reported `commonmeasure` over the same directory — because it also pins the
/// decision this package took: the reported engagement is reachable from
/// `commonmeasure-cli`, is on the console for the asking, and is deliberately not in the
/// account of what left. Egress is a capture-time enforcement act and the only
/// name that authorised it is the governing one.
#[test]
fn the_relay_report_names_the_governing_engagement_that_cleared_what_it_delivered() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[
            {"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true},
            {"match":"/work/client-a","engagement":"client-a"}
        ]}"#,
    )
    .unwrap();
    // The reported engagement over the cleared directory, resolved at read time
    // and disagreeing with the governing one.
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[{"match":"/work/personal","engagement":"commonmeasure"}]}"#,
    )
    .unwrap();
    record_crossing(home.path(), "s-cleared", "https://www.example.com/cleared");
    // Two refusals in the cleared session and one in the client's: the
    // cleared session's count crosses, the client's does not, and no
    // refused URL or reason leaves with either.
    record_refused_in(
        home.path(),
        "s-cleared",
        "/work/personal",
        "https://www.example.com/refused-one",
    );
    record_refused_in(
        home.path(),
        "s-cleared",
        "/work/personal",
        "https://www.example.com/refused-two",
    );
    record_crossing_in(
        home.path(),
        "s-client",
        "/work/client-a",
        "https://www.example.com/client",
    );
    record_refused_in(
        home.path(),
        "s-client",
        "/work/client-a",
        "https://www.example.com/client-refused",
    );

    let delivered: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = delivered.clone();
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut receiver = server
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            captured.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": receiver.url()}).to_string(),
    )
    .unwrap();

    let output = relay(home.path(), &[]);
    assert!(
        output.status.success(),
        "relay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout).to_string();
    receiver.stop();

    // What the receiver actually took, counted at the receiver.
    let batches = delivered.lock().unwrap();
    let received: u64 = batches
        .iter()
        .map(|batch| batch["events"].as_array().map(Vec::len).unwrap_or(0) as u64)
        .sum();
    assert!(received > 0, "the cleared crossing left the machine");
    assert_eq!(batches.len(), 1, "one session projected, one batch");
    assert_eq!(
        batches[0]["refused"], 2,
        "the cleared session's refusals cross as a count: {}",
        batches[0]
    );
    let wire = batches[0].to_string();
    assert!(
        !wire.contains("refused-one") && !wire.contains("access rule"),
        "{wire}"
    );
    drop(batches);
    assert!(
        report.contains(
            "2 refused crossings in the projected sessions, on the wire as a count per \
             session with no URL and no reason"
        ),
        "the summary states the count that crossed; got: {report}"
    );

    assert_eq!(
        events_under(&report, "personal"),
        Some(received),
        "the report names the engagement whose clearance delivered these events, \
         and the count is what the receiver took; got: {report}"
    );
    assert_eq!(
        events_under(&report, "withheld").map(|withheld| withheld as usize),
        Some(1),
        "the uncleared session is an absence to state, not a row to omit; got: {report}"
    );

    // The reported engagement is on the console, and is not in the account of
    // what left.
    let console = Console::start(home.path());
    let reported: Vec<String> = console
        .json("/api/sessions")
        .as_array()
        .expect("sessions")
        .iter()
        .filter_map(|session| session["engagement"].as_str().map(str::to_owned))
        .collect();
    assert!(
        reported.iter().any(|name| name == "commonmeasure"),
        "the console reports the other identity: {reported:?}"
    );
    assert!(
        !report.contains("commonmeasure"),
        "a read-time reporting name has no place in the account of an \
         enforcement act; got: {report}"
    );
}

/// The one test in this workspace that requires another process: a conforming
/// Content Telemetry receiver on a local socket. Ignored by default so the
/// offline gates stay green. To run it:
///
/// 1. Start a conforming receiver locally, provisioned with a platform write
///    key and a read key for each of two publishers that returns only that
///    publisher's events.
/// 2. Export `COMMONMEASURE_RELAY_TEST_KEY` (the `oat_pk_` write key),
///    `COMMONMEASURE_RELAY_TEST_READER_GUARDIAN` and
///    `COMMONMEASURE_RELAY_TEST_READER_TELEGRAPH` (the `oat_pub_` read keys), and
///    optionally `COMMONMEASURE_RELAY_TEST_RECEIVER` (default
///    `http://localhost:8080`).
/// 3. `cargo test -p commonmeasure-cli --test relay_e2e -- --ignored`
#[test]
#[ignore = "requires a local conforming receiver (see the doc comment for how to run it)"]
fn a_live_receiver_accepts_the_projection_and_isolates_owners() {
    let receiver = std::env::var("COMMONMEASURE_RELAY_TEST_RECEIVER")
        .unwrap_or_else(|_| "http://localhost:8080".to_owned());
    let write_key = std::env::var("COMMONMEASURE_RELAY_TEST_KEY")
        .expect("COMMONMEASURE_RELAY_TEST_KEY must hold the oat_pk_ write key");
    let guardian_key = std::env::var("COMMONMEASURE_RELAY_TEST_READER_GUARDIAN")
        .expect("COMMONMEASURE_RELAY_TEST_READER_GUARDIAN must hold Guardian's oat_pub_ read key");
    let telegraph_key = std::env::var("COMMONMEASURE_RELAY_TEST_READER_TELEGRAPH").expect(
        "COMMONMEASURE_RELAY_TEST_READER_TELEGRAPH must hold Telegraph's oat_pub_ read key",
    );

    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    let marker = format!(
        "relay-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0)
    );

    // Two owners' content in two sessions, recorded by the real hook path.
    let guardian_urls = [
        format!("https://www.theguardian.com/{marker}/politics"),
        format!("https://www.theguardian.com/{marker}/business"),
    ];
    let telegraph_urls = [format!("https://www.telegraph.co.uk/{marker}/news")];
    for url in &guardian_urls {
        record_crossing(home.path(), &format!("live-a-{marker}"), url);
    }
    for url in &telegraph_urls {
        record_crossing(home.path(), &format!("live-b-{marker}"), url);
    }

    // One published run: an admitted Guardian source crosses, a rejected
    // Telegraph source stays home.
    let run_dir = home.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let run_url = format!("https://www.theguardian.com/{marker}/run-source");
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    std::fs::write(
        run_dir.join("summary.json"),
        json!({
            "schema_version": "contextops-run/v1",
            "run": {"id": run_uuid(&marker), "started_at": now},
            "plans": [{
                "id": "live-plan",
                "sources": [
                    {
                        "admitted": true,
                        "url": run_url,
                        "retrieval_rank": 1,
                        "tokens": 200,
                        "content_hash": format!("sha256:{}", "ab".repeat(32)),
                        "licence": {"state": "unknown"},
                    },
                    {
                        "admitted": false,
                        "admission_reason": "Host is outside the job's allowed-host list.",
                        "url": format!("https://www.telegraph.co.uk/{marker}/rejected"),
                        "retrieval_rank": 2,
                        "tokens": 100,
                        "content_hash": format!("sha256:{}", "cd".repeat(32)),
                        "licence": {"state": "unknown"},
                    },
                ],
            }],
        })
        .to_string(),
    )
    .unwrap();

    let run_arg = run_dir.display().to_string();
    let args = [
        "--receiver",
        receiver.as_str(),
        "--api-key",
        write_key.as_str(),
        "--run",
        run_arg.as_str(),
    ];
    let first = relay(home.path(), &args);
    assert!(
        first.status.success(),
        "live delivery failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        stdout.contains("projected 2 sessions and 1 runs"),
        "got: {stdout}"
    );
    // 2 grounded session crossings (2 events each), 1 grounded session
    // crossing (2 events), 1 admitted grounded run source (2 events): 8, all
    // new at the receiver.
    assert!(stdout.contains("delivered 8 events"), "got: {stdout}");
    assert!(stdout.contains("(8 new at the receiver)"), "got: {stdout}");

    // Idempotency, live: an unchanged ledger redelivers nothing.
    let second = relay(home.path(), &args);
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("0 events newly spooled"), "got: {stdout}");
    assert!(stdout.contains("delivered 0 events"), "got: {stdout}");

    // Idempotency across a crash: killed after delivery, before the
    // acknowledgement. The receiver has seen every id before and creates
    // nothing.
    std::fs::remove_file(home.path().join("relay/spool/outbound.delivery.json")).unwrap();
    let redelivered = relay(home.path(), &args);
    let stdout = String::from_utf8_lossy(&redelivered.stdout);
    assert!(stdout.contains("delivered 8 events"), "got: {stdout}");
    assert!(
        stdout.contains("(0 new at the receiver)"),
        "redelivery must not double-count, got: {stdout}"
    );

    // Two-owner isolation at the receiver: each publisher's key sees its own
    // content and none of the other's.
    let guardian_seen = owner_urls(&receiver, &guardian_key, &marker);
    assert!(
        guardian_urls.iter().all(|url| guardian_seen.contains(url))
            && guardian_seen.contains(&run_url),
        "Guardian must see its own events, saw: {guardian_seen:?}"
    );
    assert!(
        guardian_seen
            .iter()
            .all(|url| !url.contains("telegraph.co.uk")),
        "Guardian must not see Telegraph content, saw: {guardian_seen:?}"
    );
    let telegraph_seen = owner_urls(&receiver, &telegraph_key, &marker);
    assert!(
        telegraph_urls
            .iter()
            .all(|url| telegraph_seen.contains(url)),
        "Telegraph must see its own events, saw: {telegraph_seen:?}"
    );
    assert!(
        telegraph_seen
            .iter()
            .all(|url| !url.contains("theguardian.com")),
        "Telegraph must not see Guardian content, saw: {telegraph_seen:?}"
    );
    assert!(
        telegraph_seen.iter().all(|url| !url.contains("rejected")),
        "a source the run rejected must never reach its owner, saw: {telegraph_seen:?}"
    );
}

/// This owner's view of recent events, filtered to this test's own URLs.
fn owner_urls(receiver: &str, reader_key: &str, marker: &str) -> Vec<String> {
    let mut request = Request::get("/");
    request.headers.set("X-API-Key", reader_key);
    let response = send(
        &format!("{receiver}/content-owners/events?limit=1000"),
        request,
    )
    .expect("owner query should complete");
    assert_eq!(
        response.status,
        200,
        "owner query answered {}: {}",
        response.status,
        String::from_utf8_lossy(&response.body)
    );
    let body: Value = serde_json::from_slice(&response.body).expect("owner query answers JSON");
    let items = body["items"].as_array().cloned().unwrap_or_default();
    items
        .iter()
        .filter_map(|event| event["content_url"].as_str())
        .filter(|url| url.contains(marker))
        .map(str::to_owned)
        .collect()
}

/// A UUID for the fixture run, derived from the marker so a re-run of the
/// test is a fresh run. Format only; no uuid dependency needed here.
fn run_uuid(marker: &str) -> String {
    let digest = marker.bytes().fold(0u128, |acc, byte| {
        acc.wrapping_mul(131).wrapping_add(byte as u128)
    });
    let hex = format!("{digest:032x}");
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// A refusal after a session's last admitted crossing still reaches the
/// receiver across a relay boundary: the count travels only on a batch, so
/// the session's last delivered event goes again under its own id carrying
/// the new count, the receiver takes the larger count, and the summary
/// states what was put on the wire and nothing more.
#[test]
fn a_refusal_after_the_last_admitted_crossing_still_reaches_the_receiver() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "s-late", "https://www.example.com/admitted");

    // A receiver that, like a conforming one, counts an event once by its
    // id, so a carried event is accepted and not created again.
    let delivered: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = delivered.clone();
    let seen: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut receiver = server
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let mut seen = seen.lock().unwrap();
            let created = body["events"]
                .as_array()
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|event| event["id"].as_str().map(str::to_owned))
                        .filter(|id| seen.insert(id.clone()))
                        .count()
                })
                .unwrap_or(0);
            captured.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": created}).to_string(),
            )
        })
        .unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": receiver.url()}).to_string(),
    )
    .unwrap();

    let first = relay(home.path(), &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(delivered.lock().unwrap().len(), 1);
    assert_eq!(delivered.lock().unwrap()[0]["refused"], 0);
    let first_ids: Vec<Value> = delivered.lock().unwrap()[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["id"].clone())
        .collect();

    // Two refusals after everything admitted has already left.
    record_refused_in(
        home.path(),
        "s-late",
        "/work/personal",
        "https://www.example.com/one",
    );
    record_refused_in(
        home.path(),
        "s-late",
        "/work/personal",
        "https://www.example.com/two",
    );
    let second = relay(home.path(), &[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let report = String::from_utf8_lossy(&second.stdout).to_string();
    {
        let batches = delivered.lock().unwrap();
        assert_eq!(batches.len(), 2, "a batch went for the count alone");
        let latest = &batches[1];
        assert_eq!(latest["refused"], 2, "{latest}");
        let carried: Vec<Value> = latest["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].clone())
            .collect();
        assert_eq!(carried.len(), 1);
        assert_eq!(
            carried[0],
            *first_ids.last().unwrap(),
            "the session's last delivered event, under its own id"
        );
        assert!(!latest.to_string().contains("example.com/one"));
    }
    assert!(
        report.contains("2 refused crossings in the projected sessions"),
        "{report}"
    );
    assert!(report.contains("0 new at the receiver"), "{report}");

    // Nothing has moved: nothing goes, and the summary says so.
    let third = relay(home.path(), &[]);
    assert!(third.status.success());
    let report = String::from_utf8_lossy(&third.stdout).to_string();
    assert_eq!(delivered.lock().unwrap().len(), 2);
    assert!(
        report.contains("0 refused crossings in the projected sessions"),
        "{report}"
    );
    receiver.stop();
}

/// Include directories as well as file bytes: even creating an empty relay
/// directory violates a forecast's promise to leave the operator home alone.
fn home_snapshot(home: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &Path,
        path: &Path,
        entries: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                entries.insert(relative, None);
                visit(root, &path, entries);
            } else {
                entries.insert(relative, Some(std::fs::read(path).unwrap()));
            }
        }
    }
    let mut entries = std::collections::BTreeMap::new();
    visit(home, home, &mut entries);
    entries
}

fn capture_relay(bodies: Arc<Mutex<Vec<Value>>>) -> commonmeasure_http::ServerHandle {
    commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            bodies.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap()
}

fn relay_text(home: &Path, args: &[&str]) -> String {
    let output = relay(home, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn dry_run_matches_delivery_and_leaves_the_whole_home_unchanged() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "forecast", "https://www.example.com/page");
    record_crossing_in(
        home.path(),
        "withheld",
        "/work/private",
        "https://private.example/page",
    );
    record_refused_in(
        home.path(),
        "forecast",
        "/work/personal",
        "https://refused.example/page",
    );
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    let before = home_snapshot(home.path());
    let forecast = relay_text(home.path(), &["--receiver", &url, "--dry-run"]);
    assert_eq!(before, home_snapshot(home.path()));
    assert!(bodies.lock().unwrap().is_empty());
    let expected = format!(
        concat!(
            "dry run: nothing was sent; no state was changed\n",
            "would deliver 2 events in 1 batches to {} (new at the receiver: unknown)\n",
            "  1 content_grounded\n",
            "  1 content_retrieved\n",
            "  2 under governing engagement personal\n",
            "projected 1 of 2 sessions and 0 runs; 2 events would be newly spooled\n",
            "  1 withheld: no crossing cleared to leave\n",
            "  1 refused crossings in the projected sessions, on the wire as a count per session with no URL and no reason\n",
            "hosts that would leave:\n",
            "  www.example.com\n",
            "forecast policy: {}, as it stands on disk\n",
            "a real run syncs managed policy and reporting approvals first, which can change what is cleared and what leaves\n"
        ),
        url,
        home.path().join("policy.json").display()
    );
    assert_eq!(forecast, expected);
    let delivered = relay_text(home.path(), &["--receiver", &url]);
    assert!(
        delivered.contains("projected 1 of 2 sessions and 0 runs; 2 events newly spooled"),
        "{delivered}"
    );
    assert!(
        delivered.contains("delivered 2 events in 1 batches"),
        "{delivered}"
    );
    assert!(
        delivered.contains("  2 under governing engagement personal"),
        "{delivered}"
    );
    assert_eq!(
        bodies.lock().unwrap()[0]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(bodies.lock().unwrap()[0]["refused"], 1);
    let after_delivery = home_snapshot(home.path());
    let empty = relay_text(home.path(), &["--receiver", &url, "--dry-run"]);
    assert!(
        empty.contains("would deliver 0 events in 0 batches"),
        "{empty}"
    );
    assert!(empty.contains("hosts that would leave: none\n"), "{empty}");
    assert_eq!(after_delivery, home_snapshot(home.path()));
    assert_eq!(bodies.lock().unwrap().len(), 1);
    receiver.stop();
}

#[test]
fn dry_run_of_a_draft_clears_more_events_without_writing() {
    let home = tempfile::tempdir().unwrap();
    let drafts = tempfile::tempdir().unwrap();
    let draft = drafts.path().join("draft.json");
    clear_personal_egress(home.path());
    record_crossing_in(
        home.path(),
        "draft",
        "/work/client",
        "https://client.example/page",
    );
    std::fs::write(&draft, r#"{"scopes":[{"match":"/work/client","engagement":"client","allow_telemetry_egress":true}]}"#).unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    let before = home_snapshot(home.path());
    let current = relay_text(home.path(), &["--receiver", &url, "--dry-run"]);
    let forecast = relay_text(
        home.path(),
        &[
            "--receiver",
            &url,
            "--dry-run",
            "--policy",
            draft.to_str().unwrap(),
        ],
    );
    assert!(
        current.contains("would deliver 0 events in 0 batches"),
        "{current}"
    );
    assert!(
        forecast.contains("would deliver 2 events in 1 batches"),
        "{forecast}"
    );
    assert!(
        forecast.contains("  2 under governing engagement client"),
        "{forecast}"
    );
    assert!(
        forecast.contains("hosts that would leave:\n  client.example\n"),
        "{forecast}"
    );
    // The basis line names the draft the forecast was taken against, and
    // says what a real run would refresh before projecting.
    assert!(
        forecast.ends_with(&format!(
            "forecast policy: the draft {}\na real run syncs managed policy and reporting approvals \
             first, which can change what is cleared and what leaves\n",
            draft.display()
        )),
        "{forecast}"
    );
    assert_eq!(before, home_snapshot(home.path()));
    assert!(bodies.lock().unwrap().is_empty());
    receiver.stop();
}

#[test]
fn dry_run_rejects_unreadable_and_invalid_drafts_with_the_reason_and_sends_nothing() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "invalid-draft", "https://www.example.com/page");
    let draft = home.path().join("draft.json");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    for (bytes, reason) in [
        (None, "cannot read"),
        (Some("{"), "not a valid policy"),
        (
            Some(
                r#"{"scopes":[{"match":"","engagement":"client","allow_telemetry_egress":true}]}"#,
            ),
            "empty",
        ),
    ] {
        if let Some(bytes) = bytes {
            std::fs::write(&draft, bytes).unwrap();
        }
        let before = home_snapshot(home.path());
        let output = relay(
            home.path(),
            &[
                "--receiver",
                &receiver.url(),
                "--dry-run",
                "--policy",
                draft.to_str().unwrap(),
            ],
        );
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(reason) && error.contains("draft.json"),
            "{error}"
        );
        assert_eq!(before, home_snapshot(home.path()));
        assert!(bodies.lock().unwrap().is_empty());
    }
    receiver.stop();
}

/// Use the real enrolment record and session-start hook to append the identity
/// a resumed session gains. No enrolment or hub integration is claimed here.
fn resume_with_identity(home: &Path, session: &str, hub: &str) -> String {
    let key = commonmeasure_harness::identity::EdgeKey::generate().unwrap();
    key.store(home).unwrap();
    let key_id = key.thumbprint();
    std::fs::write(
        home.join("enrolment.json"),
        json!({
            "hub": hub, "organization": {"id": "relay-test", "name": "Relay test"},
            "name": "relay-test", "key_id": key_id,
            "identity": {"origin": hub, "bot_page": format!("{hub}/bot")},
            "enrolled_at": "2026-09-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "session-start"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{}", json!({
        "session_id": session, "hook_event_name": "SessionStart", "source": "resume", "cwd": "/work/personal"
    })).unwrap();
    assert!(child.wait().unwrap().success());
    let records = commonmeasure_harness::SessionLog::read(
        &home.join("sessions").join(format!("{session}.ndjson")),
    )
    .unwrap();
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "edge_identity"
                && record["payload"]["key_id"] == key_id)
    );
    key_id
}

#[test]
fn resumed_session_keeps_its_first_agent_at_the_receiver_and_empty_runs_keep_the_pin() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "resumed", "https://www.example.com/first");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    relay_text(home.path(), &["--receiver", &url]);
    let pin_path = home.path().join("relay/session-agents.json");
    let pin = std::fs::read(&pin_path).unwrap();
    let empty = relay_text(home.path(), &["--receiver", &url]);
    assert!(empty.contains("delivered 0 events in 0 batches"), "{empty}");
    assert_eq!(pin, std::fs::read(&pin_path).unwrap());
    let key_id = resume_with_identity(home.path(), "resumed", &url);
    record_crossing(home.path(), "resumed", "https://www.example.com/second");
    relay_text(home.path(), &["--receiver", &url]);
    let captured = bodies.lock().unwrap();
    let batches: Vec<_> = captured
        .iter()
        .filter(|body| body["document_type"] == "event_batch")
        .collect();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["session_id"], batches[1]["session_id"]);
    assert_eq!(batches[0]["agent_id"], "commonmeasure");
    assert_eq!(batches[1]["agent_id"], "commonmeasure");
    assert_ne!(batches[1]["agent_id"], key_id);
    assert_eq!(
        batches[1]["events"][0]["content_url"],
        "https://www.example.com/second"
    );
    drop(captured);
    assert_eq!(pin, std::fs::read(pin_path).unwrap());
    receiver.stop();
}

#[test]
fn retained_spool_recovers_the_agent_for_a_session_delivered_before_pins_exist() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "upgrade", "https://www.example.com/first");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    relay_text(home.path(), &["--receiver", &url]);
    std::fs::remove_file(home.path().join("relay/session-agents.json")).unwrap();
    resume_with_identity(home.path(), "upgrade", &url);
    record_crossing(home.path(), "upgrade", "https://www.example.com/second");
    relay_text(home.path(), &["--receiver", &url]);
    let captured = bodies.lock().unwrap();
    let batches: Vec<_> = captured
        .iter()
        .filter(|body| body["document_type"] == "event_batch")
        .collect();
    assert_eq!(batches.len(), 2);
    assert!(
        batches
            .iter()
            .all(|batch| batch["agent_id"] == "commonmeasure")
    );
    assert_eq!(batches[0]["session_id"], batches[1]["session_id"]);
    drop(captured);
    receiver.stop();
}

/// EGR-05 at the command: a forecast for a receiver named on the command line
/// prints neither the key in `relay.json` nor the one passed with it.
#[test]
fn dry_run_prints_no_api_key() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "s1", "https://host.example/page");
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": "https://hub.example/api/v1/telemetry", "api_key": "configured-key"})
            .to_string(),
    )
    .unwrap();
    let output = relay(
        home.path(),
        &[
            "--dry-run",
            "--receiver",
            "https://other.example",
            "--api-key",
            "passed-key",
        ],
    );
    assert!(output.status.success());
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(printed.contains("would deliver"), "{printed}");
    assert!(
        !printed.contains("configured-key") && !printed.contains("passed-key"),
        "{printed}"
    );
}

/// A receipt as 0.4.1 wrote it holds the receiver as configured, and that
/// release accepted a key in the receiver's credentials or query. `doctor`
/// names the receiver by origin whether `relay.json` is still refused,
/// corrected as the 0.4.2 Upgrading note says, or removed.
#[test]
fn doctor_names_a_receipt_receiver_by_its_origin_alone() {
    for delivered_to in [
        "https://ops:ak_PLANTED@hub.example/api/v1/telemetry",
        "https://hub.example/api/v1/telemetry?api_key=ak_PLANTED",
    ] {
        for relay_json in [
            Some(json!({"receiver": delivered_to})),
            Some(
                json!({"receiver": "https://hub.example/api/v1/telemetry", "api_key": "ak_PLANTED"}),
            ),
            None,
        ] {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join("relay")).unwrap();
            std::fs::write(
                home.path().join("relay/receipts.json"),
                json!({
                    "receiver": delivered_to,
                    "delivered_to": delivered_to,
                    "last_delivered_at": "2026-09-20T10:00:00.000Z",
                    "last_error": null,
                })
                .to_string(),
            )
            .unwrap();
            if let Some(relay_json) = &relay_json {
                std::fs::write(home.path().join("relay.json"), relay_json.to_string()).unwrap();
            }
            let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
                .arg("doctor")
                .env("COMMONMEASURE_HOME", home.path())
                .output()
                .unwrap();
            let printed = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success(), "{printed}");
            assert!(
                printed.contains("last delivery: 2026-09-20T10:00:00.000Z"),
                "{printed}"
            );
            assert!(
                !printed.contains("ak_PLANTED"),
                "{delivered_to} with relay.json {relay_json:?}: {printed}"
            );
        }
    }
}

#[test]
fn dry_run_of_an_empty_home_prints_zero_and_creates_no_relay_state() {
    let home = tempfile::tempdir().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    let before = home_snapshot(home.path());
    let text = relay_text(home.path(), &["--receiver", &url, "--dry-run"]);
    assert_eq!(
        text,
        format!(
            concat!(
                "dry run: nothing was sent; no state was changed\n",
                "would deliver 0 events in 0 batches to {} (new at the receiver: unknown)\n",
                "projected 0 of 0 sessions and 0 runs; 0 events would be newly spooled\n",
                "hosts that would leave: none\n",
                "forecast policy: {}, as it stands on disk\n",
                "a real run syncs managed policy and reporting approvals first, which can change what is cleared and what leaves\n"
            ),
            url,
            home.path().join("policy.json").display()
        )
    );
    assert_eq!(before, home_snapshot(home.path()));
    assert!(bodies.lock().unwrap().is_empty());
    receiver.stop();
}

#[test]
fn dry_run_refreshes_neither_managed_policy_nor_enrolment() {
    let home = tempfile::tempdir().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    resume_with_identity(home.path(), "managed-forecast", &url);
    clear_personal_egress(home.path());
    record_crossing(
        home.path(),
        "managed-forecast",
        "https://www.example.com/page",
    );
    std::fs::write(home.path().join("deployment.json"), json!({
        "mode": "managed",
        "signer": {"key_id": "forecast-signer", "algorithm": "ed25519",
                   "public_key": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"},
        "policy_url": format!("{url}/api/v1/policy/desired"),
        "organisation": "relay-test"
    }).to_string()).unwrap();
    let before = home_snapshot(home.path());
    let text = relay_text(
        home.path(),
        &[
            "--receiver",
            &url,
            "--api-key",
            "loopback-test-key",
            "--dry-run",
        ],
    );
    assert!(
        text.contains("would deliver 2 events in 1 batches"),
        "{text}"
    );
    assert_eq!(before, home_snapshot(home.path()));
    assert!(
        bodies.lock().unwrap().is_empty(),
        "no policy, standing or delivery request"
    );
    receiver.stop();
}

#[test]
fn dry_run_includes_pending_batches_and_a_failed_attempt_keeps_its_agent_on_retry() {
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "retry", "https://www.example.com/first");
    let mut refusing = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(|_| {
            commonmeasure_http::Response::json(503, r#"{"detail":"temporarily unavailable"}"#)
        })
        .unwrap();
    let failed = relay(home.path(), &["--receiver", &refusing.url()]);
    assert!(!failed.status.success());
    refusing.stop();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let url = receiver.url();
    resume_with_identity(home.path(), "retry", &url);
    record_crossing(home.path(), "retry", "https://second.example/second");
    make_retries_due(home.path());
    let before = home_snapshot(home.path());
    let forecast = relay_text(home.path(), &["--receiver", &url, "--dry-run"]);
    assert!(
        forecast.contains("2 events would be newly spooled"),
        "{forecast}"
    );
    assert!(
        forecast.contains("would deliver 4 events in 2 batches"),
        "{forecast}"
    );
    assert!(
        forecast.contains("hosts that would leave:\n  second.example\n  www.example.com\n"),
        "{forecast}"
    );
    assert_eq!(before, home_snapshot(home.path()));
    assert!(bodies.lock().unwrap().is_empty());
    let delivered = relay_text(home.path(), &["--receiver", &url]);
    assert!(
        delivered.contains("delivered 4 events in 2 batches"),
        "{delivered}"
    );
    let captured = bodies.lock().unwrap();
    let batches: Vec<_> = captured
        .iter()
        .filter(|body| body["document_type"] == "event_batch")
        .collect();
    assert_eq!(batches.len(), 2);
    assert!(
        batches
            .iter()
            .all(|batch| batch["agent_id"] == "commonmeasure")
    );
    drop(captured);
    receiver.stop();
}

/// Fault injection for CLI tests: advance persisted deadlines instead of waiting
/// through the real schedule, while no relay is running. Production has no clock
/// override on the CLI.
fn make_retries_due(home: &Path) {
    let path = home.join("relay/spool/outbound.delivery.json");
    let mut states: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for state in states.as_object_mut().unwrap().values_mut() {
        state["next_attempt_at"] = json!("2000-01-01T00:00:00Z");
    }
    std::fs::write(path, serde_json::to_vec(&states).unwrap()).unwrap();
}

#[test]
fn cadence_is_visible_in_status_doctor_console_and_explicit_requeue() {
    use commonmeasure_relay::spool::{MAX_ATTEMPTS, Spool};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "cli-cadence", "https://public.example/page");
    let accept = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let flag = accept.clone();
    let seen = calls.clone();
    let mut receiver = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            if flag.load(Ordering::SeqCst) {
                commonmeasure_http::Response::json(201, r#"{"status":"ok","events_created":2}"#)
            } else {
                commonmeasure_http::Response::text(503, "fixture outage")
            }
        })
        .unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver":receiver.url()}).to_string(),
    )
    .unwrap();
    assert!(!relay(home.path(), &[]).status.success());
    let deferred = relay_text(home.path(), &[]);
    assert!(
        deferred.contains("1 queued batches and 0 dead batches remain undelivered"),
        "{deferred}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        !relay(home.path(), &["requeue", "--batch", "0"])
            .status
            .success()
    );
    let missing = relay(home.path(), &["requeue", "--batch", "999"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no such spool batch: 999"));
    {
        let _owner = Spool::open(home.path()).unwrap();
        let locked = relay(home.path(), &["requeue"]);
        assert!(!locked.status.success());
        assert!(
            String::from_utf8_lossy(&locked.stderr)
                .contains("another relay or requeue owns the spool")
        );
    }
    let forecast = relay_text(home.path(), &["--dry-run"]);
    assert!(
        forecast.contains("would deliver 0 events in 0 batches"),
        "{forecast}"
    );
    for _ in 1..MAX_ATTEMPTS {
        make_retries_due(home.path());
        assert!(!relay(home.path(), &[]).status.success());
    }
    for args in [vec!["status"], vec!["doctor", "claude-code"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(args)
            .env("COMMONMEASURE_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("0 queued, 1 dead, 0 delivered"), "{text}");
        assert!(text.contains("fixture outage"), "{text}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["status", "--json"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["egress"]["dead"], 1);
    assert_eq!(status["egress"]["delivered_batches"], 0);
    let console = Console::start(home.path());
    let page = send(&format!("{}/app", console.base), Request::get("/app")).unwrap();
    let page = String::from_utf8(page.body).unwrap();
    assert!(
        page.contains("Dead batches")
            && page.contains("fixture outage")
            && page.contains("commonmeasure relay requeue"),
        "{page}"
    );
    let before = calls.load(Ordering::SeqCst);
    let requeued = relay_text(home.path(), &["requeue"]);
    assert!(requeued.contains("requeued 1 dead batches"), "{requeued}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "requeue sends nothing"
    );
    accept.store(true, Ordering::SeqCst);
    let delivered = relay_text(home.path(), &[]);
    assert!(
        delivered.contains("delivered 2 events in 1 batches"),
        "{delivered}"
    );
    assert_eq!(
        Spool::read_only(home.path()).delivery_states().unwrap()[&0].attempts,
        1
    );
    assert!(
        !relay(home.path(), &["requeue", "--batch", "0"])
            .status
            .success()
    );
    receiver.stop();
}

/// One session log whose final record was cut short beside a whole one. The
/// binary prints the report of what it relayed for the whole session, exits
/// non-zero, and names the skipped session and its file on stderr (NET-06).
/// The cut-short tail is written by the test; the receiver is a loopback
/// server that accepts every batch.
#[test]
fn a_damaged_session_log_is_named_after_the_report_of_what_was_relayed() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = capture_relay(bodies.clone());
    let home = tempfile::tempdir().unwrap();
    clear_personal_egress(home.path());
    record_crossing(home.path(), "whole", "https://www.example.com/whole");
    record_crossing(home.path(), "killed", "https://www.example.com/killed");
    std::fs::OpenOptions::new()
        .append(true)
        .open(home.path().join("sessions/killed.ndjson"))
        .unwrap()
        .write_all(br#"{"seq":1,"event":"crossing_obs"#)
        .unwrap();

    let output = relay(home.path(), &["--receiver", &receiver.url()]);
    receiver.stop();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("projected 1 of 1 sessions"),
        "the report covers the session that read: {stdout}"
    );
    assert!(stderr.contains("session killed was skipped"), "{stderr}");
    assert!(stderr.contains("killed.ndjson"), "{stderr}");
    let sent: Vec<String> = bodies
        .lock()
        .unwrap()
        .iter()
        .flat_map(|body| body["events"].as_array().unwrap().clone())
        .filter_map(|event| event["content_url"].as_str().map(str::to_owned))
        .collect();
    assert!(!sent.is_empty(), "the whole session was delivered");
    assert!(
        sent.iter().all(|url| url.ends_with("/whole")),
        "nothing of the damaged log left: {sent:?}"
    );
}

/// A queue line 0.3.4 or earlier wrote, with or without
/// `directory_selection` and with or without its final newline, is refused
/// by every command that reads the spool. No byte in the home changes: the
/// line is not taken for an enqueue cut short and truncated, and no
/// `delivery.lock` is created. `sessions/` exists beforehand, as in a home
/// that has recorded a session, because `doctor` creates it.
#[test]
fn every_command_refuses_a_spool_line_written_before_indices_and_changes_nothing() {
    let receiver = "http://127.0.0.1:1";
    let commands: [&[&str]; 5] = [
        &["status"],
        &["doctor"],
        &["relay", "--receiver", receiver],
        &["relay", "--dry-run", "--receiver", receiver],
        &["relay", "requeue"],
    ];
    for directory_selection in [true, false] {
        for newline in [true, false] {
            for args in commands {
                let home = tempfile::tempdir().unwrap();
                let spool = home.path().join("relay/spool");
                std::fs::create_dir_all(&spool).unwrap();
                std::fs::create_dir_all(home.path().join("sessions")).unwrap();
                let mut line = json!({
                    "origin": "earlier relay",
                    "document": {"events": [{"id": "6f1c1c52-5d9e-4a5e-9d7e-2b0f3f1d7a10"}]},
                });
                if directory_selection {
                    line["directory_selection"] = json!(true);
                }
                let queue = format!("{line}{}", if newline { "\n" } else { "" });
                std::fs::write(spool.join("outbound.ndjson"), queue).unwrap();
                let before = home_snapshot(home.path());

                let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
                    .args(args)
                    .env("COMMONMEASURE_HOME", home.path())
                    .output()
                    .expect("the binary should run");
                let case = format!(
                    "{args:?}, directory_selection {directory_selection}, newline {newline}"
                );
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    text.contains(
                        "spool line 0 (counted from zero) was written by commonmeasure 0.3.4 or \
                         earlier"
                    ) && text.contains("(CHANGELOG.md, 0.4.1, Upgrading)"),
                    "{case}: {text}"
                );
                let reads_only = matches!(args[0], "status" | "doctor");
                assert_eq!(output.status.success(), reads_only, "{case}: {text}");
                if reads_only {
                    assert!(
                        text.contains(
                            "refused spool: 0 batches are queued or dead in lines with an \
                             index; 1 line without an index is not read"
                        ),
                        "{case}: {text}"
                    );
                }
                assert!(
                    home_snapshot(home.path()) == before,
                    "{case}: the home is left as found"
                );
            }
        }
    }
}
