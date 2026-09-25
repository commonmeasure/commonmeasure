//! The declared directory uses real directory selection; it creates no reporting approval.
use super::*;
use commonmeasure_harness::directory::Registry;

fn configure(home: &Path, directory: &Path) {
    std::fs::write(
        home.join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0", "origin": ORIGIN, "hosts": ["m365-copilot"],
            "interval_seconds": 300, "session_directory": directory,
        })
        .to_string(),
    )
    .expect("configuration");
}

#[test]
fn a_declared_directory_needs_a_current_explicit_local_selection() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    for directory in [Path::new("relative"), Path::new("/"), root.path()] {
        configure(home.path(), directory);
        let output = service_command(home.path()).output().expect("process");
        assert!(
            !output.status.success(),
            "unselected directory started the service"
        );
    }
    Registry::enrol(home.path(), root.path(), "Hosted test", false).expect("local selection");
    let child = root.path().join("child");
    std::fs::create_dir(&child).expect("child");
    configure(home.path(), &child);
    let output = service_command(home.path()).output().expect("process");
    assert!(
        !output.status.success(),
        "a descendant is not the declared root"
    );
    configure(home.path(), root.path());
    std::fs::remove_file(home.path().join("directories.json")).expect("remove selection");
    let output = service_command(home.path()).output().expect("process");
    assert!(
        !output.status.success(),
        "a missing registry must not fall back to scope clearances"
    );
}

#[test]
fn the_operator_directory_is_recorded_without_claiming_client_context_or_permitting_egress() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    Registry::enrol(home.path(), root.path(), "Hosted test", true).expect("requested reporting");
    configure(home.path(), root.path());
    let service = Service::start(home.path());
    let authorization = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &authorization);
    let response = service.call(
        "m365-copilot",
        &authorization,
        &session,
        1,
        "context_status",
        json!({}),
    );
    let status = payload(&response);
    let expected = root.path().canonicalize().expect("canonical root");
    assert_eq!(status["cwd"], expected.to_str().expect("UTF-8"));
    assert_eq!(status["policy"]["private_floor"], "held");
    let policy = commonmeasure_harness::policy::SessionPolicy::load(home.path(), expected.to_str())
        .expect("policy");
    assert!(
        !policy.allows_telemetry_egress(),
        "requesting reporting cannot replace a signed owner approval"
    );
    let records = commonmeasure_harness::SessionLog::read(
        &home
            .path()
            .join("sessions")
            .join(format!("{session}.ndjson")),
    )
    .expect("record");
    let scope = records
        .iter()
        .find(|r| r["event"] == "hosted_scope")
        .expect("scope basis");
    assert_eq!(scope["payload"]["basis"], "service_configuration");
    assert_eq!(
        scope["payload"]["directory"],
        expected.to_str().expect("UTF-8")
    );
    let listed = service.post(
        "/mcp/m365-copilot",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
        ],
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    assert!(
        !text(&listed).contains("context_enrol"),
        "a remote client cannot enrol or approve its scope"
    );
}

/// This is a synthetic observed-source fixture through the real hook and
/// relay, not evidence of a live Copilot acquisition or answer use.
#[test]
fn the_declared_scope_reports_only_with_a_signed_approval_and_stops_after_revocation() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Hosted test", true).expect("selection");
    configure(home.path(), root.path());
    let service = Service::start(home.path());
    let auth = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &auth);
    let status = payload(&service.call(
        "m365-copilot",
        &auth,
        &session,
        1,
        "context_status",
        json!({}),
    ));
    drop(service);

    let cwd = status["cwd"].as_str().expect("cwd").to_owned();
    let observe = |session: &str| observe(home.path(), &cwd, session, FIXTURE_URL);
    let approve = |revision: u64, approvals: Value| approve(&hub, home.path(), revision, approvals);
    let relay = || relay_ok(home.path());
    observe("declared-scope-fixture-1");
    relay();
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "no owner approval"
    );
    approve(
        1,
        json!([{"project_id":project.id,"binding":project.binding}]),
    );
    relay();
    let delivered = hub.batches.lock().expect("batches").len();
    assert!(
        delivered > 0,
        "a valid approval must permit eligible evidence"
    );
    approve(2, json!([]));
    observe("declared-scope-fixture-2");
    relay();
    assert_eq!(
        hub.batches.lock().expect("batches").len(),
        delivered,
        "revoked approval withholds new evidence"
    );
}

const FIXTURE_URL: &str = "https://www.example.com/fixture";

/// One synthetic crossing of `url` through the real hook, in `cwd`.
fn observe(home: &Path, cwd: &str, session: &str, url: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("hook");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({
            "session_id": session, "cwd": cwd,
            "hook_event_name": "PostToolUse", "tool_name": "WebFetch",
            "tool_input": {"url": url},
            "tool_response": {"result":"Synthetic fixture text for reporting permission checks."}
        })
    )
    .expect("fixture input");
    drop(child.stdin.take());
    assert!(child.wait().expect("hook finishes").success());
}

/// A reporting-approval snapshot signed by the hub's policy key, valid for
/// `lifetime` from now.
fn signed_approvals(
    hub: &Hub,
    home: &Path,
    revision: u64,
    approvals: Value,
    lifetime: chrono::Duration,
) -> Value {
    let edge = commonmeasure_harness::EnrolmentRecord::load(home)
        .expect("enrolment")
        .expect("enrolled");
    let payload = json!({"schema":"commonmeasure-reporting-approvals/v1",
        "organisation": ORGANISATION, "edge_key_id": edge.key_id,
        "revision": revision, "approvals": approvals});
    let now = chrono::Utc::now();
    let mut snapshot = json!({"payload":payload,"digest":canonical_digest(&payload),
        "key_id":SIGNER_KEY_ID,"issued_at":now,"expires_at":now+lifetime});
    snapshot["signature"] = json!(encode_hex(
        hub.policy_signer
            .sign(canonical_json(&snapshot).as_bytes())
            .as_ref()
    ));
    snapshot
}

/// A reporting-approval snapshot accepted as `enrol --sync` accepts one.
/// Unless the test sets the hub's approvals, the loopback hub answers the
/// approvals route `404`, so the relay's own refresh fails and the accepted
/// snapshot stands.
fn approve(hub: &Hub, home: &Path, revision: u64, approvals: Value) {
    let snapshot = signed_approvals(hub, home, revision, approvals, chrono::Duration::hours(1));
    commonmeasure_harness::directory::accept(home, &snapshot).expect("valid signed snapshot");
}

fn relay_ok(home: &Path) {
    let (ok, stdout, stderr) = run(home, &["relay"]);
    assert!(ok, "{stdout}\n{stderr}");
}

/// Fault injection while no relay runs: move every persisted retry deadline
/// into the past. The CLI has no clock override.
fn make_retry_due(home: &Path) {
    let path = home.join("relay/spool/outbound.delivery.json");
    let mut states: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("delivery state")).expect("JSON");
    for state in states.as_object_mut().expect("states").values_mut() {
        state["next_attempt_at"] = json!("2000-01-01T00:00:00Z");
    }
    std::fs::write(path, serde_json::to_vec(&states).expect("JSON")).expect("write");
}

/// Every event id the hub received, with how many times it arrived, and the
/// content URLs among them.
fn received(hub: &Hub) -> (std::collections::BTreeMap<String, usize>, Vec<String>) {
    let mut ids = std::collections::BTreeMap::new();
    let mut urls = Vec::new();
    for batch in hub.batches.lock().expect("batches").iter() {
        for event in batch["events"].as_array().expect("events") {
            *ids.entry(event["id"].as_str().expect("id").to_owned())
                .or_default() += 1;
            if let Some(url) = event["content_url"].as_str() {
                urls.push(url.to_owned());
            }
        }
    }
    (ids, urls)
}

/// EGR-48, through the real hook and relay against a loopback hub. A batch
/// spooled while approved and due after the approval is revoked is held, not
/// dropped: nothing is sent, and the spool records it held. Evidence recorded
/// while approved and first relayed after revocation is not projected. After
/// re-approval the next relay sends both, each event once, and a further
/// relay sends nothing.
#[test]
fn a_revoked_approval_holds_evidence_and_re_approval_sends_it_once() {
    const QUEUED: &str = "https://www.example.com/queued";
    const RECORDED: &str = "https://www.example.com/recorded";
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project = Registry::enrol(home.path(), root.path(), "Held test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let cwd = cwd.to_str().expect("UTF-8");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    approve(&hub, home.path(), 1, approved.clone());
    observe(home.path(), cwd, "held-queued", QUEUED);
    hub.delivery_outage.store(true, Ordering::SeqCst);
    assert!(!run(home.path(), &["relay"]).0, "the outage fails delivery");
    hub.delivery_outage.store(false, Ordering::SeqCst);
    observe(home.path(), cwd, "held-recorded", RECORDED);

    approve(&hub, home.path(), 2, json!([]));
    make_retry_due(home.path());
    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "a revoked approval sends nothing"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 1, "the queued batch is held: {egress}");
    assert!(egress["hold_reason"].is_string(), "{egress}");
    assert_eq!(egress["delivered"], 0);

    approve(&hub, home.path(), 3, approved);
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == QUEUED), "{urls:?}");
    assert!(urls.iter().any(|url| url == RECORDED), "{urls:?}");
    assert!(
        ids.values().all(|&times| times == 1),
        "each event is sent once: {ids:?}"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 0, "{egress}");
    assert_eq!(egress["pending"], 0, "{egress}");

    let batches = hub.batches.lock().expect("batches").len();
    relay_ok(home.path());
    assert_eq!(
        hub.batches.lock().expect("batches").len(),
        batches,
        "a further relay sends nothing"
    );
}

/// The sequence of the hub's real-edge directory test, where EGR-48 was
/// seen. A failed delivery sets the batch's retry deadline; a relay before
/// that deadline neither rechecks nor sends the batch, under revocation or
/// after re-approval. The batch is queued throughout and is sent once when
/// the deadline passes.
#[test]
fn a_batch_in_retry_backoff_at_re_approval_is_sent_when_due() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Backoff test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    approve(&hub, home.path(), 1, approved.clone());
    observe(
        home.path(),
        cwd.to_str().expect("UTF-8"),
        "backoff",
        FIXTURE_URL,
    );
    hub.delivery_outage.store(true, Ordering::SeqCst);
    assert!(!run(home.path(), &["relay"]).0, "the outage fails delivery");
    hub.delivery_outage.store(false, Ordering::SeqCst);

    approve(&hub, home.path(), 2, json!([]));
    relay_ok(home.path());
    approve(&hub, home.path(), 3, approved);
    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "a batch before its retry deadline is not sent"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 0, "not rechecked, so not held: {egress}");
    assert!(egress["next_attempt_at"].is_string(), "{egress}");
    assert!(
        egress["pending"].as_u64().is_some_and(|n| n > 0),
        "{egress}"
    );

    make_retry_due(home.path());
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == FIXTURE_URL), "{urls:?}");
    assert!(ids.values().all(|&times| times == 1), "{ids:?}");
    let batches = hub.batches.lock().expect("batches").len();
    relay_ok(home.path());
    assert_eq!(hub.batches.lock().expect("batches").len(), batches);
}

/// EGR-49, through the real hook and relay against a loopback hub. An
/// approval that expires withholds the evidence; once the hub renews it, the
/// relay's own refresh restores clearance and that same relay sends it, with
/// no `enrol --sync` between.
#[test]
fn an_approval_renewed_after_expiry_clears_evidence_on_the_next_relay() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Renewal test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    let lifetime = Duration::from_secs(2);
    let short = signed_approvals(
        &hub,
        home.path(),
        1,
        approved.clone(),
        chrono::Duration::from_std(lifetime).expect("duration"),
    );
    let expires = Instant::now() + lifetime;
    commonmeasure_harness::directory::accept(home.path(), &short).expect("valid signed snapshot");
    observe(
        home.path(),
        cwd.to_str().expect("UTF-8"),
        "renewal",
        FIXTURE_URL,
    );
    std::thread::sleep(
        expires.saturating_duration_since(Instant::now()) + Duration::from_millis(200),
    );

    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "an expired approval sends nothing"
    );

    *hub.approvals.lock().expect("approvals") = Some(signed_approvals(
        &hub,
        home.path(),
        2,
        approved,
        chrono::Duration::hours(1),
    ));
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == FIXTURE_URL), "{urls:?}");
    assert!(ids.values().all(|&times| times == 1), "{ids:?}");
}
