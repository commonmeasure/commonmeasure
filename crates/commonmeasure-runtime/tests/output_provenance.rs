//! The output provenance label, end to end over the real internal corpus
//! adapter and the real C2PA SDK: a suite that declares `output_provenance`
//! gets every answered plan labelled with a manifest built from that plan's
//! window, signed with the demonstration identity and embedded under Annex
//! A.8; the label reads back, re-derives from the record, and stops matching
//! the moment a hash, a grade or a byte of the text is altered.
//!
//! The gateway is a loopback origin returning a fixed answer, so the answer
//! bytes are known and the label's binding to them can be checked.

use std::path::{Path, PathBuf};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::processor::provenance::{
    self, CERTIFICATE_VARIABLE, Grade, Input, KEY_VARIABLE, OutputProvenance, SigningIdentity,
    SigningMaterial, Source,
};
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::InternalCorpusAdapter;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const ANSWER: &str = "The onboarding gateway validates a data source before it connects. \
                      CITATION: https://corpus.example/connecting.md";
const PROMPT: &str = "How does the onboarding gateway connect a data source?";

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, content).expect("write");
}

fn corpus() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"name": "provenance test corpus",
            "licence": {"state": "declared", "reference": "test/licence-v1"},
            "dates": {"connecting.md": "2026-05-01", "validating.md": "2026-05-02"}}"#,
    );
    write(
        root,
        "connecting.md",
        "# Connecting a data source\n\nConnect a data source through the onboarding gateway. \
         The gateway records the connection and its owner.\n",
    );
    write(
        root,
        "validating.md",
        "# Validating a data source\n\nThe onboarding gateway validates a data source before \
         it connects, checking its schema and its declared owner.\n",
    );
    directory
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
                    "usage": {"prompt_tokens": 60, "completion_tokens": 20}
                })
                .to_string(),
            )
        })
        .expect("spawn")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the repository root")
        .to_path_buf()
}

/// The committed demonstration identity (`demo/provenance/README.md`).
fn demonstration_identity() -> SigningIdentity {
    let root = repo_root().join("demo/provenance");
    SigningIdentity::Configured(SigningMaterial {
        certificate_chain: std::fs::read(root.join("signer.pem")).expect("signer.pem"),
        private_key: std::fs::read(root.join("signer-key.pem")).expect("signer-key.pem"),
        certificate_ref: "demo/provenance/signer.pem".to_owned(),
        key_ref: "demo/provenance/signer-key.pem".to_owned(),
    })
}

fn suite(declare: bool) -> Suite {
    let mut suite = json!({
        "suite_version": "provenance-test/v1",
        "label": "Output provenance on the attest path",
        "job": {
            "id": Uuid::new_v4(),
            "kind": "research.answer",
            "prompt": PROMPT,
            "policy_mode": "strict",
            "objective": {"kind": "minimise_latency"},
            "constraints": [],
            "evidence_requirements": []
        },
        "model_plan": {"name": "pinned", "version": "1", "model": "local-test-model"},
        "result_limit": 4,
        "providers": ["internal"],
        "require_cited_answer": true
    });
    if declare {
        suite["output_provenance"] = json!({
            "training_mining": {
                "ai_inference": "allowed",
                "ai_training": "not_allowed",
                "ai_generative_training": "not_allowed",
                "data_mining": "constrained",
                "constraint_info": "https://operator.example/ai-use"
            }
        });
    }
    serde_json::from_value(suite).expect("a well-formed suite")
}

struct Run {
    _gateway: ServerHandle,
    _corpus: tempfile::TempDir,
    _directory: tempfile::TempDir,
    output: PathBuf,
    summary: Value,
}

impl Run {
    fn execute(suite: &Suite, signing: SigningIdentity) -> Self {
        let corpus = corpus();
        let gateway = gateway();
        let directory = tempfile::tempdir().expect("tempdir");
        let root = corpus.path().to_path_buf();
        let output = directory.path().join("run");
        let options = RunOptions {
            allow_external_acquisition: false,
            output: output.clone(),
            suppliers: Box::new(move |_| {
                Ok(Box::new(InternalCorpusAdapter::new(&root))
                    as Box<dyn commonmeasure_supply::SupplyAdapter>)
            }),
            backend: Some(Box::new(
                TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
            )),
            replay: None,
            allowance: None,
            provenance_signing: signing,
        };
        let summary = execute(suite, &options)
            .expect("the run should complete")
            .summary;
        Self {
            _gateway: gateway,
            _corpus: corpus,
            _directory: directory,
            output,
            summary,
        }
    }

    fn plan(&self, id: &str) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == id)
            .unwrap_or_else(|| panic!("plan {id}"))
    }

    fn provenance_invocations<'a>(&'a self, plan: &'a Value) -> Vec<&'a Value> {
        plan["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .filter(|invocation| invocation["processor"]["name"] == provenance::NAME)
            .collect()
    }

    /// The window as the transform stage published it: each part by the
    /// hash of the text that entered, in window order. This is the record
    /// the label is re-derived from.
    fn window(&self, plan: &Value) -> Vec<Source> {
        plan["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .find(|invocation| invocation["stage"] == "transform")
            .expect("a transform invocation")["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|output| Source {
                reference: output["reference"].as_str().expect("reference").to_owned(),
                content_hash: output["content_hash"]
                    .as_str()
                    .expect("content hash")
                    .to_owned(),
                grade: Grade::Mediated,
            })
            .collect()
    }

    fn labelled_text(&self, plan: &Value) -> String {
        let invocation = self.provenance_invocations(plan)[0];
        let reference = invocation["outputs"][0]["reference"]
            .as_str()
            .expect("labelled output reference");
        std::fs::read_to_string(self.output.join(reference)).expect("labelled output")
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[test]
fn a_declared_suite_labels_every_answered_plan_and_the_label_reads_back() {
    let run = Run::execute(&suite(true), demonstration_identity());
    let plan = run.plan("internal-only");
    assert_eq!(plan["status"], "completed");
    assert_eq!(
        plan["answer"], ANSWER,
        "the plan's answer stays the model's own bytes"
    );

    let invocations = run.provenance_invocations(plan);
    assert_eq!(invocations.len(), 1);
    let invocation = invocations[0];
    assert_eq!(invocation["stage"], "attest");
    // The contract's stage order: the label is the last invocation, after
    // the verify stage.
    let stages: Vec<&str> = plan["processors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|invocation| invocation["stage"].as_str().unwrap())
        .collect();
    assert_eq!(stages.last(), Some(&"attest"));
    assert!(
        stages.iter().position(|stage| *stage == "verify")
            < stages.iter().position(|stage| *stage == "attest"),
        "verify precedes attest: {stages:?}"
    );
    assert_eq!(stages.iter().filter(|stage| **stage == "attest").count(), 1);
    assert_eq!(invocation["decision"], "admit");
    assert!(invocation["gaps"].as_array().unwrap().is_empty());
    assert_eq!(invocation["detail"]["validation"]["state"], "valid");
    assert_eq!(invocation["detail"]["validation"]["trusted"], false);
    assert!(
        invocation["detail"]["validation"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("valid but untrusted"),
        "the record says the signature verifies and the certificate is on no trust list"
    );
    assert_eq!(
        invocation["outputs"][0]["reference"],
        "provenance/internal-only.txt"
    );
    assert_eq!(
        invocation["outputs"][1]["reference"],
        "provenance/internal-only.c2pa"
    );
    let manifest_bytes = std::fs::read(run.output.join("provenance/internal-only.c2pa")).unwrap();
    assert_eq!(
        invocation["outputs"][1]["content_hash"],
        sha256(&manifest_bytes)
    );

    // The label reads back from the published text alone.
    let text = run.labelled_text(plan);
    assert_eq!(
        invocation["outputs"][0]["content_hash"],
        sha256(text.as_bytes())
    );
    let read = provenance::read_back(&text).expect("the label reads back");
    assert_eq!(read.validation["state"], "valid");
    assert_eq!(read.validation["trusted"], false);
    assert_eq!(read.clean_text_hash, sha256(ANSWER.as_bytes()));
    assert_eq!(read.manifest_bytes, manifest_bytes.len());

    // Every window part is an ingredient by content hash and grade, in order.
    let window = run.window(plan);
    assert_eq!(window.len(), 2, "both corpus documents entered the window");
    assert_eq!(read.ingredients, window);
    assert!(window.iter().all(|source| source.grade == Grade::Mediated));

    // The source record names the run, the plan, the run manifest and the
    // answer by hash, and lists the same sources.
    assert_eq!(read.source_record["run_id"], run.summary["run"]["id"]);
    assert_eq!(read.source_record["plan_id"], "internal-only");
    assert_eq!(
        read.source_record["run_manifest_hash"],
        run.summary["run"]["manifest_hash"]
    );
    assert_eq!(read.source_record["answer_hash"], sha256(ANSWER.as_bytes()));
    let listed: Vec<Source> =
        serde_json::from_value(read.source_record["sources"].clone()).expect("sources");
    assert_eq!(listed, window);

    // The created action, the operator's declaration and its identity.
    assert_eq!(read.actions["actions"][0]["action"], "c2pa.created");
    assert_eq!(
        read.actions["actions"][0]["digitalSourceType"],
        provenance::DIGITAL_SOURCE_TYPE
    );
    let entries = &read.training_mining["entries"];
    assert_eq!(entries["cawg.ai_inference"]["use"], "allowed");
    assert_eq!(entries["cawg.ai_training"]["use"], "notAllowed");
    assert_eq!(entries["cawg.data_mining"]["use"], "constrained");
    assert_eq!(
        entries["cawg.data_mining"]["constraint_info"],
        "https://operator.example/ai-use"
    );
    assert_eq!(
        read.identity["signer_payload"]["sig_type"],
        "cawg.x509.cose"
    );
    assert_eq!(
        read.identity["signer_payload"]["role"][0],
        provenance::OPERATOR_ROLE
    );
    assert_eq!(read.signature["issuer"], "Common Measure Ltd");

    // The baseline answered too, with nothing grounding it: labelled with no
    // ingredients rather than left out.
    let baseline = run.plan("no-context");
    let baseline_text = run.labelled_text(baseline);
    let baseline_read = provenance::read_back(&baseline_text).expect("baseline label");
    assert!(baseline_read.ingredients.is_empty());
    assert_eq!(baseline_read.source_record["plan_id"], "no-context");

    // The suite's declaration is sealed in the run manifest.
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(run.output.join("manifest.json")).expect("manifest.json"),
    )
    .unwrap();
    assert_eq!(
        manifest["manifest"]["output_provenance"]["training_mining"]["ai_training"],
        "not_allowed"
    );
    let installed = manifest["manifest"]["processors"].as_array().unwrap();
    assert!(
        installed
            .iter()
            .any(|processor| processor["name"] == provenance::NAME)
    );
}

/// The acceptance property: the manifest re-derives from the source record,
/// and a record with one hash or one grade altered no longer derives it.
#[test]
fn the_label_re_derives_from_the_record_and_fails_when_a_hash_or_grade_is_altered() {
    let run = Run::execute(&suite(true), demonstration_identity());
    let plan = run.plan("internal-only");
    let read = provenance::read_back(&run.labelled_text(plan)).expect("label");
    let window = run.window(plan);
    let sealed: Value =
        serde_json::from_slice(&std::fs::read(run.output.join("manifest.json")).unwrap()).unwrap();
    let policy: OutputProvenance =
        serde_json::from_value(sealed["manifest"]["output_provenance"].clone())
            .expect("the sealed declaration");
    let run_id = run.summary["run"]["id"].as_str().unwrap().to_owned();
    let manifest_hash = run.summary["run"]["manifest_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let signing = SigningIdentity::Unconfigured;
    let derive = |sources: &[Source]| {
        let input = Input {
            run_id: &run_id,
            plan_id: "internal-only",
            run_manifest_hash: &manifest_hash,
            answer: ANSWER,
            answer_reference: "plans/internal-only/answer",
            sources,
            policy: &policy,
            signing: &signing,
            labelled_reference: "provenance/internal-only.txt",
            manifest_reference: "provenance/internal-only.c2pa",
        };
        (
            provenance::source_record(&input),
            provenance::definition(&input)["ingredients"].clone(),
        )
    };

    let (record, ingredients) = derive(&window);
    assert_eq!(record, read.source_record, "the source record re-derives");
    let read_ingredients: Vec<Source> = ingredients
        .as_array()
        .unwrap()
        .iter()
        .map(|ingredient| Source {
            reference: ingredient["title"].as_str().unwrap().to_owned(),
            content_hash: ingredient["instance_id"].as_str().unwrap().to_owned(),
            grade: Grade::Mediated,
        })
        .collect();
    assert_eq!(
        read_ingredients, read.ingredients,
        "the ingredients re-derive"
    );

    // The same re-derivation from the published summary, as `commonmeasure
    // provenance --run` performs it, agrees with the label until the summary
    // is edited.
    let implied = provenance::record_from_summary(&run.summary, "internal-only").unwrap();
    assert!(provenance::record_differences(&read.source_record, &implied).is_empty());
    let mut edited = run.summary.clone();
    let plan_index = edited["plans"]
        .as_array()
        .unwrap()
        .iter()
        .position(|plan| plan["id"] == "internal-only")
        .unwrap();
    let transform_index = edited["plans"][plan_index]["processors"]
        .as_array()
        .unwrap()
        .iter()
        .position(|invocation| invocation["stage"] == "transform")
        .unwrap();
    edited["plans"][plan_index]["processors"][transform_index]["outputs"][0]["content_hash"] =
        json!(sha256(b"not the text that entered"));
    let differences = provenance::record_differences(
        &read.source_record,
        &provenance::record_from_summary(&edited, "internal-only").unwrap(),
    );
    assert_eq!(differences.len(), 1, "{differences:?}");
    assert!(differences[0].starts_with("sources[0].content_hash"));

    let mut altered_hash = window.clone();
    altered_hash[0].content_hash = sha256(b"not the text that entered");
    let (record, _) = derive(&altered_hash);
    assert_ne!(
        record, read.source_record,
        "an altered hash no longer derives the label"
    );

    let mut altered_grade = window.clone();
    altered_grade[1].grade = Grade::Observed;
    let (record, ingredients) = derive(&altered_grade);
    assert_ne!(
        record, read.source_record,
        "an altered grade no longer derives the label"
    );
    assert_ne!(
        ingredients[1]["description"],
        read.manifest["manifests"][&read.manifest_label]["ingredients"][1]["description"],
        "the ingredient carries the grade the record was altered away from"
    );
}

#[test]
fn a_tampered_output_no_longer_validates() {
    let run = Run::execute(&suite(true), demonstration_identity());
    let text = run.labelled_text(run.plan("internal-only"));
    let tampered = text.replacen("validates", "ignores", 1);
    assert_ne!(tampered, text);
    let read = provenance::read_back(&tampered).expect("the wrapper still parses");
    assert_eq!(read.validation["state"], "invalid");
    assert!(
        read.validation["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|status| status["code"] == "assertion.dataHash.mismatch"),
        "the data hash names the mismatch: {}",
        read.validation
    );
}

#[test]
fn the_manifest_carries_hashes_and_ids_and_never_the_prompt_or_the_answer() {
    let run = Run::execute(&suite(true), demonstration_identity());
    let manifest = std::fs::read(run.output.join("provenance/internal-only.c2pa")).unwrap();
    let contains = |needle: &str| {
        manifest
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    assert!(!contains("onboarding gateway validates"), "no answer text");
    assert!(!contains("How does the onboarding"), "no prompt text");
    assert!(!contains("checking its schema"), "no context text");
    let window = run.window(run.plan("internal-only"));
    for source in &window {
        assert!(contains(&source.content_hash), "each source by hash");
        assert!(contains(&source.reference), "each source by reference");
    }
    assert!(contains(
        run.summary["run"]["manifest_hash"].as_str().unwrap()
    ));
    assert!(contains(&sha256(ANSWER.as_bytes())));
}

#[test]
fn without_a_signing_identity_the_gap_names_the_variables_and_nothing_is_published() {
    let run = Run::execute(&suite(true), SigningIdentity::Unconfigured);
    let plan = run.plan("internal-only");
    assert_eq!(plan["status"], "completed", "the answer stands");
    let invocation = run.provenance_invocations(plan)[0];
    assert_eq!(invocation["decision"], "abstain");
    let gap = invocation["gaps"][0]["detail"].as_str().unwrap();
    assert!(gap.contains(CERTIFICATE_VARIABLE) && gap.contains(KEY_VARIABLE));
    assert!(
        invocation["detail"]["manifest_definition"]["ingredients"]
            .as_array()
            .unwrap()
            .len()
            == 2,
        "the claim that would have been signed is still in the record"
    );
    assert!(!run.output.join("provenance").exists());

    let unusable = Run::execute(
        &suite(true),
        SigningIdentity::Unusable {
            reference: "/nowhere/signer.pem".to_owned(),
            reason: format!("{CERTIFICATE_VARIABLE} cannot be read: no such file"),
        },
    );
    let invocation = unusable.provenance_invocations(unusable.plan("internal-only"))[0];
    assert_eq!(invocation["decision"], "abstain");
    assert!(
        invocation["gaps"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("no such file")
    );
}

#[test]
fn an_undeclared_suite_records_no_invocation_and_seals_no_declaration() {
    let run = Run::execute(&suite(false), demonstration_identity());
    let plan = run.plan("internal-only");
    assert_eq!(plan["status"], "completed");
    assert!(run.provenance_invocations(plan).is_empty());
    assert!(!run.output.join("provenance").exists());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(run.output.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["manifest"].get("output_provenance").is_none());
}

#[test]
fn a_constrained_entry_without_terms_is_refused_before_the_run_starts() {
    let mut suite = suite(true);
    suite
        .output_provenance
        .as_mut()
        .unwrap()
        .training_mining
        .constraint_info = None;
    let directory = tempfile::tempdir().expect("tempdir");
    let options = RunOptions {
        allow_external_acquisition: false,
        output: directory.path().join("run"),
        suppliers: Box::new(|_| unreachable!("the suite is refused before supply")),
        backend: None,
        replay: None,
        allowance: None,
        provenance_signing: SigningIdentity::Unconfigured,
    };
    let error = execute(&suite, &options).err().expect("refused");
    assert!(error.to_string().contains("constraint_info"));
}
