//! A second session reads the output's own provenance back from the pasted
//! text. One process runs a suite that declares `output_provenance` under the
//! demonstration identity; a separate process, given only the labelled text
//! and no run directory, reads the manifest back through `commonmeasure
//! provenance` and reports the ingredients, the source record and the
//! validation state.

use std::path::{Path, PathBuf};
use std::process::Command;

use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};

const ANSWER: &str = "Connect the source through the onboarding gateway, which validates it \
                      first. CITATION: https://corpus.example/connecting.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, content).expect("write");
}

fn gateway() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            Response::json(
                200,
                &json!({
                    "id": "tz-provenance",
                    "model": "local-test-model",
                    "choices": [{"message": {"content": ANSWER}}],
                    "usage": {"prompt_tokens": 40, "completion_tokens": 20}
                })
                .to_string(),
            )
        })
        .expect("spawn")
}

fn commonmeasure() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command.current_dir(repo_root());
    for variable in [
        "EXA_API_KEY",
        "TAVILY_API_KEY",
        "FIRECRAWL_API_KEY",
        "TOLLBIT_API_KEY",
        "COMMONMEASURE_INFERENCE_ENDPOINT",
        "COMMONMEASURE_PROVENANCE_CERTIFICATE",
        "COMMONMEASURE_PROVENANCE_KEY",
    ] {
        command.env_remove(variable);
    }
    command
}

/// The first session: a run whose answers are labelled.
fn labelled_run(directory: &Path, gateway: &ServerHandle) -> (PathBuf, Value) {
    let corpus = directory.join("corpus");
    write(
        &corpus,
        "corpus.json",
        r#"{"name": "read-back corpus",
            "licence": {"state": "declared", "reference": "test/licence-v1"},
            "dates": {"connecting.md": "2026-05-01"}}"#,
    );
    write(
        &corpus,
        "connecting.md",
        "# Connecting a data source\n\nConnect a data source through the onboarding gateway, \
         which validates it first.\n",
    );
    let suite = directory.join("suite.json");
    write(
        directory,
        "suite.json",
        &json!({
            "suite_version": "provenance-readback/v1",
            "label": "A labelled run read back in a second session",
            "job": {
                "id": "1d3e5f7a-9b2c-4d6e-8f1a-3c5e7b9d1f2a",
                "kind": "research.answer",
                "prompt": "How do I connect a data source through the onboarding gateway?",
                "policy_mode": "strict",
                "objective": {"kind": "minimise_latency"},
                "constraints": [],
                "evidence_requirements": []
            },
            "model_plan": {"name": "pinned", "version": "1", "model": "local-test-model"},
            "result_limit": 4,
            "providers": ["internal"],
            "output_provenance": {
                "training_mining": {
                    "ai_inference": "allowed",
                    "ai_training": "not_allowed",
                    "ai_generative_training": "not_allowed",
                    "data_mining": "not_allowed"
                }
            }
        })
        .to_string(),
    );
    let output = directory.join("run");
    let status = commonmeasure()
        .args(["run"])
        .arg(&suite)
        .arg("--output")
        .arg(&output)
        .env("COMMONMEASURE_INTERNAL_CORPUS", &corpus)
        .env("COMMONMEASURE_INFERENCE_ENDPOINT", gateway.url())
        .env(
            "COMMONMEASURE_PROVENANCE_CERTIFICATE",
            repo_root().join("demo/provenance/signer.pem"),
        )
        .env(
            "COMMONMEASURE_PROVENANCE_KEY",
            repo_root().join("demo/provenance/signer-key.pem"),
        )
        .status()
        .expect("run");
    assert!(status.success());
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    (output, summary)
}

#[test]
fn a_second_session_reads_the_label_back_from_the_pasted_text_alone() {
    let gateway = gateway();
    let directory = tempfile::tempdir().expect("tempdir");
    let (output, summary) = labelled_run(directory.path(), &gateway);
    let plan = summary["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["id"] == "internal-only")
        .expect("the internal plan");
    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["answer"], ANSWER);
    let invocation = plan["processors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|invocation| invocation["processor"]["name"] == "output-provenance")
        .expect("a provenance invocation");
    assert_eq!(invocation["decision"], "admit");

    // The dossier names the label, its validation state and the read-back
    // command, so a reader finds it without the summary.
    let dossier = commonmeasure()
        .arg("inspect")
        .arg(&output)
        .output()
        .expect("inspect");
    assert!(dossier.status.success());
    let dossier = String::from_utf8_lossy(&dossier.stdout).replace('\n', " ");
    let flat: String = dossier.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("provenance/internal-only.txt")
            && flat.contains("valid but untrusted")
            && flat.contains("commonmeasure provenance"),
        "dossier: {flat}"
    );

    // Against the run it came from, the label's source record re-derives
    // from the summary; after the summary is edited it no longer does, and
    // the command says which field.
    let labelled_path = output.join("provenance/internal-only.txt");
    let checked = commonmeasure()
        .arg("provenance")
        .arg(&labelled_path)
        .arg("--run")
        .arg(&output)
        .output()
        .expect("provenance --run");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(checked["record"]["matches"], true);
    let summary_path = output.join("summary.json");
    let mut edited: Value = serde_json::from_slice(&std::fs::read(&summary_path).unwrap()).unwrap();
    let plans = edited["plans"].as_array_mut().unwrap();
    let plan = plans
        .iter_mut()
        .find(|plan| plan["id"] == "internal-only")
        .unwrap();
    let transform = plan["processors"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|invocation| invocation["stage"] == "transform")
        .unwrap();
    transform["outputs"][0]["content_hash"] = json!("sha256:0000");
    std::fs::write(&summary_path, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    let mismatch = commonmeasure()
        .arg("provenance")
        .arg(&labelled_path)
        .arg("--run")
        .arg(&output)
        .output()
        .expect("provenance --run");
    assert!(!mismatch.status.success());
    let mismatch_report: Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert_eq!(mismatch_report["record"]["matches"], false);
    assert!(
        mismatch_report["record"]["differences"][0]
            .as_str()
            .unwrap()
            .starts_with("sources[0].content_hash")
    );
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("differs"));

    // The paste: the labelled text copied into a file that belongs to no
    // run, so the second session has nothing but the text to go on.
    let labelled = std::fs::read_to_string(&labelled_path).unwrap();
    let pasted = directory.path().join("pasted.txt");
    std::fs::write(&pasted, &labelled).unwrap();
    std::fs::remove_dir_all(&output).unwrap();

    let report = commonmeasure()
        .arg("provenance")
        .arg(&pasted)
        .output()
        .expect("provenance");
    assert!(
        report.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&report.stderr)
    );
    let read: Value = serde_json::from_slice(&report.stdout).expect("a JSON report");
    assert_eq!(read["validation"]["state"], "valid");
    assert_eq!(read["validation"]["trusted"], false);
    assert!(
        read["validation"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("valid but untrusted")
    );
    assert_eq!(read["source_record"]["run_id"], summary["run"]["id"]);
    assert_eq!(read["source_record"]["plan_id"], "internal-only");
    assert_eq!(
        read["source_record"]["run_manifest_hash"],
        summary["run"]["manifest_hash"]
    );
    let ingredients = read["ingredients"].as_array().unwrap();
    assert_eq!(ingredients.len(), 1);
    assert_eq!(ingredients[0]["grade"], "mediated");
    assert_eq!(
        ingredients[0]["content_hash"], invocation["inputs"][1]["content_hash"],
        "the ingredient's hash is the hash of the text that entered the window"
    );
    assert_eq!(
        read["training_mining"]["entries"]["cawg.ai_training"]["use"],
        "notAllowed"
    );
    assert_eq!(
        read["identity"]["signer_payload"]["role"][0],
        "cawg.producer"
    );
    assert!(
        !report
            .stdout
            .windows(ANSWER.len())
            .any(|w| w == ANSWER.as_bytes()),
        "the report, like the manifest, carries the answer by hash and not by text"
    );

    // Text with no label is an error, not an empty report.
    let plain = directory.path().join("plain.txt");
    std::fs::write(&plain, ANSWER).unwrap();
    let report = commonmeasure()
        .arg("provenance")
        .arg(&plain)
        .output()
        .expect("provenance");
    assert!(!report.status.success());
    assert!(String::from_utf8_lossy(&report.stderr).contains("no C2PA Annex A.8 label"));
}
