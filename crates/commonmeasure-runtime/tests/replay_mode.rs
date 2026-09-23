//! Recorded replay as a run mode, exercised against the committed manifest
//! and captures in `recon/`.
//!
//! The replay claims under test: verified recorded bytes travel the real
//! adapter/transport/policy path, the run says `replay` and `replay-tested`
//! rather than anything stronger, every sealed response is bound to the exact
//! recorded input hash, a missing or altered recording fails the run rather
//! than weakening it, and two replay runs of one suite agree on everything
//! but the declared volatile fields — which is why a `minimise_latency`
//! objective abstains under replay: the latency it would rank on times the
//! loopback origin serving the recording, not the recorded provider.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::{
    ReplayContext, ReplayError, ReplaySupply, RunError, RunOptions, Suite, execute,
};
use commonmeasure_types::{Constraint, ContextJob, Money, Objective, PolicyMode};
use serde_json::{Value, json};
use uuid::Uuid;

/// The maintainer's recorded fixtures (`COMMONMEASURE_PRIVATE_EVIDENCE`,
/// resolved by `build.rs`).
fn evidence() -> PathBuf {
    PathBuf::from(env!("COMMONMEASURE_EVIDENCE_DIR"))
}

fn recon() -> PathBuf {
    evidence().join("recon")
}

/// A deterministic loopback gateway, counting what it was asked.
fn gateway_origin() -> (ServerHandle, Arc<Mutex<u32>>) {
    let calls = Arc::new(Mutex::new(0u32));
    let counter = Arc::clone(&calls);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            *counter.lock().expect("counter") += 1;
            Response::json(
                200,
                r#"{"id":"tz-replay","model":"local-test-model",
                    "choices":[{"message":{"content":"The supplied excerpts describe the Code of Practice."}}],
                    "usage":{"prompt_tokens":40,"completion_tokens":9}}"#,
            )
        })
        .expect("spawn");
    (handle, calls)
}

/// A deterministic loopback gateway whose fixed answer carries citation
/// lines, so a suite requiring citations gives the grounding evaluator
/// something to judge. One line names a URL no recording served, which is
/// `uncovered`; one does not parse, which is `unavailable`. Neither depends
/// on any recorded text, so the verdict counts are non-zero whatever the
/// captures hold.
fn cited_gateway_origin() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            Response::json(
                200,
                r#"{"id":"tz-replay-cited","model":"local-test-model",
                    "choices":[{"message":{"content":"The supplied excerpts describe the Code of Practice.\nCITATION: https://example.invalid/absent :: \"the Code of Practice\"\nCITATION: neither a url nor a quote"}}],
                    "usage":{"prompt_tokens":40,"completion_tokens":9}}"#,
            )
        })
        .expect("spawn")
}

/// One suite over all four recorded providers. Strict policy, no host
/// allow-list: the recorded sources are the hosts the reconnaissance found,
/// and this suite compares supply, not source policy.
fn suite() -> Suite {
    Suite {
        suite_version: "replay-test/v1".into(),
        label: "Replay-mode path".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "What do the supplied sources establish?".into(),
            policy_mode: PolicyMode::Strict,
            objective: Objective::MinimiseLatency,
            constraints: vec![Constraint::MaximumAcquisitionCost {
                amount: Money::from_decimal_str("USD", "0.05").expect("a valid cap"),
            }],
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 2,
        providers: vec![
            "exa".into(),
            "firecrawl".into(),
            "tavily".into(),
            "tollbit".into(),
        ],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

struct ReplayRun {
    _gateway: ServerHandle,
    gateway_calls: Arc<Mutex<u32>>,
    directory: tempfile::TempDir,
    summary: Value,
}

impl ReplayRun {
    fn execute(suite: &Suite) -> Self {
        let supply = ReplaySupply::start(&recon(), &suite.providers)
            .expect("the committed manifest covers every suite provider");
        let (gateway, gateway_calls) = gateway_origin();
        let directory = tempfile::tempdir().expect("tempdir");
        let mut options = supply.run_options(directory.path().join("replay"));
        // The environment must not decide this test's gateway.
        options.backend = Some(Box::new(
            TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
        ));
        let report = execute(suite, &options).expect("the replay run should complete");
        Self {
            _gateway: gateway,
            gateway_calls,
            directory,
            summary: report.summary,
        }
    }

    fn published(&self) -> PathBuf {
        self.directory.path().join("replay")
    }

    fn plan(&self, id: &str) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == id)
            .unwrap_or_else(|| panic!("no plan {id}"))
    }
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_replay_run_is_marked_replay_tested_and_binds_every_response_to_its_recording() {
    let suite = suite();
    let run = ReplayRun::execute(&suite);

    assert_eq!(run.summary["run"]["mode"], "replay");
    assert_eq!(
        run.summary["run"]["replay_manifest"],
        json!(recon().join("replay-manifest.json").display().to_string())
    );

    // The manifest's own hashes, read independently of the runtime.
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(recon().join("replay-manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let declared = |provider: &str, field: &str| -> String {
        manifest["recordings"]
            .as_array()
            .expect("recordings")
            .iter()
            .find(|recording| recording["provider"] == provider)
            .expect("a recording per provider")[field]
            .as_str()
            .expect("a declared value")
            .to_owned()
    };

    for (plan_id, provider) in [
        ("exa-only", "exa"),
        ("firecrawl-only", "firecrawl"),
        ("tavily-only", "tavily"),
        ("tollbit-only", "tollbit"),
    ] {
        let plan = run.plan(plan_id);
        assert_eq!(
            plan["verification_state"], "replay-tested",
            "{plan_id} must claim replay-tested, never live-verified"
        );
        let acquisition = &plan["acquisition"];
        // The sealed endpoint is the recorded one: the loopback origin's
        // ephemeral port is transport plumbing that would make two replay
        // runs disagree. That nothing external was reached is held
        // by the absent credentials and the origins this supply serves.
        assert_eq!(
            acquisition["endpoint"],
            json!(declared(provider, "recorded_endpoint")),
            "{plan_id} must seal the recorded endpoint, not the loopback port"
        );
        // The replay binding: sealed bytes equal the recorded input,
        // by the hash the manifest declared before the run existed.
        assert_eq!(
            acquisition["response_hash"],
            json!(declared(provider, "response_sha256"))
        );
        assert_eq!(
            acquisition["replay"]["recorded_response_sha256"],
            json!(declared(provider, "response_sha256"))
        );
        assert_eq!(acquisition["replay"]["matches_recorded_input"], true);
        assert_eq!(acquisition["replay"]["captured_at"], "2026-08-01");
    }

    // Three recorded responses carry text and complete; TollBit's carries
    // none, so its plan is honestly unavailable — still replay-tested,
    // because the exchange happened and its bytes are sealed.
    for plan_id in ["exa-only", "firecrawl-only", "tavily-only"] {
        assert_eq!(run.plan(plan_id)["status"], "completed", "{plan_id}");
    }
    assert_eq!(run.plan("tollbit-only")["status"], "unavailable");
    assert_eq!(*run.gateway_calls.lock().expect("counter"), 4);

    // The published comparison: one binding per replayed plan, each matched.
    let replay: Value =
        serde_json::from_slice(&std::fs::read(run.published().join("replay.json")).unwrap())
            .expect("replay.json");
    assert_eq!(replay["replay_version"], "contextops-replay-run/v1");
    let bindings = replay["bindings"].as_array().expect("bindings");
    assert_eq!(bindings.len(), 4);
    for binding in bindings {
        assert_eq!(
            binding["sealed_response_hash"],
            binding["recorded_response_sha256"]
        );
        assert_eq!(binding["matches_recorded_input"], true);
    }
    assert_eq!(
        replay["recordings"].as_array().expect("recordings").len(),
        4
    );
}

/// The deterministic experiment loop: identical inputs and a
/// deterministic model configuration produce identical semantic output,
/// excluding the declared volatile run fields (identity, timestamps, measured
/// latencies, and the record identifiers that carry them).
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn two_replay_runs_of_one_suite_agree_outside_the_declared_volatile_fields() {
    let suite = suite();
    // A fresh replay supply per run, which is what two CLI invocations do:
    // each process starts its own loopback origins on ephemeral ports. Any
    // origin-dependent value leaking into the summary, for example the
    // loopback port surfacing in acquisition.endpoint, makes the
    // runs disagree here. The gateway stays fixed across runs because it is
    // deployment configuration, not something the run starts.
    let (gateway, _calls) = gateway_origin();
    let run = |name: &str, directory: &tempfile::TempDir| {
        let supply =
            ReplaySupply::start(&recon(), &suite.providers).expect("manifest covers suite");
        let mut options = supply.run_options(directory.path().join(name));
        options.backend = Some(Box::new(
            TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
        ));
        execute(&suite, &options)
            .expect("the replay run should complete")
            .summary
    };
    let (first_dir, second_dir) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let mut first = run("first", &first_dir);
    let mut second = run("second", &second_dir);

    assert_eq!(
        first["run"]["manifest_hash"], second["run"]["manifest_hash"],
        "one suite must seal one manifest"
    );
    for summary in [&mut first, &mut second] {
        normalise(summary);
    }
    assert_eq!(
        first, second,
        "two replay runs of one suite disagreed on something that is not declared volatile"
    );
}

/// The determinism assertion above runs a suite that requires no citations,
/// so every grounding record it compares says `unevaluated` and `judge()`
/// never runs. This one requires them: a clock, an iteration order or a
/// sampling step introduced into the evaluator moves the record between two
/// replays of one suite and fails here.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn two_replay_runs_agree_on_the_grounding_records_they_computed() {
    let mut suite = suite();
    suite.require_cited_answer = true;
    let gateway = cited_gateway_origin();
    let run = |name: &str, directory: &tempfile::TempDir| {
        let supply =
            ReplaySupply::start(&recon(), &suite.providers).expect("manifest covers suite");
        let mut options = supply.run_options(directory.path().join(name));
        options.backend = Some(Box::new(
            TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
        ));
        execute(&suite, &options)
            .expect("the replay run should complete")
            .summary
    };
    let (first_dir, second_dir) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let mut first = run("first", &first_dir);
    let mut second = run("second", &second_dir);

    // Without this the comparison below would pass over records that judged
    // nothing, which is the hole this test exists to close.
    let mut judged = 0;
    for plan in first["plans"].as_array().expect("plans") {
        let grounding = &plan["evaluation"]["grounding"];
        if plan["answer"].is_null() {
            continue;
        }
        assert!(
            grounding["unevaluated"].is_null(),
            "a plan with an answer under a citation-requiring suite must be judged"
        );
        let counts = &grounding["verdict_counts"];
        let total: u64 = ["supported", "contradicted", "uncovered", "unavailable"]
            .iter()
            .map(|verdict| counts[verdict].as_u64().unwrap_or_default())
            .sum();
        assert!(total > 0, "the evaluator reached no verdict: {counts}");
        judged += 1;
    }
    assert!(judged > 0, "no plan produced an answer to judge");

    for summary in [&mut first, &mut second] {
        normalise(summary);
    }
    assert_eq!(
        first, second,
        "two replay runs of one suite disagreed on a computed grounding record"
    );
}

/// At run level, the summary's headline output must not be
/// a function of a declared-volatile figure. The suite's objective is
/// `minimise_latency` and every latency this run measured times a loopback
/// origin serving a recording, so the selection abstains naming latency
/// (`docs/contracts/run-output.md` §Selection) — the stable answer the
/// determinism test above then pins across two replays.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_replay_run_under_minimise_latency_abstains_naming_latency() {
    let suite = suite();
    let run = ReplayRun::execute(&suite);

    let selection = &run.summary["selection"];
    assert_eq!(selection["plan_id"], Value::Null);
    assert_eq!(selection["method"], "minimum observed latency");
    assert_eq!(selection["unavailable_inputs"], json!(["latency"]));
    assert!(
        selection["explanation"]
            .as_str()
            .expect("an explanation")
            .contains("loopback origin"),
        "the abstention must name what a replayed latency actually times: {}",
        selection["explanation"]
    );
}

/// The manifest seals the inference gateway's name — or null when none is
/// configured — because gateway presence is part of experiment identity: a
/// run whose answers came through a gateway and a run with no inference are
/// not the same experiment. The gateway's *endpoint* stays outside the hash;
/// that is deployment plumbing (`docs/contracts/run-output.md` §Manifest).
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn gateway_presence_is_sealed_as_part_of_experiment_identity() {
    let suite = suite();
    let (gateway, _calls) = gateway_origin();
    let run = |backend: Option<Box<dyn commonmeasure_inference::InferenceBackend>>| -> Value {
        let directory = tempfile::tempdir().expect("tempdir");
        let supply =
            ReplaySupply::start(&recon(), &suite.providers).expect("manifest covers suite");
        let mut options = supply.run_options(directory.path().join("run"));
        options.backend = backend;
        execute(&suite, &options).expect("the replay run should complete");
        serde_json::from_slice(
            &std::fs::read(directory.path().join("run/manifest.json")).expect("manifest.json"),
        )
        .expect("manifest JSON")
    };
    let with_gateway = run(Some(Box::new(
        TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
    )));
    let without_gateway = run(None);

    assert_eq!(with_gateway["manifest"]["inference_gateway"], "tensorzero");
    assert_eq!(
        without_gateway["manifest"]["inference_gateway"],
        Value::Null
    );
    assert_ne!(
        with_gateway["hash"], without_gateway["hash"],
        "a run with a gateway and a run with none must not claim to be the same experiment"
    );
}

/// A replay run, a run that asked for nothing and a live run are three
/// different experiments over one job, so the manifest must not call them the
/// same one — while two runs in the same mode still must agree.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn acquisition_mode_is_sealed_as_part_of_experiment_identity() {
    let suite = suite();
    let replayed = |label: &str| -> Value {
        let directory = tempfile::tempdir().expect("tempdir");
        let supply =
            ReplaySupply::start(&recon(), &suite.providers).expect("manifest covers suite");
        let options = supply.run_options(directory.path().join(label));
        execute(&suite, &options).expect("the replay run should complete");
        serde_json::from_slice(
            &std::fs::read(directory.path().join(label).join("manifest.json"))
                .expect("manifest.json"),
        )
        .expect("manifest JSON")
    };
    // No replay context and no authority to call out: the run attempts no
    // acquisition at all, which is the mode that must not share replay's hash.
    let unconfigured = {
        let directory = tempfile::tempdir().expect("tempdir");
        let options = RunOptions {
            allow_external_acquisition: false,
            output: directory.path().join("run"),
            suppliers: Box::new(|provider: &str| {
                Err(commonmeasure_supply::SupplyError::CredentialMissing {
                    variable: format!("{provider}_KEY_ABSENT"),
                })
            }),
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        execute(&suite, &options).expect("the run should complete");
        serde_json::from_slice::<Value>(
            &std::fs::read(directory.path().join("run/manifest.json")).expect("manifest.json"),
        )
        .expect("manifest JSON")
    };

    let first = replayed("one");
    let second = replayed("two");

    assert_eq!(first["manifest"]["acquisition_mode"], "replay");
    assert_eq!(
        unconfigured["manifest"]["acquisition_mode"],
        "no-external-acquisition"
    );
    assert_ne!(
        first["hash"], unconfigured["hash"],
        "a run served from recordings and a run that acquired nothing are not one experiment"
    );
    // The recordings that answered are part of the identity, and they are
    // digested rather than named, so the operator's directory path cannot
    // reach the hash.
    assert!(
        first["manifest"]["replay_recordings"].is_string(),
        "a replay run seals which recordings answered"
    );
    assert_eq!(unconfigured["manifest"]["replay_recordings"], Value::Null);

    // The trap: identity must separate the modes without becoming volatile
    // inside one. Two replay runs of one suite still agree.
    assert_eq!(
        first["hash"], second["hash"],
        "two replay runs of one suite must remain the same experiment"
    );
}

/// Null out the declared volatile fields, and only those, wherever they occur.
fn normalise(summary: &mut Value) {
    summary["run"]["id"] = Value::Null;
    summary["run"]["started_at"] = Value::Null;
    summary["evidence_log"]["sha256"] = Value::Null;
    for plan in summary["plans"].as_array_mut().expect("plans") {
        plan["latency_ms"] = Value::Null;
        if plan["acquisition"].is_object() {
            plan["acquisition"]["latency_ms"] = Value::Null;
        }
        if plan["inference"].is_object() {
            plan["inference"]["latency_ms"] = Value::Null;
        }
        for decision in plan["policy_decisions"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            decision["id"] = Value::Null;
            decision["run_id"] = Value::Null;
            decision["timestamp"] = Value::Null;
        }
        for invocation in plan["processors"].as_array_mut().into_iter().flatten() {
            for field in ["timestamp", "started_at", "finished_at", "latency_ms"] {
                invocation[field] = Value::Null;
            }
        }
    }
}

/// A provider without a recording fails before anything runs. The internal
/// corpus is the standing example: its `query` is a local read with no
/// recorded acquisition, and replay refuses to substitute one.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_provider_without_a_recording_refuses_to_start() {
    let error = ReplaySupply::start(&recon(), &["internal".to_owned()])
        .err()
        .expect("no recording exists for a corpus query");
    match &error {
        ReplayError::RecordingMissing {
            provider,
            capability,
        } => {
            assert_eq!(provider, "internal");
            assert_eq!(capability, "query");
        }
        other => panic!("expected RecordingMissing, got {other:?}"),
    }
    assert!(
        error.to_string().contains("never falls back"),
        "the refusal must say there is no fallback: {error}"
    );
}

#[test]
fn a_directory_without_a_manifest_refuses_to_start() {
    let empty = tempfile::tempdir().expect("tempdir");
    let error = ReplaySupply::start(empty.path(), &["exa".to_owned()])
        .err()
        .expect("no manifest, no replay");
    assert!(
        matches!(error, ReplayError::ManifestUnreadable { .. }),
        "{error:?}"
    );
}

/// A capture that no longer matches the hash the manifest declared is not
/// served. Recon artefacts are evidence; a replay vouches for every byte it
/// serves or refuses to serve it.
#[test]
fn an_altered_capture_refuses_to_serve() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        directory.path().join("altered.json"),
        serde_json::to_vec(&json!({
            "response": {"status": 200, "body": {"results": []}}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("replay-manifest.json"),
        serde_json::to_vec(&json!({
            "manifest_version": "contextops-replay/v1",
            "derived_from": "synthetic test data",
            "recordings": [{
                "provider": "exa",
                "capability": "search",
                "source": "altered.json",
                "captured_at": "2026-08-01",
                "recorded_endpoint": "https://api.exa.ai/search",
                "response_sha256": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                "redactions": "none",
                "permitted_use": "test"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let error = ReplaySupply::start(directory.path(), &["exa".to_owned()])
        .err()
        .expect("a hash mismatch must refuse");
    assert!(
        matches!(error, ReplayError::HashMismatch { .. }),
        "{error:?}"
    );
}

/// A capture that says it answered one URL, declared in the manifest as
/// another, refuses to serve.
///
/// The manifest's `recorded_endpoint` is what a replayed plan publishes as
/// the URL it acquired from, and a reader following that URL to the capture's
/// own record must find the same one. Trusting the manifest alone would let a
/// recording of one endpoint be sealed against the name of another.
#[test]
fn a_capture_answering_another_url_refuses_to_serve() {
    let directory = tempfile::tempdir().expect("tempdir");
    let body = json!({"results": []});
    std::fs::write(
        directory.path().join("moved.json"),
        serde_json::to_vec(&json!({
            "request": {"method": "POST", "url": "https://api.exa.ai/contents"},
            "response": {"status": 200, "body": body}
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = commonmeasure_types::canonical::sha256_digest(
        commonmeasure_types::canonical::canonical_json(&body).as_bytes(),
    );
    std::fs::write(
        directory.path().join("replay-manifest.json"),
        serde_json::to_vec(&json!({
            "manifest_version": "contextops-replay/v1",
            "derived_from": "synthetic test data",
            "recordings": [{
                "provider": "exa",
                "capability": "search",
                "source": "moved.json",
                "captured_at": "2026-08-01",
                "recorded_endpoint": "https://api.exa.ai/search",
                "response_sha256": hash,
                "redactions": "none",
                "permitted_use": "test"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let error = ReplaySupply::start(directory.path(), &["exa".to_owned()])
        .err()
        .expect("an endpoint the capture disagrees with must refuse");
    match &error {
        ReplayError::EndpointMismatch {
            source,
            declared,
            recorded,
        } => {
            assert_eq!(source, "moved.json");
            assert_eq!(declared, "https://api.exa.ai/search");
            assert_eq!(recorded, "https://api.exa.ai/contents");
        }
        other => panic!("expected an endpoint mismatch, got {other:?}"),
    }
    // The hash agrees, so the refusal is about the endpoint alone.
    assert!(error.to_string().contains("nobody called"), "{error}");
}

/// A provider whose published search page is smaller than the job's result
/// limit answers at its own size, and the plan records the shortfall naming
/// both numbers.
///
/// TollBit publishes a page of 20. A job asking for more has its request
/// reduced to that page; without the gap, one plan in a comparison would run
/// at 20 while the others ran at the job's own size with nothing in the record
/// saying so.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_provider_page_smaller_than_the_job_asked_for_is_recorded_as_a_gap() {
    let mut suite = suite();
    suite.result_limit = 25;
    let run = ReplayRun::execute(&suite);

    let plan = run.plan("tollbit-only");
    let gaps: Vec<&str> = plan["gaps"]
        .as_array()
        .expect("gaps")
        .iter()
        .filter_map(|gap| gap["detail"].as_str())
        .collect();
    let clamp = gaps
        .iter()
        .find(|detail| detail.contains("search page"))
        .unwrap_or_else(|| panic!("no gap named the clamp: {gaps:?}"));
    assert!(
        clamp.contains("25") && clamp.contains("20"),
        "the gap must name both numbers: {clamp}"
    );

    // A provider that publishes no ceiling records no such gap.
    let exa: Vec<&str> = run.plan("exa-only")["gaps"]
        .as_array()
        .expect("gaps")
        .iter()
        .filter_map(|gap| gap["detail"].as_str())
        .collect();
    assert!(
        !exa.iter().any(|detail| detail.contains("search page")),
        "{exa:?}"
    );
}

/// The runtime's own guard: a replay context that does not cover the suite is
/// a suite error before any plan executes, never a weaker successful run.
#[test]
fn execute_refuses_a_replay_run_whose_context_misses_a_provider() {
    let directory = tempfile::tempdir().expect("tempdir");
    let options = RunOptions {
        allow_external_acquisition: false,
        output: directory.path().join("replay"),
        suppliers: Box::new(|provider| {
            panic!("no supplier may be resolved for {provider}: the run must refuse first")
        }),
        backend: None,
        replay: Some(ReplayContext {
            manifest_path: "unused".to_owned(),
            recordings: std::collections::BTreeMap::new(),
            served: std::collections::BTreeMap::new(),
        }),
        allowance: None,
        provenance_signing:
            commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
    };
    let mut suite = suite();
    suite.providers = vec!["exa".into()];
    match execute(&suite, &options).map(|report| report.run_id) {
        Err(RunError::Suite(detail)) => {
            assert!(detail.contains("exa"), "{detail}");
            assert!(detail.contains("never substitutes"), "{detail}");
        }
        other => panic!("expected a suite error, got {other:?}"),
    }
}

/// The first licensed provider replays its whole quote-then-buy exchange:
/// five recorded legs behind one routed origin, the quote in evidence before
/// the recorded settlement, and every sealed exchange — the quote legs
/// included — bound to its recording by hash equality a reader can recheck.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_quoted_provider_replays_all_five_legs_and_binds_the_quote_to_its_recordings() {
    let mut suite = suite();
    // The recorded preview executed media--sample for keyword NVIDIA with
    // limit 3; the replayed suite asks for exactly what the recording
    // answers.
    suite.job.prompt = "NVIDIA".into();
    suite.result_limit = 3;
    suite.providers = vec!["redpine".into()];
    let run = ReplayRun::execute(&suite);

    assert_eq!(run.summary["run"]["mode"], "replay");
    let plan = run.plan("redpine-only");
    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["verification_state"], "replay-tested");

    // The main sealed response is the recorded settlement.
    let acquisition = &plan["acquisition"];
    assert_eq!(
        acquisition["replay"]["source"],
        "redpine/redpine-confirm-sample.json"
    );
    assert_eq!(acquisition["replay"]["matches_recorded_input"], true);
    assert_eq!(acquisition["replay"]["captured_at"], "2026-08-04");
    assert_eq!(acquisition["endpoint"], "https://api.redpine.ai/mcp");

    // The quote rode the same replay: trial-covered, decision confirmed,
    // and the receipt recorded as trial coverage rather than free.
    let quote = &acquisition["quote"];
    assert_eq!(quote["billing_status"], "trial");
    assert_eq!(quote["decision"], "confirmed");
    assert_eq!(acquisition["charge"]["native"]["unit"], "trial_queries");

    // replay.json binds the quote legs beside the settlement, and lists all
    // five recordings' provenance.
    let replay: Value = serde_json::from_slice(
        &std::fs::read(run.published().join("replay.json")).expect("replay.json"),
    )
    .expect("replay.json is JSON");
    let redpine_recordings = replay["recordings"]
        .as_array()
        .expect("recordings")
        .iter()
        .filter(|recording| recording["provider"] == "redpine")
        .count();
    assert_eq!(redpine_recordings, 5, "every served leg is listed");
    let binding = replay["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .find(|binding| binding["provider"] == "redpine")
        .expect("the redpine binding");
    let quote_bindings = binding["quote_bindings"].as_array().expect("quote legs");
    assert_eq!(quote_bindings.len(), 2);
    for leg in quote_bindings {
        assert_eq!(
            leg["matches_recorded_input"], true,
            "sealed quote leg {} must match its recording",
            leg["leg"]
        );
    }
}

/// A quote-then-buy replay needs every leg before it starts: running four
/// legs and failing at the settlement would be the weaker mid-plan mode
/// replay refuses to have.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_quoted_provider_with_a_missing_leg_refuses_to_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(directory.path().join("redpine")).expect("mkdir");
    let committed: Value = serde_json::from_slice(
        &std::fs::read(recon().join("replay-manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let kept: Vec<&Value> = committed["recordings"]
        .as_array()
        .expect("recordings")
        .iter()
        .filter(|recording| {
            recording["provider"] == "redpine" && recording["capability"] != "confirm"
        })
        .collect();
    for recording in &kept {
        let source = recording["source"].as_str().expect("source");
        std::fs::copy(recon().join(source), directory.path().join(source)).expect("copy capture");
    }
    std::fs::write(
        directory.path().join("replay-manifest.json"),
        serde_json::to_vec(&json!({
            "manifest_version": "contextops-replay/v1",
            "derived_from": "test: the redpine legs minus the settlement",
            "recordings": kept,
        }))
        .expect("manifest bytes"),
    )
    .expect("write manifest");

    let error = ReplaySupply::start(directory.path(), &["redpine".to_owned()])
        .err()
        .expect("a missing settlement leg must refuse");
    match &error {
        ReplayError::RecordingMissing {
            provider,
            capability,
        } => {
            assert_eq!(provider, "redpine");
            assert_eq!(capability, "confirm");
        }
        other => panic!("expected RecordingMissing, got {other:?}"),
    }
}
