//! The declared directory uses real directory selection; it creates no grant.
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
        "a missing registry must not restore legacy clearance"
    );
}

#[test]
fn the_operator_directory_is_recorded_without_claiming_client_context_or_granting_egress() {
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
        "requesting reporting cannot replace a signed owner grant"
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
fn the_declared_scope_reports_only_with_a_signed_grant_and_stops_after_revocation() {
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

    let observe = |session: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["hook", "post-tool-use"])
            .env("COMMONMEASURE_HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("hook");
        writeln!(child.stdin.as_mut().expect("stdin"), "{}", json!({
            "session_id": session, "cwd": status["cwd"],
            "hook_event_name": "PostToolUse", "tool_name": "WebFetch",
            "tool_input": {"url":"https://www.example.com/fixture"},
            "tool_response": {"result":"Synthetic fixture text for reporting permission checks."}
        })).expect("fixture input");
        drop(child.stdin.take());
        assert!(child.wait().expect("hook finishes").success());
    };
    let grant = |revision: u64, grants: Value| {
        let edge = commonmeasure_harness::EnrolmentRecord::load(home.path())
            .expect("enrolment")
            .expect("enrolled");
        let payload = json!({"schema":"commonmeasure-directory-grants/v1",
            "organisation": ORGANISATION, "edge_key_id": edge.key_id,
            "revision": revision, "grants": grants});
        let now = chrono::Utc::now();
        let mut snapshot = json!({"payload":payload,"digest":canonical_digest(&payload),
            "key_id":SIGNER_KEY_ID,"issued_at":now,"expires_at":now+chrono::Duration::hours(1)});
        snapshot["signature"] = json!(encode_hex(
            hub.policy_signer
                .sign(canonical_json(&snapshot).as_bytes())
                .as_ref()
        ));
        commonmeasure_harness::directory::accept(home.path(), &snapshot)
            .expect("valid signed snapshot");
    };
    let relay = || {
        let (ok, stdout, stderr) = run(home.path(), &["relay"]);
        assert!(ok, "{stdout}\n{stderr}");
    };
    observe("declared-scope-fixture-1");
    relay();
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "no owner grant"
    );
    grant(
        1,
        json!([{"project_id":project.id,"binding":project.binding}]),
    );
    relay();
    let delivered = hub.batches.lock().expect("batches").len();
    assert!(delivered > 0, "a valid grant must permit eligible evidence");
    grant(2, json!([]));
    observe("declared-scope-fixture-2");
    relay();
    assert_eq!(
        hub.batches.lock().expect("batches").len(),
        delivered,
        "revoked grant withholds new evidence"
    );
}
