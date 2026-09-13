//! The relay driven as an operator uses it: evidence recorded by the real
//! hook path, the real binary projecting and delivering, and the console's
//! status block read back over real HTTP.
//!
//! The offline tests here use a real loopback receiver (`commonmeasure_http::Server`)
//! to observe the bytes the binary actually posts. The one test against the
//! real oa-server implementation is ignored by default so the offline gates
//! stay green; see `a_live_oa_server_accepts_the_projection` for how to run
//! it and `docs/knowledge-base/unverified-assumptions.md` for its standing.

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
    record_crossing_in(
        home.path(),
        "s-client",
        "/work/client-a",
        "https://www.example.com/client",
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
    drop(batches);

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
/// Content Telemetry receiver (oa-server) on a local socket. Ignored by
/// default so the offline gates stay green. To run it:
///
/// 1. Start a local oa-server with a provisioned demo database
///    (`~/ops/code/infrastructure`, `scripts/provision-local-demo.sh`,
///    `DEV_MODE=true`), which prints a platform write key and two publisher
///    read keys.
/// 2. Export `COMMONMEASURE_RELAY_TEST_KEY` (the `oat_pk_` write key),
///    `COMMONMEASURE_RELAY_TEST_READER_GUARDIAN` and
///    `COMMONMEASURE_RELAY_TEST_READER_TELEGRAPH` (the `oat_pub_` read keys), and
///    optionally `COMMONMEASURE_RELAY_TEST_RECEIVER` (default
///    `http://localhost:8080`).
/// 3. `cargo test -p commonmeasure-cli --test relay_e2e -- --ignored`
#[test]
#[ignore = "requires a local oa-server (see the doc comment for how to run it)"]
fn a_live_oa_server_accepts_the_projection_and_isolates_owners() {
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
        "wp07-{}-{}",
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
    std::fs::remove_file(home.path().join("relay/spool/outbound.ack")).unwrap();
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
