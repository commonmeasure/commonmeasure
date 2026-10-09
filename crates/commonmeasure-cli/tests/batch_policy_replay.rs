//! A synthetic replay fixture through the real binary, replay transport and
//! parser. It proves suite-only policy wiring, not a supplier integration.

use std::process::Command;

use commonmeasure_types::canonical::canonical_digest;
use serde_json::{Value, json};

#[test]
fn replay_does_not_load_or_refresh_the_home_policy() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir(&home).unwrap();
    // Ordinary batch resolution would refuse both declarations. Replay must
    // neither read them nor substitute policy for the suite it reproduces.
    std::fs::write(home.join("policy.json"), "malformed fixture policy").unwrap();
    std::fs::write(home.join("deployment.json"), "malformed fixture deployment").unwrap();
    let suite = json!({
        "suite_version":"replay-policy-fixture/v1", "label":"Replay policy fixture",
        "job":{"id":"3d8beea4-eb91-4f2d-9426-2fa2f06fef8b", "kind":"research.answer",
            "prompt":"fixture", "policy_mode":"observe", "objective":{"kind":"maximise_quality"},
            "constraints":[], "evidence_requirements":[]},
        "model_plan":{"name":"fixture", "version":"1", "model":"unconfigured"},
        "result_limit":1, "providers":["exa"],
    });
    std::fs::write(directory.path().join("suite.json"), suite.to_string()).unwrap();
    let body = json!({"results":[{"url":"https://fixture.example/article", "text":"Synthetic fixture evidence."}]});
    std::fs::write(
        directory.path().join("capture.json"),
        json!({
            "request":{"url":"https://api.exa.ai/search"}, "response":{"status":200,"body":body}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(directory.path().join("replay-manifest.json"), json!({
        "manifest_version":"contextops-replay/v1", "derived_from":"synthetic test data",
        "recordings":[{"provider":"exa", "capability":"search", "source":"capture.json",
            "captured_at":"2026-10-08", "recorded_endpoint":"https://api.exa.ai/search",
            "response_sha256":canonical_digest(&body), "redactions":"none", "permitted_use":"test"}]
    }).to_string()).unwrap();
    let output = directory.path().join("run");
    let result = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .env_clear()
        .env("COMMONMEASURE_HOME", &home)
        .current_dir(directory.path())
        .args(["run", "suite.json", "--replay", ".", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(summary["run"]["mode"], "replay");
    assert!(summary.get("source_policy").is_none());
    assert!(manifest["manifest"].get("source_policy").is_none());
    let plan = summary["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["provider"] == "exa")
        .unwrap();
    assert_eq!(plan["source_count"], 1);
    assert_eq!(plan["verification_state"], "replay-tested");
    assert!(!home.join("managed").exists());
    assert!(!home.join("allowances").exists());
}
