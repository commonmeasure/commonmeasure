//! The fleet-status document from the real binary, across edge homes: the
//! identity moves when the policy a session is in changes and stays when
//! only another scope does, and a receiver's answers come from the
//! documents alone (`docs/contracts/fleet-status.md`).

use std::path::Path;
use std::process::Command;

use commonmeasure_harness::fleet::{Convergence, classify};
use serde_json::Value;

const POLICY: &str = r#"{
  "policy_mode": "observe",
  "scopes": [
    {"match": "clientwork", "policy_mode": "strict",
     "constraints": [{"kind": "allowed_source_host", "host": "www.gov.uk"}]},
    {"match": "personalwork", "policy_mode": "observe"}
  ]
}"#;

fn edge(policy: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("policy.json"), policy).expect("policy");
    home
}

fn status(home: &Path, cwd: &Path, json: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .arg("status")
        .env("COMMONMEASURE_HOME", home)
        .current_dir(cwd);
    if json {
        command.arg("--json");
    }
    command.output().expect("status should run")
}

fn document(home: &Path, cwd: &Path) -> Value {
    let output = status(home, cwd, true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the document is JSON")
}

fn identity(document: &Value) -> String {
    document["applied"]["policy_identity"]["digest"]
        .as_str()
        .expect("an identity digest")
        .to_owned()
}

/// Two edges with one policy report one identity; an edit to the scope the
/// session is in moves it; an edit to another scope does not, although the
/// declaration's digest does.
#[test]
fn drift_shows_between_two_edge_homes_and_not_when_only_an_unrelated_scope_changes() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let client = workspace.path().join("clientwork");
    std::fs::create_dir_all(&client).expect("client dir");

    let first = edge(POLICY);
    let second = edge(POLICY);
    let first_document = document(first.path(), &client);
    let second_document = document(second.path(), &client);
    assert_eq!(first_document["contract"], "contextops-fleet-status/v1");
    assert_eq!(
        identity(&first_document),
        identity(&second_document),
        "one policy on two edges is one identity"
    );
    assert_eq!(
        first_document["applied"]["policy_digest"],
        second_document["applied"]["policy_digest"]
    );

    let related = edge(&POLICY.replace(
        r#"{"match": "clientwork", "policy_mode": "strict","#,
        r#"{"match": "clientwork", "policy_mode": "prefer","#,
    ));
    let related_document = document(related.path(), &client);
    assert_ne!(
        identity(&first_document),
        identity(&related_document),
        "the scope this session is in changed, so its identity drifted"
    );

    let unrelated = edge(&POLICY.replace(
        r#"{"match": "personalwork", "policy_mode": "observe"}"#,
        r#"{"match": "personalwork", "policy_mode": "strict", "constraints": [{"kind": "denied_source_host", "host": "x.example"}]}"#,
    ));
    let unrelated_document = document(unrelated.path(), &client);
    assert_eq!(
        identity(&first_document),
        identity(&unrelated_document),
        "a scope this session is not in changed, and that is not drift for this session"
    );
    assert_ne!(
        first_document["applied"]["policy_digest"], unrelated_document["applied"]["policy_digest"],
        "the declaration as a whole did change, and its digest says so"
    );

    // The same edge, outside every scope, resolves to the top-level policy:
    // another identity again.
    let outside = document(first.path(), workspace.path());
    assert_ne!(identity(&first_document), identity(&outside));
}

/// What the document says about an edge nobody has enrolled or managed,
/// and what it never says.
#[test]
fn an_unenrolled_local_edge_reports_unknown_identity_no_desired_state_and_no_policy_text() {
    let home = edge(&POLICY.replace(
        r#"{"match": "clientwork", "policy_mode": "strict","#,
        r#"{"match": "clientwork", "policy_mode": "strict", "engagement": "acme-secret",
            "allow_telemetry_egress": true,"#,
    ));
    let workspace = tempfile::tempdir().expect("tempdir");
    let document = document(home.path(), workspace.path());

    assert!(document["edge"]["key_id"].is_null());
    assert!(
        document["edge"]["unknown"]
            .as_str()
            .is_some_and(|reason| reason.contains("not enrolled")),
        "{document}"
    );
    assert_eq!(document["deployment_mode"], "local");
    assert!(document["desired"].is_null());
    assert!(document["applied"]["revision"].is_null());
    assert_eq!(document["applied"]["policy_declared"], true);
    assert_eq!(document["applied"]["principal"]["basis"], "os_user");
    assert!(document["last_enforcement"]["at"].is_null());
    assert_eq!(document["allowances"], serde_json::json!([]));
    let text = document.to_string();
    for excluded in [
        "acme-secret",
        "clientwork",
        "www.gov.uk",
        "allowed_source_host",
    ] {
        assert!(
            !text.contains(excluded),
            "{excluded} must not be in the document: {text}"
        );
    }
    assert!(
        !text.contains(&workspace.path().display().to_string()),
        "the working directory stays on the machine: {text}"
    );

    // A receiver given this document and any desired revision answers
    // unknown: nothing here says which revision the edge applied.
    let (verdict, reason) = classify(&document, 1, "sha256:anything");
    assert_eq!(verdict, Convergence::Unknown);
    assert!(reason.contains("no applied revision"), "{reason}");

    let readable = status(home.path(), workspace.path(), false);
    assert!(readable.status.success());
    let text = String::from_utf8_lossy(&readable.stdout);
    assert!(text.contains("policy identity   sha256:"), "{text}");
    assert!(text.contains("edge              unknown"), "{text}");
    assert!(text.contains("deployment mode   local"), "{text}");
    assert!(text.contains("drift evidence, not proof"), "{text}");
}

/// A policy that does not load is reported as unavailable with no digest,
/// not as the observing default the operator never wrote.
#[test]
fn an_unreadable_policy_reports_unavailable_and_no_identity() {
    let home = edge("{ not json");
    let workspace = tempfile::tempdir().expect("tempdir");
    let document = document(home.path(), workspace.path());
    assert!(document["applied"]["policy_digest"].is_null());
    assert!(document["applied"]["policy_identity"].is_null());
    assert!(
        document["applied"]["unavailable"]
            .as_str()
            .is_some_and(|reason| reason.contains("not a valid policy")),
        "{document}"
    );
    assert_eq!(classify(&document, 1, "sha256:x").0, Convergence::Unknown);
    let readable = status(home.path(), workspace.path(), false);
    assert!(readable.status.success());
    let text = String::from_utf8_lossy(&readable.stdout);
    assert!(text.contains("policy            unavailable:"), "{text}");
}
