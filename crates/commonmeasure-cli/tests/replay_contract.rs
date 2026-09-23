//! The `--replay` run mode, asserted against the real binary and the
//! committed manifest in `recon/` of the recorded fixtures.
//!
//! Like `run_contract.rs`, everything here runs `commonmeasure` as a process
//! with provider credentials removed from the child environment: a replay run
//! must never need one, and this suite would catch it reaching for one.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

mod credential_sweep;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The maintainer's recorded fixtures (`COMMONMEASURE_PRIVATE_EVIDENCE`,
/// resolved by `build.rs`).
fn evidence() -> PathBuf {
    PathBuf::from(env!("COMMONMEASURE_EVIDENCE_DIR"))
}

fn commonmeasure(args: &[&str], output: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .current_dir(repo_root())
        .args(args)
        .arg("--output")
        .arg(output)
        .env_remove("EXA_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .env_remove("FIRECRAWL_API_KEY")
        .env_remove("TOLLBIT_API_KEY")
        .env_remove("COMMONMEASURE_INFERENCE_ENDPOINT");
    command
}

fn replay_run() -> (tempfile::TempDir, Value) {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("replay");
    let recon = evidence().join("recon");
    let result = commonmeasure(
        &[
            "run",
            "demo/jobs/eu-ai-act-replay.json",
            "--replay",
            recon.to_str().expect("the fixture directory is UTF-8"),
        ],
        &output,
    )
    .output()
    .expect("the binary should start");
    assert!(
        result.status.success(),
        "replay run failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary = std::fs::read(output.join("summary.json")).expect("summary.json");
    (
        directory,
        serde_json::from_slice(&summary).expect("summary.json should be valid JSON"),
    )
}

/// A replay run with no credentials and no gateway still acquires — from
/// loopback origins serving the committed recordings — and claims exactly
/// `replay-tested`, never `live-verified` and never `planned`.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_replay_run_acquires_from_recordings_and_claims_replay_tested() {
    let (directory, summary) = replay_run();
    let output = directory.path().join("replay");

    assert_eq!(summary["run"]["mode"], "replay");
    assert!(
        summary["run"]["replay_manifest"]
            .as_str()
            .expect("a replay run names its manifest")
            .ends_with("replay-manifest.json")
    );

    for plan in summary["plans"].as_array().expect("plans") {
        let id = plan["id"].as_str().unwrap_or("<unnamed>");
        if plan["provider"] == "none" {
            continue;
        }
        assert_eq!(
            plan["verification_state"], "replay-tested",
            "plan `{id}` must claim replay-tested"
        );
        // No gateway is configured, so no plan may answer; the acquisition
        // still happened and its record binds to the recording.
        assert!(
            plan["answer"].is_null(),
            "plan `{id}` answered with no gateway"
        );
        assert_eq!(
            plan["acquisition"]["replay"]["matches_recorded_input"],
            true
        );
        let sealed = output.join(
            plan["acquisition"]["response_ref"]
                .as_str()
                .expect("a sealed response"),
        );
        assert!(sealed.exists(), "plan `{id}`'s response is not sealed");
    }

    let replay: Value =
        serde_json::from_slice(&std::fs::read(output.join("replay.json")).expect("replay.json"))
            .expect("replay.json is JSON");
    assert_eq!(
        replay["bindings"].as_array().expect("bindings").len(),
        4,
        "every provider plan is bound to its recording"
    );

    // A replay artefact carries no credential-shaped string: the recordings
    // were redacted at capture and the placeholder is deliberately not
    // secret-shaped.
    credential_sweep::no_artefact_carries_a_credential(
        &output,
        &[
            "summary.json",
            "manifest.json",
            "evidence.ndjson",
            "replay.json",
        ],
    );
}

/// A directory with no manifest cannot replay, and the refusal happens before
/// any run directory is published.
#[test]
fn a_replay_directory_without_a_manifest_fails_the_run_explicitly() {
    let empty = tempfile::tempdir().expect("tempdir");
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("replay");
    let mut command = commonmeasure(&["run", "demo/jobs/eu-ai-act-replay.json"], &output);
    command.arg("--replay").arg(empty.path());
    let result = command.output().expect("the binary should start");
    assert!(
        !result.status.success(),
        "a replay with no manifest must fail"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("replay manifest"),
        "the failure must name the manifest: {stderr}"
    );
    assert!(!output.exists(), "no run may be published on refusal");
}

/// `--live` and `--replay` are contradictory instructions, and the binary
/// refuses the contradiction rather than picking one.
#[test]
fn live_and_replay_refuse_to_combine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("replay");
    let result = commonmeasure(
        &[
            "run",
            "demo/jobs/eu-ai-act-replay.json",
            "--live",
            "--replay",
            "recon",
        ],
        &output,
    )
    .output()
    .expect("the binary should start");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("--live") && stderr.contains("--replay"),
        "the refusal must name both flags: {stderr}"
    );
}
