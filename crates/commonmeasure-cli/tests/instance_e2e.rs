//! Instance registration driven as an operator does it: the real binary
//! against a running Common Measure Hub. The offline tests of the client
//! and of the binding applied to a session are
//! `crates/commonmeasure-relay/tests/instance_registration.rs`, against a
//! test double; this file holds the tests against a real hub, ignored by
//! default. See `a_real_hub_registers_renews_revokes_and_closes` for how to
//! run it. `a_real_hub_reads_a_reporting_duty_after_relay_and_closure` is the
//! second such test: a reporting duty, the relay's delivery to the hub and
//! the hub's onward delivery. One offline test of the binary's printed
//! summary runs by default:
//! `a_closure_the_hub_never_received_prints_closure_null`.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use commonmeasure_http::{Request, Response, Server, send};
use serde_json::{Value, json};

fn commonmeasure(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .env_remove("AGENT_SESSION_ID")
        .output()
        .expect("the binary should run")
}

/// One `context_fetch` through the mediated server for `session`, as a host
/// makes it over stdio. Answers the tool result.
fn mediated_fetch(home: &Path, session: &str, url: &str) -> Value {
    mediated_fetch_in(home, session, url, None)
}

/// The same, with the server started in `cwd`: the working directory a
/// crossing records is what the source policy's scopes resolve, and so what
/// clears the crossing for the relay.
fn mediated_fetch_in(home: &Path, session: &str, url: &str, cwd: Option<&Path>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .args(["mcp", "--host", "claude-code", "--session", session])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server should start");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "instance-e2e", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "context_fetch", "arguments": {"url": url}}}),
        ] {
            writeln!(stdin, "{message}").expect("write");
        }
    }
    let output = child.wait_with_output().expect("the server exits");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|answer| answer["id"] == json!(2))
        .map(|answer| answer["result"].clone())
        .expect("an answer to the tool call")
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

fn session_records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join("sessions").join(format!("{session}.ndjson")))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The registration lifecycle against a running Common Measure Hub, driven
/// by this binary: `connect --managed`, a policy synchronisation, the
/// owner's delegation and custody acceptance, then `instance register`, a
/// replay under the same idempotency key, a mediated fetch stamped with the
/// instance reference after an online check, `instance renew`, an owner
/// revocation refusing the next fetch, and a second instance closed with its
/// custody receipt. Ignored by default because it needs a hub. To run it:
///
/// 1. Run a Common Measure Hub server (instance registration increment 1 or
///    later) with its database migrated, `IDENTITY_ORIGIN` set and its
///    public origin equal to the URL the edge is given, with an organisation
///    whose owner has a session.
/// 2. Export `COMMONMEASURE_TEST_HUB` (the server's base URL, e.g.
///    `http://127.0.0.1:8080`) and `COMMONMEASURE_TEST_HUB_SESSION` (the
///    owner's raw session token, sent as the `__Host-session` cookie).
/// 3. `cargo test -p commonmeasure-cli --test instance_e2e -- --ignored --nocapture`
///
/// It prints a transcript with every secret redacted.
#[test]
#[ignore = "requires a running hub server (see the doc comment for how to run it)"]
fn a_real_hub_registers_renews_revokes_and_closes() {
    let hub_url = std::env::var("COMMONMEASURE_TEST_HUB")
        .expect("COMMONMEASURE_TEST_HUB names the running hub")
        .trim_end_matches('/')
        .to_owned();
    let cookie = format!(
        "__Host-session={}",
        std::env::var("COMMONMEASURE_TEST_HUB_SESSION")
            .expect("COMMONMEASURE_TEST_HUB_SESSION is the owner's session token")
    );
    let home = tempfile::tempdir().unwrap();
    let scratch = home.path().display().to_string();
    let redact = |text: &str| text.replace(&scratch, "<home>");
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
    let show = |answer: &Value| {
        println!(
            "operation={} outcome={} http_status={} reason={} revision={} status={} \
             expires_at={} next_check={}",
            answer["operation"]["operation"],
            answer["operation"]["outcome"],
            answer["operation"]["http_status"],
            answer["operation"]["reason"],
            answer["revision"],
            answer["status"],
            answer["expires_at"],
            answer["next_check"]
        );
    };
    let hours = |n: i64| (chrono::Utc::now() + chrono::Duration::hours(n)).to_rfc3339();

    // A loopback page the mediated fetch reads, so the run needs no network.
    let mut page = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(|request| {
            if request.target == "/doc" {
                Response::text(200, "A page fetched under a registered instance.")
            } else {
                Response::text(404, "not found")
            }
        })
        .unwrap();
    let url = format!("{}/doc", page.url().trim_end_matches('/'));

    println!("## 1. Before enrolment, register refuses and names what is missing");
    let (ok, _, stderr) = run(&[
        "instance",
        "register",
        "--session",
        "run-1",
        "--delegation",
        "none",
        "--acceptance",
        "none",
        "--host",
        "claude-code",
        "--host-version",
        "0",
        "--purpose",
        "research",
    ]);
    assert!(!ok);
    assert!(stderr.contains("not enrolled"), "{stderr}");
    print!("{stderr}");

    println!(
        "\n## 2. The owner publishes a policy and mints a token; the edge connects as a managed edge"
    );
    let (status, minted) = owner(
        "POST",
        "/api/v1/enrolment/tokens",
        Some(json!({"name": "instance-run"})),
    );
    assert_eq!(status, 201, "{minted}");
    let token = minted["value"].as_str().expect("token").to_owned();

    let (status, published) = owner(
        "POST",
        "/api/v1/policy/revisions",
        Some(json!({"policy": {"policy_mode": "observe", "allow_private_hosts": true}})),
    );
    assert_eq!(status, 201, "{published}");
    println!("POST /api/v1/policy/revisions -> 201");
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", &token, "--managed"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("commonmeasure connect <hub> --token et_<redacted> --managed -> ok");
    let synced = commonmeasure(home.path(), &["policy", "sync"]);
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    let key_id = commonmeasure_harness::enrolled_key_id(home.path()).expect("an enrolled key id");

    println!("\n## 3. The owner delegates work to this edge key and records custody acceptance");
    let work = json!([
        {"kind": "session", "namespace": "claude-code", "id": "run-1"},
        {"kind": "session", "namespace": "claude-code", "id": "run-2"},
        {"kind": "job", "namespace": "context-job", "id": "job-1"},
    ]);
    let (status, delegated) = owner(
        "POST",
        "/api/v1/instance-delegations",
        Some(json!({
            "edge_key_id": key_id, "operator_id": null, "sponsor": "research-desk",
            "work": work, "purpose": "research", "allow_children": false,
            "expires_at": hours(12),
        })),
    );
    assert_eq!(status, 201, "{delegated}");
    let delegation = delegated["delegation"]
        .as_str()
        .expect("delegation")
        .to_owned();
    let (status, accepted) = owner(
        "POST",
        "/api/v1/instance-acceptances",
        Some(json!({
            "service": "commonmeasure-hub", "custody": "references_only",
            "retention_until": hours(24 * 90), "access": "owners", "deletion": "on_request",
            "succession": "none", "expires_at": hours(24 * 30),
        })),
    );
    assert_eq!(status, 201, "{accepted}");
    let acceptance = accepted["acceptance"]
        .as_str()
        .expect("acceptance")
        .to_owned();
    println!("POST /api/v1/instance-delegations -> 201; POST /api/v1/instance-acceptances -> 201");

    println!("\n## 4. commonmeasure instance register, then the same key again");
    let register = |session: &str, key: &str| {
        run(&[
            "instance",
            "register",
            "--session",
            session,
            "--job",
            "job-1",
            "--delegation",
            &delegation,
            "--acceptance",
            &acceptance,
            "--host",
            "claude-code",
            "--host-version",
            "2.1",
            "--purpose",
            "research",
            "--expires-in-minutes",
            "30",
            "--idempotency-key",
            key,
        ])
    };
    let (ok, first, stderr) = register("run-1", "live-create-1");
    assert!(ok, "{stderr}");
    show(&first);
    assert_eq!(first["operation"]["http_status"], 201);
    assert_eq!(first["revision"], 1);
    let instance = first["instance"].as_str().expect("instance").to_owned();
    let (ok, again, stderr) = register("run-1", "live-create-1");
    assert!(ok, "{stderr}");
    show(&again);
    assert_eq!(again["operation"]["outcome"], "replayed");
    assert_eq!(again["instance"], first["instance"]);

    println!("\n## 5. A mediated fetch in the registered session");
    let result = mediated_fetch(home.path(), "run-1", &url);
    assert_ne!(result["isError"], json!(true), "{result}");
    let crossing = session_records(home.path(), "run-1")
        .into_iter()
        .find(|record| record["event"] == "crossing_mediated")
        .expect("a mediated crossing");
    println!(
        "crossing_mediated instance={}",
        crossing["payload"]["instance"]
    );
    assert_eq!(crossing["payload"]["instance"]["id"], json!(instance));
    assert_eq!(crossing["payload"]["instance"]["revision"], 1);

    println!("\n## 6. commonmeasure instance renew; commonmeasure instance status");
    let (ok, renewed, stderr) =
        run(&["instance", "renew", &instance, "--expires-in-minutes", "45"]);
    assert!(ok, "{stderr}");
    show(&renewed);
    assert_eq!(renewed["revision"], 2);
    let (ok, standing, stderr) = run(&["instance", "status", &instance]);
    assert!(ok, "{stderr}");
    show(&standing);
    assert_eq!(standing["status"], "active");

    println!("\n## 7. The owner revokes the instance; the next fetch is refused");
    let (status, revoked) = owner(
        "POST",
        &format!("/api/v1/instances/{instance}/revoke"),
        Some(json!({"expected_revision": 2, "reason": "live run"})),
    );
    assert_eq!(status, 200, "{revoked}");
    let result = mediated_fetch(home.path(), "run-1", &url);
    assert_eq!(result["isError"], json!(true), "{result}");
    let refusal = session_records(home.path(), "run-1")
        .into_iter()
        .find(|record| record["event"] == "instance_refused")
        .expect("a recorded refusal");
    println!(
        "instance_refused outcome={} reason={} instance={}",
        refusal["payload"]["outcome"], refusal["payload"]["reason"], refusal["payload"]["instance"]
    );
    assert_eq!(refusal["payload"]["reason"], "instance_revoked");
    let (ok, late, stderr) = run(&["instance", "renew", &instance]);
    assert!(!ok);
    show(&late);
    print!("{stderr}");
    assert_eq!(late["operation"]["reason"], "instance_terminal");

    println!("\n## 8. A second instance is closed with its evidence digest");
    let (ok, second, stderr) = register("run-2", "live-create-2");
    assert!(ok, "{stderr}");
    let second = second["instance"].as_str().expect("instance").to_owned();
    let digest = format!("sha256:{}", "ab".repeat(32));
    let (ok, closed, stderr) = run(&[
        "instance",
        "close",
        &second,
        "--outcome",
        "completed",
        "--evidence",
        &format!("sessions/run-2.ndjson={digest}"),
    ]);
    assert!(ok, "{stderr}");
    show(&closed);
    assert_eq!(closed["status"], "closed");
    let record: Value = serde_json::from_slice(
        &std::fs::read(home.path().join("instances").join(format!("{second}.json"))).unwrap(),
    )
    .unwrap();
    println!(
        "closure outcome={} custody={} evidence_available={}",
        record["closure"]["outcome"],
        record["closure"]["receipt"]["binding"]["custody"],
        record["closure"]["receipt"]["binding"]["evidence_available"]
    );
    assert_eq!(
        record["closure"]["receipt"]["binding"]["custody"],
        "references_only"
    );

    let _ = commonmeasure(home.path(), &["disconnect"]);
    page.stop();
}

/// `instance close` against a hub that cannot be reached, by the binary,
/// offline. The home holds an enrolled key and an active instance written by
/// the test, as a registration leaves them; the enrolment names a loopback
/// port with no listener. The printed summary carries `closure` as a present
/// null member and the operation `closure_unacknowledged`, the command fails,
/// and the retained record keeps no closure and the status `active`.
/// Catches a summary or record that reports a closure the hub never
/// received. No hub, double or network is involved, so it establishes
/// nothing about a hub's answer.
#[test]
fn a_closure_the_hub_never_received_prints_closure_null() {
    use commonmeasure_harness::enrolment::{
        EnrolledIdentity, EnrolledOrganization, EnrolmentRecord,
    };
    use commonmeasure_harness::identity::EdgeKey;
    use commonmeasure_harness::instance::Retained;

    let home = tempfile::tempdir().unwrap();
    // A port that was free a moment ago and has no listener now.
    let hub = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let key = EdgeKey::generate().unwrap();
    key.store(home.path()).unwrap();
    EnrolmentRecord {
        hub: hub.clone(),
        organization: EnrolledOrganization {
            id: "org-1".to_owned(),
            name: "Org".to_owned(),
        },
        name: "laptop".to_owned(),
        key_id: key.thumbprint(),
        identity: EnrolledIdentity {
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
        },
        enrolled_at: "2026-09-21T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    }
    .store(home.path())
    .unwrap();
    let instance = "5b0c8a52-0000-4000-8000-00000000000a";
    let now = chrono::Utc::now();
    let retained: Retained = serde_json::from_value(json!({
        "version": 1,
        "hub": hub,
        "issuer": "commonmeasure-hub",
        "instance": instance,
        "revision": 1,
        "status": "active",
        "work": [{"kind": "session", "namespace": "claude-code", "id": "run-1"}],
        "session_id": "run-1",
        "valid_from": now.to_rfc3339(),
        "expires_at": (now + chrono::Duration::hours(1)).to_rfc3339(),
        "next_check": (now + chrono::Duration::minutes(5)).to_rfc3339(),
        "online_validation_required": false,
        "registration": {},
        "accepted": {},
        "operations": [],
    }))
    .expect("a retained record");
    retained.write(home.path()).unwrap();

    let output = commonmeasure(
        home.path(),
        &["instance", "close", instance, "--outcome", "completed"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    let printed: Value = serde_json::from_slice(&output.stdout).expect("the printed summary");
    assert!(
        printed.get("closure").is_some_and(Value::is_null),
        "the summary carries `closure` null: {printed}"
    );
    assert_eq!(
        printed["operation"]["outcome"], "closure_unacknowledged",
        "{printed}"
    );
    assert_eq!(printed["status"], "active", "{printed}");
    assert!(stderr.contains("closure_unacknowledged"), "{stderr}");

    let held = Retained::read(home.path(), instance)
        .unwrap()
        .expect("the record is kept");
    assert_eq!(held.closure, None);
    assert_eq!(held.status, "active");
    assert_eq!(
        held.operations
            .last()
            .map(|operation| operation.outcome.as_str()),
        Some("closure_unacknowledged")
    );
}

/// A registered instance with a reporting duty, from registration to the
/// hub's read of the duty after onward delivery, against a running Common
/// Measure Hub (registration increment 2 or later): `connect --managed`, a
/// policy synchronisation, a content owner for the page's host, the owner's
/// delegation and a custody acceptance made with reporting (the hub is the
/// durable service), `instance register --duty`, a mediated fetch stamped
/// with the instance reference, `relay` delivering the session to the hub,
/// `instance status` reading the duty, `instance close`, and `instance
/// status` again until the hub reads the duty `delivered`.
///
/// The onward double refuses every delivery with 503 until the test tells it
/// to accept. After closure, and once the double has counted a refused
/// document of this run's session, the hub's read is `closed` with the duty
/// `outstanding` and no acceptance; the test then tells the double to accept,
/// the hub retries one minute after the refused attempt, and the duty reads
/// `delivered` with acceptances whose `events` sum to the duty's `delivered`
/// count and none queued. So `delivered` is asserted to need the receiver's
/// acceptance as well as the instance's end, and `outstanding` while the
/// instance is active is asserted beside it. The printed summary's `closure`
/// is asserted a present null member while the instance is active, and its
/// `receipt` held after closure. The run takes about a minute and a half.
///
/// The mediated fetch reads `https://example.com/`, so the run needs the
/// network: the privacy floor keeps a crossing to a loopback or private
/// address out of every projection, and a session holding only such
/// crossings gives the relay nothing to deliver. Two loopback processes
/// stand beside the binary, and neither is evidence of anything beyond
/// itself:
///
/// - the onward receiver the hub delivers the owner's events to. It is a
///   **test double** in this file: it answers `POST /events` with 503 until
///   told to accept, then with the Content Telemetry acceptance body, and
///   keeps what it accepted. It is not
///   oa-server, not a reference receiver and not a publisher's system, so
///   the run is integration evidence for this edge and the hub and for no
///   receiver;
/// - the hub, which the person running the test starts.
///
/// The hub must be able to deliver to that double, which listens on
/// `http://127.0.0.1:<port>`. A hub serves one only under its development
/// onward setting: `DEV_MODE=true DEV_ONWARD_LOOPBACK='127.0.0.1:*'
/// BIND_ADDRESS=127.0.0.1`.
/// A hub started without it refuses the destination; the test then prints
/// the refusal, runs every later step, shows what the hub reads for the duty
/// without a destination, and fails at the final assertions. It does not
/// edit the hub's database or run the hub's worker by another route to reach
/// `delivered`.
///
/// To run it, start the hub as for the test above and export the same two
/// variables, then:
/// `cargo test -p commonmeasure-cli --test instance_e2e a_real_hub_reads -- --ignored --nocapture`
///
/// The session name carries a suffix unique to the run, so the run can be
/// repeated on one hub database, and the refusal the test waits for is
/// matched to the session by the `session_id` of the refused document. On a
/// database where earlier runs' events are refused ahead of this session's
/// for longer than the wait, the test fails instead of counting their
/// refusal as its own.
#[test]
#[ignore = "requires a running hub server (see the doc comment for how to run it)"]
fn a_real_hub_reads_a_reporting_duty_after_relay_and_closure() {
    let hub_url = std::env::var("COMMONMEASURE_TEST_HUB")
        .expect("COMMONMEASURE_TEST_HUB names the running hub")
        .trim_end_matches('/')
        .to_owned();
    let cookie = format!(
        "__Host-session={}",
        std::env::var("COMMONMEASURE_TEST_HUB_SESSION")
            .expect("COMMONMEASURE_TEST_HUB_SESSION is the owner's session token")
    );
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work_dir = work.path().canonicalize().unwrap();
    // The hub derives a session's wire identity from its name and holds a
    // session to one agent, and each run enrols a new key, so the name
    // carries a suffix unique to the run. The transcript shows it without.
    let suffix = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis())
    );
    let session = format!("duty-1-{suffix}");
    let redact = {
        let home = home.path().display().to_string();
        let work = work_dir.display().to_string();
        let suffix = format!("-{suffix}");
        move |text: &str| {
            text.replace(&home, "<home>")
                .replace(&work, "<work>")
                .replace(&suffix, "")
        }
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
    // retained it: the instance's status and its one duty.
    let hub_read_unprinted = |instance: &str| -> Value {
        let (ok, standing, stderr) = run(&["instance", "status", instance]);
        assert!(ok, "{stderr}");
        standing
    };
    let print_read = |standing: &Value| {
        let duty = &standing["duties"][0];
        println!(
            "instance status -> status={} duty state={} events={} receiver_acceptances={}",
            standing["status"], duty["state"], duty["events"], duty["receiver_acceptances"]
        );
    };
    let hub_read = |instance: &str| -> Value {
        let standing = hub_read_unprinted(instance);
        print_read(&standing);
        standing
    };
    let page_host = "example.com";
    let url = format!("https://{page_host}/");

    // The onward receiver: a test double, as the doc comment says. It
    // refuses until `onward_accepting` is set, counting each refusal and
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

    println!("## 1. The owner publishes a policy; the edge connects as a managed edge");
    let (status, minted) = owner(
        "POST",
        "/api/v1/enrolment/tokens",
        Some(json!({"name": "duty-run"})),
    );
    assert_eq!(status, 201, "{minted}");
    let token = minted["value"].as_str().expect("token").to_owned();
    // The scope clears the run's working directory for telemetry egress;
    // without one the relay withholds the session.
    let (status, published) = owner(
        "POST",
        "/api/v1/policy/revisions",
        Some(json!({"policy": {
            "policy_mode": "observe",
            "scopes": [{"match": work_dir.display().to_string(), "engagement": "duty-run",
                "allow_telemetry_egress": true}],
        }})),
    );
    assert_eq!(status, 201, "{published}");
    println!(
        "POST /api/v1/policy/revisions -> 201 (scope <work>, engagement duty-run, telemetry egress allowed)"
    );
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", &token, "--managed"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("commonmeasure connect <hub> --token et_<redacted> --managed -> ok");
    let synced = commonmeasure(home.path(), &["policy", "sync"]);
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    println!("commonmeasure policy sync -> ok");
    let key_id = commonmeasure_harness::enrolled_key_id(home.path()).expect("an enrolled key id");

    println!("\n## 2. A content owner for the page's host, and its onward destination");
    let (status, created) = owner(
        "POST",
        "/api/v1/owners",
        Some(json!({"name": "Duty run owner"})),
    );
    assert_eq!(status, 201, "{created}");
    let owner_id = created["id"].as_str().expect("owner id").to_owned();
    let (status, claimed) = owner(
        "POST",
        &format!("/api/v1/owners/{owner_id}/hosts"),
        Some(json!({"host": page_host, "basis": "duty run"})),
    );
    assert_eq!(status, 201, "{claimed}");
    println!("POST /api/v1/owners -> 201; POST /api/v1/owners/<owner>/hosts (example.com) -> 201");
    let (destination_status, destination_answer) = owner(
        "PUT",
        &format!("/api/v1/owners/{owner_id}/destination"),
        Some(json!({"url": onward_url, "credential": null})),
    );
    println!(
        "PUT /api/v1/owners/<owner>/destination (the loopback test double, http) -> \
         {destination_status} {destination_answer}"
    );
    let duty = format!("owner:{owner_id}");

    println!("\n## 3. Delegation, and custody acceptance with reporting by the hub");
    let (status, delegated) = owner(
        "POST",
        "/api/v1/instance-delegations",
        Some(json!({
            "edge_key_id": key_id, "operator_id": null, "sponsor": "research-desk",
            "work": [{"kind": "session", "namespace": "claude-code", "id": session}],
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
        "POST /api/v1/instance-delegations -> 201; POST /api/v1/instance-acceptances \
         (service commonmeasure-hub, reporting true) -> 201"
    );

    println!("\n## 4. commonmeasure instance register --duty owner:<owner>");
    let (ok, registered, stderr) = run(&[
        "instance",
        "register",
        "--session",
        &session,
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
        "30",
        "--duty",
        &duty,
    ]);
    assert!(ok, "{stderr}");
    let instance = registered["instance"]
        .as_str()
        .expect("instance")
        .to_owned();
    let record: Value = serde_json::from_slice(
        &std::fs::read(
            home.path()
                .join("instances")
                .join(format!("{instance}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let bound = &record["accepted"]["binding"]["duties"][0]["terms"];
    println!(
        "operation={} http_status={} revision={} bound duty owner={} receiver={} state={}",
        registered["operation"]["operation"],
        registered["operation"]["http_status"],
        registered["revision"],
        bound["owner"]["name"],
        bound["receiver"],
        registered["duties"][0]["state"]
    );
    assert_eq!(registered["operation"]["http_status"], 201);
    assert_eq!(bound["reference"], json!(duty));
    assert_eq!(registered["duties"][0]["state"], "outstanding");

    println!("\n## 5. A mediated fetch in the registered session");
    let result = mediated_fetch_in(home.path(), &session, &url, Some(&work_dir));
    assert_ne!(result["isError"], json!(true), "{result}");
    let crossing = session_records(home.path(), &session)
        .into_iter()
        .find(|record| record["event"] == "crossing_mediated")
        .expect("a mediated crossing");
    println!(
        "crossing_mediated instance={}",
        crossing["payload"]["instance"]
    );
    assert_eq!(crossing["payload"]["instance"]["id"], json!(instance));

    println!("\n## 6. commonmeasure relay delivers the session to the hub");
    let relayed = commonmeasure(home.path(), &["relay", "--session", &session]);
    let relay_out = redact(&String::from_utf8_lossy(&relayed.stdout));
    print!("{relay_out}");
    assert!(
        relayed.status.success(),
        "{}",
        String::from_utf8_lossy(&relayed.stderr)
    );
    let spooled = commonmeasure_relay::spool::Spool::read_only(home.path())
        .entries()
        .expect("the spool reads");
    let sent: Vec<&Value> = spooled
        .iter()
        .flat_map(|(_, entry)| entry.document["events"].as_array().into_iter().flatten())
        .filter(|event| event["content_url"] == json!(url))
        .collect();
    // The spool holds what was queued for the hub, which delivery may alter
    // before posting. That the reference arrived is the hub's duty read in
    // step 7, which counts only events that carried it.
    assert!(!sent.is_empty());
    println!(
        "read from the edge's spool, queued for and accepted by the hub: {} content events, each \
         with instance={}",
        sent.len(),
        sent[0]["instance"]
    );
    assert!(
        sent.iter()
            .all(|event| event["instance"]["id"] == json!(instance))
    );
    // The session's identity on the wire, as the relay queued it. The hub
    // keeps it and sends it onward as the document's `session_id`, and the
    // run's suffix makes it this run's alone, so a refused document carrying
    // it holds this run's events and no earlier run's.
    let mut wire_sessions: Vec<&Value> = spooled
        .iter()
        .filter(|(_, entry)| {
            entry.document["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["content_url"] == json!(url))
            })
        })
        .map(|(_, entry)| &entry.document["session_id"])
        .collect();
    wire_sessions.dedup();
    assert_eq!(wire_sessions.len(), 1, "one session was relayed");
    let wire_session = wire_sessions[0].clone();
    assert!(wire_session.is_string(), "{wire_session}");

    println!("\n## 7. The hub's read of the duty while the instance is active");
    let active = hub_read(&instance);

    println!("\n## 8. commonmeasure instance close");
    let digest = format!("sha256:{}", "ab".repeat(32));
    let (ok, closed, stderr) = run(&[
        "instance",
        "close",
        &instance,
        "--outcome",
        "completed",
        "--evidence",
        &format!("sessions/{session}.ndjson={digest}"),
    ]);
    assert!(ok, "{stderr}");
    println!(
        "operation={} http_status={} status={} duty state={}",
        closed["operation"]["operation"],
        closed["operation"]["http_status"],
        closed["status"],
        closed["duties"][0]["state"]
    );
    assert_eq!(closed["status"], "closed");

    println!("\n## 9. The hub's read after closure: the double refusing, then accepting");
    // The hub's onward worker ticks every 15 seconds, so it sends the events
    // within a tick of step 6 and the double refuses them. The read is taken
    // once a refused document of this run's session is counted: ended, sent,
    // not accepted. A refusal of an earlier run's events, which a reused
    // database routes to this owner by their host, does not count. Without a
    // destination the hub attempts nothing and the final assertions fail.
    // Nothing between the relay and the flip below may wait beyond this
    // loop: the hub retries one minute after a refusal and two after a
    // second, and the reads after the flip allow for the first only.
    let refused_of_this_session = || {
        onward_refused_sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|refused| **refused == wire_session)
            .count()
    };
    let waiting = Instant::now();
    while destination_status == 200
        && refused_of_this_session() == 0
        && waiting.elapsed() < Duration::from_secs(45)
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    let refused_first = refused_of_this_session();
    println!(
        "the onward test double has refused {} delivery attempt(s) with 503, {refused_first} of \
         them of this run's session, and accepted none",
        onward_refusals.load(Ordering::SeqCst)
    );
    let unaccepted = hub_read(&instance);
    let accepted_while_refusing = onward_bodies.lock().unwrap().len();
    onward_accepting.store(true, Ordering::SeqCst);
    println!(
        "the onward test double now accepts (the hub retries a refused delivery one minute after \
         the attempt)"
    );
    // Read every five seconds; only the last read is printed.
    let mut last = hub_read_unprinted(&instance);
    let mut reads_before = 0;
    for _ in 0..20 {
        if last["duties"][0]["state"] == "delivered" || destination_status != 200 {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));
        last = hub_read_unprinted(&instance);
        reads_before += 1;
    }
    println!("({reads_before} reads at five-second intervals did not read `delivered`; then:)");
    print_read(&last);
    let onward_bodies = onward_bodies.lock().unwrap().clone();
    let earlier: Vec<&Value> = onward_bodies
        .iter()
        .filter(|body| body["session_id"] != wire_session)
        .collect();
    println!(
        "the onward test double refused {} delivery attempt(s), then accepted {} document(s); \
         of those, documents of earlier runs' sessions, routed to this owner by their host: {} \
         holding {} events",
        onward_refusals.load(Ordering::SeqCst),
        onward_bodies.len(),
        earlier.len(),
        earlier
            .iter()
            .map(|body| body["events"].as_array().map_or(0, Vec::len))
            .sum::<usize>()
    );

    // The owner is removed so the run can be repeated on the same database;
    // every read the assertions below use has been taken.
    let _ = owner("DELETE", &format!("/api/v1/owners/{owner_id}"), None);
    let _ = commonmeasure(home.path(), &["disconnect"]);
    onward.stop();

    assert_eq!(
        destination_status, 200,
        "the hub did not accept the loopback onward destination: {destination_answer}"
    );
    assert_eq!(active["status"], "active", "{active}");
    assert_eq!(
        active["duties"][0]["state"], "outstanding",
        "an active instance's duty is outstanding whatever has been delivered: {active}"
    );
    assert!(
        refused_first > 0,
        "the hub never attempted the onward delivery of this run's session"
    );
    assert_eq!(accepted_while_refusing, 0);
    assert!(
        active.get("closure").is_some_and(Value::is_null),
        "an active instance's summary carries `closure` null: {active}"
    );
    assert_eq!(unaccepted["status"], "closed", "{unaccepted}");
    assert!(
        !unaccepted["closure"]["receipt"].is_null(),
        "the custody receipt is printed once the closure is acknowledged: {unaccepted}"
    );
    assert_eq!(
        unaccepted["duties"][0]["state"], "outstanding",
        "closed and refused by the receiver is not delivered: {unaccepted}"
    );
    assert_eq!(acceptances(&unaccepted), 0, "{unaccepted}");
    assert_eq!(last["status"], "closed", "{last}");
    assert_eq!(last["duties"][0]["state"], "delivered", "{last}");
    assert!(accepted_whole(&last), "{last}");
    let onward_events: Vec<&Value> = onward_bodies
        .iter()
        .flat_map(|body| body["events"].as_array().into_iter().flatten())
        .collect();
    assert!(!onward_events.is_empty());
    assert!(
        onward_events
            .iter()
            .all(|event| event.get("instance").is_none()),
        "the instance reference is not sent onward"
    );
}
