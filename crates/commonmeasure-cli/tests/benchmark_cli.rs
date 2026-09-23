//! The real CLI resolves scoped source policy and keeps offline runs offline.
use commonmeasure_http::{Response, Server};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn command(home: &Path, output: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .env_clear()
        .env("COMMONMEASURE_HOME", home)
        .current_dir(home)
        .arg("benchmark")
        .arg("--dataset")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/benchmarks/simpleqa/smoke.json"),
        )
        .arg("--output")
        .arg(output)
        .args(["--providers", "exa"]);
    command
}

#[test]
fn offline_run_resolves_scoped_policy_and_makes_no_gateway_calls() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    std::fs::write(home.join("policy.json"), json!({"policy_mode":"observe", "scopes":[{
        "match":home.to_str().unwrap(), "policy_mode":"strict", "constraints":[{"kind":"denied_provider","provider":"exa"}]}]}).to_string()).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let received = hits.clone();
    let gateway = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_| {
            received.fetch_add(1, Ordering::SeqCst);
            Response::new(500, vec![])
        })
        .unwrap();
    let output = home.join("batch");
    let run = command(home, &output)
        .args(["--limit", "1"])
        .env("COMMONMEASURE_INFERENCE_ENDPOINT", gateway.url())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["providers"][1]["refused"], 1);
    assert_eq!(summary["providers"][1]["unmeasured"], 1);
    assert!(summary["providers"][1]["accuracy_measured"].is_null());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["suite_template"]["job"]["policy_mode"], "strict");
    assert_eq!(
        manifest["suite_template"]["job"]["constraints"][0]["provider"],
        "exa"
    );
    assert!(!home.join("sessions").exists());
    let again = command(home, &output).output().unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("never resumed"));
}

#[test]
fn missing_gateway_or_invalid_dataset_fails_before_creating_attempt() {
    let home = tempfile::tempdir().unwrap();
    let output = home.path().join("batch");
    let result = command(home.path(), &output)
        .arg("--live")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("COMMONMEASURE_INFERENCE_ENDPOINT"));
    assert!(!output.exists());
    let result = command(home.path(), &output)
        .args(["--limit", "0"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
fn openrouter_without_a_key_stops_before_any_attempt() {
    let home = tempfile::tempdir().unwrap();
    let output = home.path().join("openrouter");
    let result = command(home.path(), &output)
        .env(
            "COMMONMEASURE_INFERENCE_ENDPOINT",
            "https://openrouter.ai/api/v1/chat/completions",
        )
        .arg("--live")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("OPENROUTER_API_KEY"));
    assert!(!output.exists());
}

#[test]
fn an_unrelated_gateway_never_receives_the_openrouter_key() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("policy.json"),
        json!({"policy_mode":"strict",
        "constraints":[{"kind":"denied_provider","provider":"exa"}]})
        .to_string(),
    )
    .unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let received = hits.clone();
    let gateway = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            assert!(request.headers.get("Authorization").is_none());
            assert!(!String::from_utf8_lossy(&request.body).contains("fixture-openrouter-key"));
            received.fetch_add(1, Ordering::SeqCst);
            Response::json(200, r#"{"choices":[{"message":{"content":"A"}}]}"#)
        })
        .unwrap();
    let output = home.path().join("local-gateway");
    let result = command(home.path(), &output)
        .env("COMMONMEASURE_INFERENCE_ENDPOINT", gateway.url())
        .env("OPENROUTER_API_KEY", "fixture-openrouter-key")
        .args(["--limit", "1", "--live"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    let case_id = manifest["case_ids"][0].as_str().unwrap();
    let record = std::fs::read_to_string(output.join(case_id).join("summary.json")).unwrap();
    assert!(!record.contains("fixture-openrouter-key"));
}

#[test]
fn unresolved_or_unrepresentable_source_policy_stops_before_calls() {
    let hits = Arc::new(AtomicUsize::new(0));
    let received = hits.clone();
    let gateway = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_| {
            received.fetch_add(1, Ordering::SeqCst);
            Response::new(500, vec![])
        })
        .unwrap();
    for policy in [
        json!({"policy_mode":"observe", "principals":[{
            "principal":"unbound", "subject":"not-this-session"}]}),
        json!({"policy_mode":"strict", "refuse_on_pii":true}),
        json!({"policy_mode":"strict", "record_internal_prefixes":["https://private.example/"]}),
    ] {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), policy.to_string()).unwrap();
        let output = home.path().join("batch");
        let result = command(home.path(), &output)
            .arg("--live")
            .env("COMMONMEASURE_INFERENCE_ENDPOINT", gateway.url())
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "policy was silently weakened: {policy}"
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be represented"));
        assert!(!output.exists());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }
}
