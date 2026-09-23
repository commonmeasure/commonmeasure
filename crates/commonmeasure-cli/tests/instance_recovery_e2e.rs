//! Kill and recover against a running Common Measure Hub: an instance's
//! process is killed with a report outstanding, a new process recovers
//! delivery through the hub, and the killed session's record is checked after
//! exit. One test, ignored by default because it needs a hub and the network.
//! The offline tests of the same properties, each against a stated double,
//! are in `crates/commonmeasure-relay/tests/relay.rs`,
//! `crates/commonmeasure-relay/tests/instance_registration.rs` and
//! `crates/commonmeasure-harness/src/observations.rs`.
#![cfg(unix)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use commonmeasure_http::{Request, Response, Server, ServerHandle, send};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn commonmeasure(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .env_remove("AGENT_SESSION_ID")
        .output()
        .expect("the binary should run")
}

/// The instance's process: `commonmeasure mcp` for one session, driven over
/// stdio as a host drives it and kept running between requests so that it
/// can be killed.
struct HostProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl HostProcess {
    fn start(home: &Path, session: &str, cwd: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .current_dir(cwd)
            .args(["mcp", "--host", "claude-code", "--session", session])
            .env("COMMONMEASURE_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server should start");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut process = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let initialised = process.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "instance-recovery-e2e", "version": "0"}}),
        );
        assert!(initialised.get("result").is_some(), "{initialised}");
        process
    }

    /// Send one request and read until its answer.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .expect("write");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                self.stdout.read_line(&mut line).expect("read"),
                0,
                "the server closed its output before answering {method}"
            );
            if let Ok(answer) = serde_json::from_str::<Value>(&line)
                && answer["id"] == json!(id)
            {
                return answer;
            }
        }
    }

    fn fetch(&mut self, url: &str) -> Value {
        self.request(
            "tools/call",
            json!({"name": "context_fetch", "arguments": {"url": url}}),
        )["result"]
            .clone()
    }

    fn observe(&mut self, observation: Value) -> Value {
        self.request("commonmeasure/observe", observation)
    }

    /// `SIGKILL`: `Child::kill` sends it on Unix. The process gets no chance
    /// to close, flush or record anything.
    fn kill(mut self) -> std::process::ExitStatus {
        self.child.kill().expect("kill");
        self.child.wait().expect("wait")
    }
}

/// The member of a tool result, whether the server returned it as structured
/// content or as JSON text.
fn result_member(result: &Value, member: &str) -> Value {
    if !result["structuredContent"][member].is_null() {
        return result["structuredContent"][member].clone();
    }
    result["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .map_or(Value::Null, |body| body[member].clone())
}

/// The receiver acceptances the hub reads behind an instance's one duty.
fn acceptances(read: &Value) -> usize {
    read["duties"][0]["receiver_acceptances"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Whether the hub reads every event of the instance's one duty as accepted:
/// at least one acceptance, their `events` summing to the duty's `delivered`
/// count, and none queued. No contract says how many documents the hub sends
/// one session's events in, so the count of acceptances is not asserted.
fn accepted_whole(read: &Value) -> bool {
    let duty = &read["duties"][0];
    let accepted: Option<u64> = duty["receiver_acceptances"]
        .as_array()
        .filter(|acceptances| !acceptances.is_empty())
        .and_then(|acceptances| {
            acceptances
                .iter()
                .map(|acceptance| acceptance["events"].as_u64())
                .sum()
        });
    accepted.is_some()
        && accepted == duty["events"]["delivered"].as_u64()
        && duty["events"]["queued"].as_u64() == Some(0)
}

/// Whether the printed summary carries `closure` as a null member: present,
/// so an answer that dropped the member does not pass for one that holds no
/// closure.
fn closure_null(read: &Value) -> bool {
    read.get("closure").is_some_and(Value::is_null)
}

/// An identifier in UUID form for a host observation, unique to this run.
fn identifier(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let hex = format!("{:x}", Sha256::digest(format!("{label}-{nanos}")));
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn tool_text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn session_records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join("sessions").join(format!("{session}.ndjson")))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// What stands at the hub's public origin. It passes every request to the
/// hub's listener and the hub's answer back, unchanged, so the edge and the
/// hub see each other's bytes. While `hold` is set it withholds the hub's
/// answer to a telemetry post until `release`, records that answer's HTTP
/// status in `held_status` and says in `held` that the hub has answered: the
/// moment at which the relay has claimed the batch, the hub has answered it
/// and the relay has not been told. It decides nothing and answers nothing
/// itself.
struct Forwarder {
    server: ServerHandle,
    hold: Arc<AtomicBool>,
    held: Arc<AtomicBool>,
    /// The hub's status for the withheld answer; 0 until one is withheld.
    held_status: Arc<AtomicU16>,
    release: Arc<AtomicBool>,
}

impl Forwarder {
    fn start(public: &str, listener: &str) -> Self {
        let bind = public
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_owned();
        let listener = listener.trim_end_matches('/').to_owned();
        let hold = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicBool::new(false));
        let held_status = Arc::new(AtomicU16::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let (holding, reached, released) = (hold.clone(), held.clone(), release.clone());
        let answered = held_status.clone();
        let server = Server::bind(&bind)
            .unwrap_or_else(|error| panic!("bind the hub's public origin {bind}: {error:#}"))
            .spawn(move |request: Request| {
                let telemetry_post =
                    request.method == "POST" && request.target.starts_with("/api/v1/telemetry");
                let url = format!("{listener}{}", request.target);
                let mut answer = match send(&url, request) {
                    Ok(answer) => answer,
                    Err(error) => return Response::text(502, &format!("{error:#}")),
                };
                // The body was read whole, so the hub's framing no longer
                // describes it; the server writes its own.
                answer.headers.remove("Transfer-Encoding");
                if telemetry_post && holding.swap(false, Ordering::SeqCst) {
                    answered.store(answer.status, Ordering::SeqCst);
                    reached.store(true, Ordering::SeqCst);
                    let waiting = Instant::now();
                    while !released.load(Ordering::SeqCst)
                        && waiting.elapsed() < Duration::from_secs(60)
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                answer
            })
            .expect("the forwarder serves");
        Self {
            server,
            hold,
            held,
            held_status,
            release,
        }
    }
}

/// Roadmap stage 2's kill-and-recover property on the first route, where the
/// hub is the durable service. Against a running hub, by this binary:
///
/// - instance A's host process (`commonmeasure mcp`) fetches a page, records
///   a context entry and an output, and is killed with `SIGKILL` before any
///   relay. The hub reads the instance `active` and its duty `outstanding`
///   with no event. A new `commonmeasure relay` from the same home delivers
///   the killed session's events with their `instance` reference. The killed
///   session's record reads whole and `commonmeasure inspect` joins the output
///   to its acquisition. The output is a string this test supplies as the
///   host; no output artefact exists. The check on it shows that the
///   `output_hash` the killed process was given is on disk after the kill, is
///   read back by another process and resolves to the `crossing_mediated`
///   whose `content_hash` equals the value the killed process returned. It is
///   independent of the killed process's memory, not of its claims: the page
///   is not fetched a second time;
/// - the onward double refuses every delivery with 503 until the test tells
///   it to accept, and keeps the `session_id` of each document it refused. A
///   new process closes instance A with the record's digest. Once the double
///   has refused a document carrying A's session, the hub's read is `closed`
///   with the custody receipt, the duty `outstanding` and no acceptance:
///   ended and sent, not accepted. A refusal of an earlier run's events, which
///   a reused database routes to this owner by their host, does not count.
///   The test then tells the double to accept; the hub retries a refused
///   delivery after one minute, and the duty reads `delivered` with
///   acceptances whose `events` sum to the duty's `delivered` count and none
///   queued. `closed` is asserted from the hub's reads, not only from the
///   close answer;
/// - instance B's `commonmeasure relay` is killed with `SIGKILL` after it
///   claimed the batch and the hub answered it 201, and before that answer
///   reached it. The forwarder records the status and the test asserts it,
///   and asserts that the relay ended by signal 9. A relay before the claim's
///   persisted deadline sends nothing; one at the deadline sends the same
///   event ids, the hub creates none, and the onward receiver accepts each
///   event once. While B is active the double has accepted its events and
///   the duty still reads `outstanding`: acceptance without an end is not
///   `delivered`. Nobody closes B: its window passes, the hub reads it
///   `expired` with `closure` null, and the duty then reads `delivered`;
/// - after B's window the registered session's fetch is refused
///   `instance_expired` while the cached source policy still refuses its
///   denied host in that session and in an unregistered one, whose admitted
///   fetch proceeds. The applied envelope has not expired in this run, so it
///   shows a cached policy enforced beside expired authority, not a stale one;
///   the stale case is the offline test
///   `an_expired_instance_stops_its_session_while_a_stale_policy_still_rules`;
/// - last, a final record cut short is written to A's log by the test, since
///   `SIGKILL` between two synced appends leaves none. `inspect` fails naming
///   the file. `relay` skips the session by name, projects nothing and fails
///   the run. No batch is queued beforehand, which the test asserts, and all
///   of A's events were delivered in step 4, so "nothing more is recorded
///   delivered" shows only that the skip sends nothing of its own. That the
///   whole records of a damaged log are neither spooled nor sent is the
///   offline test
///   `a_cut_short_final_record_is_skipped_by_name_and_the_other_session_is_delivered`.
///
/// Two loopback processes of the test stand beside the binary. The onward
/// receiver is a **test double**: it answers `POST /events` with 503 until
/// told to accept, then with the Content Telemetry acceptance body, and keeps
/// what it accepted. It is not oa-server, a reference receiver or a
/// publisher's system. The forwarder stands at the
/// hub's public origin and passes every request and answer through unchanged;
/// it exists so the relay can be killed at a known point of one exchange.
/// The run is integration evidence for this edge and the hub on loopback.
///
/// To run it (it takes about four minutes and fetches `https://example.com/`):
///
/// 1. Start a Common Measure Hub server (registration increment 3 or later)
///    on a loopback listener, with its database migrated and an organisation
///    whose owner has a session, `DEV_MODE=true`,
///    `DEV_ONWARD_LOOPBACK='127.0.0.1:*'`, `BIND_ADDRESS=127.0.0.1`, and
///    `IDENTITY_ORIGIN` set to a second loopback URL with a free port: the
///    public origin, where the test's forwarder listens.
/// 2. Export `COMMONMEASURE_TEST_HUB` (that public origin, e.g.
///    `http://127.0.0.1:18292`), `COMMONMEASURE_TEST_HUB_LISTENER` (where the
///    server listens, e.g. `http://127.0.0.1:18291`) and
///    `COMMONMEASURE_TEST_HUB_SESSION` (the owner's raw session token).
/// 3. `cargo test -p commonmeasure-cli --test instance_recovery_e2e -- --ignored --nocapture`
///
/// Each run's session names carry a suffix unique to the run, so it can be
/// repeated on one hub database; on a database where earlier runs' events
/// are refused ahead of A's for longer than the wait, the test fails instead
/// of counting their refusal as A's.
///
/// It prints a transcript with every secret redacted.
#[test]
#[ignore = "requires a running hub server and the network (see the doc comment)"]
fn a_real_hub_recovers_delivery_after_the_instance_and_the_relay_are_killed() {
    let variable = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is not set; see the doc comment"))
            .trim_end_matches('/')
            .to_owned()
    };
    let hub_url = variable("COMMONMEASURE_TEST_HUB");
    let listener = variable("COMMONMEASURE_TEST_HUB_LISTENER");
    let cookie = format!(
        "__Host-session={}",
        variable("COMMONMEASURE_TEST_HUB_SESSION")
    );
    let mut forwarder = Forwarder::start(&hub_url, &listener);

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path().canonicalize().unwrap();
    let scratch = home.path().display().to_string();
    let work_text = work_dir.display().to_string();
    // The hub derives a session's wire identity from its name and holds a
    // session to one agent, and each run enrols a new key, so the names carry
    // a suffix unique to the run. The transcript shows them without it.
    let suffix = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis())
    );
    let session_a = format!("kill-1-{suffix}");
    let session_b = format!("kill-2-{suffix}");
    let session_plain = format!("plain-{suffix}");
    let redact = |text: &str| {
        text.replace(&scratch, "<home>")
            .replace(&work_text, "<work>")
            .replace(&format!("-{suffix}"), "")
    };
    let owner = |method: &str, path: &str, body: Option<Value>| -> (u16, Value) {
        let mut request = match body {
            Some(body) => Request::post(path, body.to_string().into_bytes(), "application/json"),
            None => Request::get(path),
        };
        request.method = method.to_owned();
        request.headers.set("Cookie", &cookie);
        let response = send(&format!("{hub_url}{path}"), request).expect("the hub answers");
        (
            response.status,
            serde_json::from_slice(&response.body).unwrap_or(Value::Null),
        )
    };
    let run = |args: &[&str]| -> (bool, Value, String) {
        let output = commonmeasure(home.path(), args);
        (
            output.status.success(),
            serde_json::from_slice(&output.stdout).unwrap_or(Value::Null),
            redact(&String::from_utf8_lossy(&output.stderr)),
        )
    };
    let hours = |n: i64| (chrono::Utc::now() + chrono::Duration::hours(n)).to_rfc3339();
    // What `instance status` prints, which is the hub's answer as the edge
    // retained it: the instance's status, its closure with the custody
    // receipt where one is held, and its one duty.
    let hub_read_unprinted = |instance: &str| -> Value {
        let (ok, standing, stderr) = run(&["instance", "status", instance]);
        assert!(ok, "{stderr}");
        standing
    };
    let print_read = |standing: &Value| {
        let duty = &standing["duties"][0];
        println!(
            "instance status -> status={} closure receipt={} duty state={} events={} \
             receiver_acceptances={}",
            standing["status"],
            if standing["closure"]["receipt"].is_null() {
                "none"
            } else {
                "held"
            },
            duty["state"],
            duty["events"],
            duty["receiver_acceptances"].as_array().map_or(0, Vec::len)
        );
    };
    let hub_read = |instance: &str| -> Value {
        let standing = hub_read_unprinted(instance);
        print_read(&standing);
        standing
    };
    let until_delivered = |instance: &str| -> Value {
        // The hub's onward worker ticks every 15 seconds and retries a
        // refused delivery one minute after the attempt.
        // Read every five seconds; only the last read is printed.
        let mut last = hub_read_unprinted(instance);
        let mut reads_before = 0;
        for _ in 0..20 {
            if last["duties"][0]["state"] == "delivered" {
                break;
            }
            std::thread::sleep(Duration::from_secs(5));
            last = hub_read_unprinted(instance);
            reads_before += 1;
        }
        if reads_before > 0 {
            println!(
                "({reads_before} reads at five-second intervals did not read `delivered`; then:)"
            );
        }
        print_read(&last);
        last
    };
    let register =
        |session: &str, delegation: &str, acceptance: &str, minutes: &str, duty: &str| {
            let (ok, registered, stderr) = run(&[
                "instance",
                "register",
                "--session",
                session,
                "--delegation",
                delegation,
                "--acceptance",
                acceptance,
                "--host",
                "claude-code",
                "--host-version",
                "2.1",
                "--purpose",
                "research",
                "--expires-in-minutes",
                minutes,
                "--duty",
                duty,
            ]);
            assert!(ok, "{stderr}");
            println!(
                "instance register --session {} --expires-in-minutes {minutes} --duty \
             owner:<owner> -> http_status={} revision={} status={} duty state={}",
                redact(session),
                registered["operation"]["http_status"],
                registered["revision"],
                registered["status"],
                registered["duties"][0]["state"]
            );
            assert_eq!(registered["duties"][0]["state"], "outstanding");
            registered["instance"]
                .as_str()
                .expect("instance")
                .to_owned()
        };
    let relay = |session: &str| -> (bool, String) {
        let output = commonmeasure(home.path(), &["relay", "--session", session]);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), redact(&text))
    };
    let delivered_ids = || -> Vec<String> {
        std::fs::read_to_string(home.path().join("relay/delivered.idx"))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    };

    let page = "https://example.com/";
    let denied_page = "https://example.org/";

    // The onward receiver: a test double, as the doc comment says.
    // It refuses until `onward_accepting` is set, counting each refusal and
    // keeping the `session_id` of each refused document, and keeps only the
    // documents it accepted.
    let onward_bodies = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let onward_accepting = Arc::new(AtomicBool::new(false));
    let onward_refusals = Arc::new(AtomicUsize::new(0));
    let onward_refused_sessions = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let mut onward = {
        let bodies = onward_bodies.clone();
        let (accepting, refusals) = (onward_accepting.clone(), onward_refusals.clone());
        let refused_sessions = onward_refused_sessions.clone();
        Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                if !accepting.load(Ordering::SeqCst) {
                    refused_sessions
                        .lock()
                        .unwrap()
                        .push(body["session_id"].clone());
                    refusals.fetch_add(1, Ordering::SeqCst);
                    return Response::text(503, "the test double is not accepting yet");
                }
                let events = body["events"].as_array().map_or(0, Vec::len);
                bodies.lock().unwrap().push(body);
                Response::json(
                    200,
                    &json!({"status": "ok", "events_created": events}).to_string(),
                )
            })
            .unwrap()
    };
    let onward_url = format!("{}/events", onward.url().trim_end_matches('/'));

    println!("## 1. Policy, a managed edge, an owner with an onward destination, authority");
    let (status, minted) = owner(
        "POST",
        "/api/v1/enrolment/tokens",
        Some(json!({"name": "kill-recover-run"})),
    );
    assert_eq!(status, 201, "{minted}");
    let token = minted["value"].as_str().expect("token").to_owned();
    let (status, published) = owner(
        "POST",
        "/api/v1/policy/revisions",
        Some(json!({"policy": {
            "policy_mode": "strict",
            "constraints": [{"kind": "denied_source_host", "host": "example.org"}],
            "scopes": [{"match": work_text, "engagement": "kill-recover-run",
                "allow_telemetry_egress": true}],
        }})),
    );
    assert_eq!(status, 201, "{published}");
    println!(
        "POST /api/v1/policy/revisions -> 201 (strict; example.org denied; scope <work>, \
         engagement kill-recover-run, telemetry egress allowed)"
    );
    let connected = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", &token, "--managed"],
    );
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    let synced = commonmeasure(home.path(), &["policy", "sync"]);
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    println!(
        "commonmeasure connect <hub> --token et_<redacted> --managed -> ok; policy sync -> ok"
    );
    let key_id = commonmeasure_harness::enrolled_key_id(home.path()).expect("an enrolled key id");
    // An earlier run that failed part-way leaves its owner, which holds the
    // page's host; it is removed so the run can be repeated on one database.
    let (_, owners) = owner("GET", "/api/v1/owners", None);
    for left in owners.as_array().into_iter().flatten() {
        if left["name"] == "Kill and recover run owner"
            && let Some(id) = left["id"].as_str()
        {
            let _ = owner("DELETE", &format!("/api/v1/owners/{id}"), None);
        }
    }
    let (status, created) = owner(
        "POST",
        "/api/v1/owners",
        Some(json!({"name": "Kill and recover run owner"})),
    );
    assert_eq!(status, 201, "{created}");
    let owner_id = created["id"].as_str().expect("owner id").to_owned();
    let (status, claimed) = owner(
        "POST",
        &format!("/api/v1/owners/{owner_id}/hosts"),
        Some(json!({"host": "example.com", "basis": "kill and recover run"})),
    );
    assert_eq!(status, 201, "{claimed}");
    let (status, destination) = owner(
        "PUT",
        &format!("/api/v1/owners/{owner_id}/destination"),
        Some(json!({"url": onward_url, "credential": null})),
    );
    assert_eq!(
        status, 200,
        "the hub did not accept the loopback onward destination; it needs \
         DEV_ONWARD_LOOPBACK: {destination}"
    );
    println!(
        "POST /api/v1/owners -> 201; hosts (example.com) -> 201; PUT destination (the loopback \
         test double, http) -> 200"
    );
    let duty = format!("owner:{owner_id}");
    let (status, delegated) = owner(
        "POST",
        "/api/v1/instance-delegations",
        Some(json!({
            "edge_key_id": key_id, "operator_id": null, "sponsor": "research-desk",
            "work": [
                {"kind": "session", "namespace": "claude-code", "id": session_a},
                {"kind": "session", "namespace": "claude-code", "id": session_b},
            ],
            "purpose": "research", "allow_children": false, "expires_at": hours(12),
        })),
    );
    assert_eq!(status, 201, "{delegated}");
    let delegation = delegated["delegation"].as_str().expect("delegation");
    let (status, accepted) = owner(
        "POST",
        "/api/v1/instance-acceptances",
        Some(json!({
            "service": "commonmeasure-hub", "custody": "references_only",
            "retention_until": hours(24 * 90), "access": "owners", "deletion": "on_request",
            "succession": "none", "expires_at": hours(24 * 30), "reporting": true,
        })),
    );
    assert_eq!(status, 201, "{accepted}");
    let acceptance = accepted["acceptance"].as_str().expect("acceptance");
    println!(
        "POST /api/v1/instance-delegations (sessions kill-1, kill-2) -> 201; POST \
         /api/v1/instance-acceptances (service commonmeasure-hub, reporting true) -> 201"
    );

    println!("\n## 2. Instance A: its host process fetches, records an output, and is killed");
    let instance_a = register(&session_a, delegation, acceptance, "30", &duty);
    let answer_text = "example.com is a domain reserved for use in documentation examples.";
    let output_hash = format!("sha256:{:x}", Sha256::digest(answer_text.as_bytes()));
    let mut host = HostProcess::start(home.path(), &session_a, &work_dir);
    let started = host.observe(json!({"event": "observations_started"}));
    assert!(started.get("result").is_some(), "{started}");
    let fetched = host.fetch(page);
    assert_ne!(fetched["isError"], json!(true), "{fetched}");
    let handle = result_member(&fetched, "acquisition_id");
    let content_hash = result_member(&fetched, "content_hash");
    assert!(
        handle.is_string(),
        "the fetch returned no handle: {fetched}"
    );
    let generation = identifier("generation");
    let entered = host.observe(json!({"event": "context_entered", "acquisition_id": handle,
        "generation_id": generation, "representation_hash": content_hash}));
    assert!(entered.get("result").is_some(), "{entered}");
    let associated = host.observe(json!({"event": "output_associated",
        "generation_id": generation, "output_id": identifier("output"),
        "output_hash": output_hash, "acquisition_ids": [handle]}));
    assert!(associated.get("result").is_some(), "{associated}");
    println!(
        "commonmeasure mcp --session kill-1 (pid <pid>): observations_started; context_fetch \
         {page} -> acquisition <handle>, content_hash {}; context_entered; output_associated \
         output_hash {output_hash}",
        content_hash.as_str().unwrap_or_default()
    );
    let exit = host.kill();
    {
        use std::os::unix::process::ExitStatusExt as _;
        println!(
            "kill -KILL <pid> -> the process ended by signal {}; nothing was relayed",
            exit.signal().unwrap_or(0)
        );
        assert_eq!(exit.signal(), Some(9));
    }

    println!("\n## 3. After the kill: the hub's read, and the record on disk");
    let after_kill = hub_read(&instance_a);
    assert_eq!(after_kill["status"], "active", "the instance is not closed");
    assert!(closure_null(&after_kill), "{after_kill}");
    assert_eq!(after_kill["duties"][0]["state"], "outstanding");
    let record_path = home
        .path()
        .join("sessions")
        .join(format!("{session_a}.ndjson"));
    let record_bytes = std::fs::read(&record_path).unwrap();
    assert_eq!(record_bytes.last(), Some(&b'\n'), "no partial tail");
    let records = session_records(home.path(), &session_a);
    assert_eq!(
        records.len(),
        record_bytes.iter().filter(|byte| **byte == b'\n').count(),
        "every line is a whole record"
    );
    let stamped = records
        .iter()
        .filter(|record| record["payload"]["instance"]["id"] == json!(instance_a))
        .count();
    let spooled_before = commonmeasure_relay::spool::Spool::read_only(home.path())
        .entries()
        .expect("the spool reads")
        .len();
    println!(
        "sessions/kill-1.ndjson: {} records, every line whole, the file ends with a newline; {} \
         carry instance <A> revision 1; batches spooled: {}; event ids recorded delivered: {}",
        records.len(),
        stamped,
        spooled_before,
        delivered_ids().len()
    );
    assert_eq!((spooled_before, delivered_ids().len()), (0, 0));

    println!("\n## 4. A new process relays the killed session from the same home");
    let (ok, relayed) = relay(&session_a);
    print!("{relayed}");
    assert!(ok, "{relayed}");
    let sent_a = delivered_ids();
    assert!(!sent_a.is_empty());
    let spooled = commonmeasure_relay::spool::Spool::read_only(home.path())
        .entries()
        .expect("the spool reads");
    let spooled_events: Vec<&Value> = spooled
        .iter()
        .flat_map(|(_, entry)| entry.document["events"].as_array().into_iter().flatten())
        .collect();
    // The member travels on content events; the turn boundary carries none.
    let content_events: Vec<&&Value> = spooled_events
        .iter()
        .filter(|event| event["content_url"].is_string())
        .collect();
    assert!(!content_events.is_empty());
    assert!(
        content_events
            .iter()
            .all(|event| event["instance"]["id"] == json!(instance_a)),
        "every queued content event carries the killed instance's reference"
    );
    println!(
        "the edge's spool: {} events queued for the hub, {} of them content events, each with \
         instance <A> revision {}; {} event ids recorded delivered",
        spooled_events.len(),
        content_events.len(),
        content_events[0]["instance"]["revision"],
        sent_a.len()
    );
    // The session's identity on the wire, as the relay queued it. The hub
    // keeps it and sends it onward as the document's `session_id`, and the
    // run's suffix makes it A's alone, so a refused document carrying it
    // holds A's events and no earlier run's.
    let mut wire_sessions: Vec<&Value> = spooled
        .iter()
        .filter(|(_, entry)| {
            entry.document["events"]
                .as_array()
                .is_some_and(|events| events.iter().any(|event| event["content_url"].is_string()))
        })
        .map(|(_, entry)| &entry.document["session_id"])
        .collect();
    wire_sessions.dedup();
    assert_eq!(wire_sessions.len(), 1, "one session was relayed");
    let wire_session_a = wire_sessions[0].clone();
    assert!(wire_session_a.is_string(), "{wire_session_a}");
    let relayed_read = hub_read(&instance_a);
    assert_eq!(relayed_read["status"], "active");
    assert_eq!(
        relayed_read["duties"][0]["state"], "outstanding",
        "an active instance's duty is outstanding whatever has been delivered"
    );

    println!("\n## 5. Provenance after exit: the killed session's record and its output");
    let inspected = commonmeasure(
        home.path(),
        &["inspect", record_path.to_str().expect("a UTF-8 path")],
    );
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let handle_text = handle.as_str().unwrap_or_default();
    let dossier =
        redact(&String::from_utf8_lossy(&inspected.stdout)).replace(handle_text, "<handle>");
    for line in dossier.lines().filter(|line| {
        line.starts_with("session source record")
            || line.starts_with("acquisition ")
            || line.contains("acquisitions")
            || line.starts_with("output_associated")
    }) {
        println!("inspect: {}", line.chars().take(400).collect::<String>());
    }
    let summary = commonmeasure_harness::session::host_observations(&records, None);
    assert_eq!(
        (
            summary.acquisitions,
            summary.context_entries,
            summary.outputs,
            summary.output_associations,
            summary.unresolved_associations
        ),
        (1, 1, 1, 1, 0)
    );
    let recorded_output = records
        .iter()
        .find(|record| record["event"] == "output_associated")
        .expect("the output observation");
    // `answer_text` is this test's own string, given to the killed process
    // as the host's output: the comparison shows the hash on disk is the one
    // that process was given, and nothing about an output artefact.
    let recomputed = format!("sha256:{:x}", Sha256::digest(answer_text.as_bytes()));
    assert_eq!(recorded_output["payload"]["output_hash"], json!(recomputed));
    let cited = records
        .iter()
        .find(|record| {
            record["event"] == "crossing_mediated" && record["payload"]["acquisition_id"] == handle
        })
        .expect("the acquisition the output cites");
    // Compared with the killed process's own answer; the page is not fetched
    // again.
    assert_eq!(cited["payload"]["content_hash"], content_hash);
    println!(
        "the output is a string the test supplied as host; the output_hash read back from disk \
         is the hash of that string: true; its one association resolves to the \
         crossing_mediated of {page} whose content_hash equals the killed process's own answer: \
         true; unresolved associations: {}",
        summary.unresolved_associations
    );
    println!(
        "seal and chain: a session record carries neither (they belong to a published run), so \
         the checks above are the ones that exist for it"
    );

    println!(
        "\n## 6. A new process closes instance A; the duty before and after the receiver's \
         acceptance"
    );
    let record_digest = format!("sha256:{:x}", Sha256::digest(&record_bytes));
    let (ok, closed, stderr) = run(&[
        "instance",
        "close",
        &instance_a,
        "--outcome",
        "failed",
        "--evidence",
        &format!("sessions/{session_a}.ndjson={record_digest}"),
    ]);
    assert!(ok, "{stderr}");
    println!(
        "instance close <A> --outcome failed --evidence sessions/kill-1.ndjson=<its sha256> -> \
         http_status={} status={} duty state={}",
        closed["operation"]["http_status"], closed["status"], closed["duties"][0]["state"]
    );
    assert_eq!(closed["status"], "closed");
    // The hub's worker sends A's events within a tick of step 4, and the
    // double refuses them. The read is taken once a refused document of A's
    // session is counted, so it is the state the hub's rule turns on: ended,
    // sent, not accepted. A refusal of an earlier run's events, which a
    // reused database routes to this owner by their host, does not count.
    // Nothing between the relay and the flip below may wait beyond this
    // loop: the hub retries one minute after a refusal and two after a
    // second, and the reads after the flip allow for the first only.
    let refused_of_a = || {
        onward_refused_sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|refused| **refused == wire_session_a)
            .count()
    };
    let waiting = Instant::now();
    while refused_of_a() == 0 {
        assert!(
            waiting.elapsed() < Duration::from_secs(45),
            "the hub never attempted the onward delivery of A's session"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    println!(
        "the onward test double has refused {} delivery attempt(s) with 503, {} of them of \
         session kill-1, and accepted none",
        onward_refusals.load(Ordering::SeqCst),
        refused_of_a()
    );
    let unaccepted_a = hub_read(&instance_a);
    assert_eq!(unaccepted_a["status"], "closed", "{unaccepted_a}");
    assert!(
        !unaccepted_a["closure"]["receipt"].is_null(),
        "the custody receipt: {unaccepted_a}"
    );
    assert_eq!(
        unaccepted_a["duties"][0]["state"], "outstanding",
        "closed and refused by the receiver is not delivered: {unaccepted_a}"
    );
    assert_eq!(acceptances(&unaccepted_a), 0, "{unaccepted_a}");
    assert!(onward_bodies.lock().unwrap().is_empty());
    onward_accepting.store(true, Ordering::SeqCst);
    println!(
        "the onward test double now accepts (the hub retries a refused delivery one minute after \
         the attempt)"
    );
    let delivered_a = until_delivered(&instance_a);
    assert_eq!(delivered_a["status"], "closed", "{delivered_a}");
    assert!(
        !delivered_a["closure"]["receipt"].is_null(),
        "the custody receipt: {delivered_a}"
    );
    assert_eq!(
        delivered_a["duties"][0]["state"], "delivered",
        "{delivered_a}"
    );
    assert!(accepted_whole(&delivered_a), "{delivered_a}");

    println!("\n## 7. Instance B: the relay is killed between the hub's acceptance and its answer");
    let instance_b = register(&session_b, delegation, acceptance, "2", &duty);
    let registered_b = Instant::now();
    let mut host = HostProcess::start(home.path(), &session_b, &work_dir);
    let fetched = host.fetch(page);
    assert_ne!(fetched["isError"], json!(true), "{fetched}");
    let _ = host.kill();
    println!("commonmeasure mcp --session kill-2: context_fetch {page} -> ok; process killed");
    forwarder.hold.store(true, Ordering::SeqCst);
    let mut relaying = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["relay", "--session", &session_b])
        .env("COMMONMEASURE_HOME", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the relay starts");
    let waiting = Instant::now();
    while !forwarder.held.load(Ordering::SeqCst) {
        assert!(
            waiting.elapsed() < Duration::from_secs(45),
            "the relay never posted a batch"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    relaying.kill().expect("kill");
    let exit = relaying.wait().expect("wait");
    forwarder.release.store(true, Ordering::SeqCst);
    let claim = commonmeasure_relay::spool::Spool::read_only(home.path())
        .delivery_states()
        .expect("delivery state reads")
        .into_values()
        .find(|state| state.status == commonmeasure_relay::spool::DeliveryStatus::Queued)
        .expect("the claimed batch is still queued");
    let deadline = claim
        .next_attempt_at
        .expect("the claim's persisted deadline");
    let hub_status = forwarder.held_status.load(Ordering::SeqCst);
    {
        use std::os::unix::process::ExitStatusExt as _;
        println!(
            "commonmeasure relay --session kill-2: the forwarder passed the batch post to the hub, \
             recorded the hub's answer ({hub_status}) and withheld it from the relay; \
             kill -KILL <pid> -> ended by signal {}",
            exit.signal().unwrap_or(0)
        );
        assert_eq!(exit.signal(), Some(9));
    }
    assert_eq!(hub_status, 201, "the hub accepted the batch the relay lost");
    println!(
        "the edge's spool: the batch is queued, attempts={}, next attempt {}s after the claim, \
         no outcome recorded; event ids recorded delivered for kill-2: {}",
        claim.attempts,
        claim
            .last_attempt_at
            .map_or(-1, |claimed| (deadline - claimed).num_seconds()),
        delivered_ids().len() - sent_a.len()
    );
    assert_eq!(delivered_ids().len(), sent_a.len(), "the edge never learnt");
    let unlearnt = hub_read(&instance_b);
    assert_eq!(unlearnt["status"], "active");
    assert_eq!(unlearnt["duties"][0]["state"], "outstanding");
    let held_by_hub = |read: &Value| -> u64 {
        ["queued", "delivered", "dead", "unrouted"]
            .iter()
            .filter_map(|count| read["duties"][0]["events"][count].as_u64())
            .sum()
    };
    let events_b = held_by_hub(&unlearnt);
    assert!(events_b > 0, "the hub recorded the killed post: {unlearnt}");

    println!("\n## 8. A relay before the claim's deadline, then one at it");
    let (ok, early) = relay(&session_b);
    print!("{early}");
    assert!(ok, "{early}");
    assert_eq!(
        delivered_ids().len(),
        sent_a.len(),
        "nothing was sent early"
    );
    let wait = (deadline - chrono::Utc::now()).num_milliseconds().max(0) as u64 + 500;
    println!("(waiting {}s for the persisted deadline)", wait / 1000);
    std::thread::sleep(Duration::from_millis(wait));
    let (ok, recovered) = relay(&session_b);
    print!("{recovered}");
    assert!(ok, "{recovered}");
    assert!(
        recovered.contains("(0 new at the receiver)"),
        "the hub held every event already: {recovered}"
    );
    let sent_b = delivered_ids().len() - sent_a.len();
    assert_eq!(sent_b as u64, events_b);

    println!("\n## 9. No event twice");
    let recovered_read = hub_read(&instance_b);
    assert_eq!(
        held_by_hub(&recovered_read),
        events_b,
        "the redelivery added no event to the duty"
    );
    let all_ids = delivered_ids();
    let distinct: std::collections::BTreeSet<&String> = all_ids.iter().collect();
    assert_eq!(distinct.len(), all_ids.len());
    println!(
        "the hub's duty for <B> counts {events_b} events before and after the second post; the \
         edge records {} event ids delivered, each once",
        all_ids.len()
    );
    // The double has been accepting since step 6, so B's events are accepted
    // within a tick while B is still active.
    let mut accepted_active = recovered_read;
    if !accepted_whole(&accepted_active) {
        for _ in 0..8 {
            std::thread::sleep(Duration::from_secs(5));
            accepted_active = hub_read_unprinted(&instance_b);
            if accepted_whole(&accepted_active) {
                break;
            }
        }
        print_read(&accepted_active);
    }
    println!(
        "instance <B> is active, the onward test double has accepted its events, and the duty \
         reads {}",
        accepted_active["duties"][0]["state"]
    );
    assert_eq!(accepted_active["status"], "active", "{accepted_active}");
    assert!(accepted_whole(&accepted_active), "{accepted_active}");
    assert_eq!(
        accepted_active["duties"][0]["state"], "outstanding",
        "accepted by the receiver and not ended is not delivered: {accepted_active}"
    );

    println!("\n## 10. Nobody closes instance B: its window passes");
    let remaining = Duration::from_secs(125).saturating_sub(registered_b.elapsed());
    println!(
        "(waiting {}s for the two-minute window)",
        remaining.as_secs()
    );
    std::thread::sleep(remaining);
    let expired = until_delivered(&instance_b);
    assert_eq!(expired["status"], "expired", "{expired}");
    assert!(closure_null(&expired), "no closure was sent: {expired}");
    assert_eq!(expired["duties"][0]["state"], "delivered", "{expired}");
    let mut after_window = HostProcess::start(home.path(), &session_b, &work_dir);
    let refused_by_policy = after_window.fetch(denied_page);
    let stopped = after_window.fetch(page);
    let _ = after_window.kill();
    let mut unregistered = HostProcess::start(home.path(), &session_plain, &work_dir);
    let plain_refused = unregistered.fetch(denied_page);
    let plain_admitted = unregistered.fetch(page);
    let _ = unregistered.kill();
    let last_event = |session: &str, url: &str| -> String {
        session_records(home.path(), session)
            .iter()
            .rev()
            .find(|record| record["payload"]["url"] == json!(url))
            .and_then(|record| record["event"].as_str())
            .unwrap_or("none")
            .to_owned()
    };
    println!(
        "session kill-2 (instance <B>, expired): {denied_page} -> {}; {page} -> {} ({})",
        last_event(&session_b, denied_page),
        last_event(&session_b, page),
        if tool_text(&stopped).contains("instance_expired") {
            "instance_expired"
        } else {
            "another reason"
        }
    );
    println!(
        "session plain (no registration, same cached policy): {denied_page} -> {}; {page} -> {}",
        last_event(&session_plain, denied_page),
        last_event(&session_plain, page)
    );
    assert_eq!(refused_by_policy["isError"], json!(true));
    assert_eq!(last_event(&session_b, denied_page), "crossing_refused");
    assert!(
        tool_text(&stopped).contains("instance_expired"),
        "{stopped}"
    );
    assert_eq!(last_event(&session_b, page), "instance_refused");
    assert_eq!(plain_refused["isError"], json!(true));
    assert_eq!(last_event(&session_plain, denied_page), "crossing_refused");
    assert_ne!(plain_admitted["isError"], json!(true), "{plain_admitted}");
    assert_eq!(last_event(&session_plain, page), "crossing_mediated");

    println!("\n## 11. A final record cut short, written by the test to instance A's log");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&record_path)
        .unwrap()
        .write_all(br#"{"seq":99,"timestamp":"2026-09-18T00:00:00.000Z","event":"crossing_med"#)
        .unwrap();
    let inspected = commonmeasure(
        home.path(),
        &["inspect", record_path.to_str().expect("a UTF-8 path")],
    );
    let refusal = redact(&String::from_utf8_lossy(&inspected.stderr));
    println!(
        "inspect -> exit {}: {}",
        inspected.status.code().unwrap_or(-1),
        refusal.trim()
    );
    assert!(!inspected.status.success());
    // Nothing is due before this relay: every spooled batch was delivered in
    // steps 4 and 8, and the sessions of step 10 are never relayed. All of
    // kill-1's events went in step 4, so the unchanged delivered index below
    // shows that the skip sends nothing of its own and no more than that.
    let queued_before = commonmeasure_relay::spool::Spool::read_only(home.path())
        .delivery_states()
        .expect("delivery state reads")
        .into_values()
        .filter(|state| state.status == commonmeasure_relay::spool::DeliveryStatus::Queued)
        .count();
    assert_eq!(queued_before, 0, "no batch is queued before the relay");
    let before = delivered_ids().len();
    let (ok, refused) = relay(&session_a);
    println!("batches queued before the relay: {queued_before}");
    println!("relay --session kill-1 -> exit ok={ok}:");
    for line in refused.lines() {
        println!("  {line}");
    }
    assert!(!ok);
    assert!(refused.contains("session kill-1 was skipped"), "{refused}");
    assert!(refused.contains("kill-1.ndjson"), "{refused}");
    assert!(refused.contains("projected 0 of 0 sessions"), "{refused}");
    assert_eq!(delivered_ids().len(), before);

    // This run's content events, by the ids the edge queued for the hub.
    let content_ids: Vec<String> = commonmeasure_relay::spool::Spool::read_only(home.path())
        .entries()
        .expect("the spool reads")
        .iter()
        .flat_map(|(_, entry)| entry.document["events"].as_array().into_iter().flatten())
        .filter(|event| event["content_url"].is_string())
        .filter_map(|event| event["id"].as_str().map(str::to_owned))
        .collect();
    let onward_bodies = onward_bodies.lock().unwrap().clone();
    let onward_ids: Vec<String> = onward_bodies
        .iter()
        .flat_map(|body| body["events"].as_array().into_iter().flatten())
        .filter_map(|event| event["id"].as_str().map(str::to_owned))
        .collect();
    let times_sent_onward = |id: &String| onward_ids.iter().filter(|sent| *sent == id).count();
    let earlier_runs = onward_ids
        .iter()
        .filter(|id| !content_ids.contains(id))
        .count();
    println!(
        "\nthe onward test double refused {} delivery attempt(s) before it was told to accept, \
         then accepted {} document(s) holding {} events: this run's {} content events once \
         each: {}; events of earlier runs on the same database, routed to this owner by their \
         host: {}; none carries an instance member: {}",
        onward_refusals.load(Ordering::SeqCst),
        onward_bodies.len(),
        onward_ids.len(),
        content_ids.len(),
        content_ids.iter().all(|id| times_sent_onward(id) == 1),
        earlier_runs,
        onward_bodies
            .iter()
            .all(|body| !body.to_string().contains("\"instance\""))
    );

    // The owner is removed so the run can be repeated on the same database;
    // every read the assertions use has been taken.
    let _ = owner("DELETE", &format!("/api/v1/owners/{owner_id}"), None);
    let _ = commonmeasure(home.path(), &["disconnect"]);
    onward.stop();
    forwarder.server.stop();

    assert_eq!(content_ids.len(), 4, "two content events from each session");
    for id in &content_ids {
        assert_eq!(
            times_sent_onward(id),
            1,
            "event {id} was sent onward other than once"
        );
    }
    assert!(
        onward_bodies
            .iter()
            .all(|body| !body.to_string().contains("\"instance\"")),
        "the instance reference is not sent onward"
    );
}
