//! Fixture tests over real HTTP, supply adapters, policy and inference request
//! construction. Controlled responses establish the workflow, not live scores.

use commonmeasure_http::{Response, Server};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::{
    RunOptions, Suite,
    benchmark::{self, Benchmark},
    processor::provenance::SigningIdentity,
};
use commonmeasure_supply::ExaAdapter;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const SMOKE: &[u8] = include_bytes!("../../../demo/benchmarks/simpleqa/smoke.json");

fn suite(denied: bool) -> Suite {
    serde_json::from_value(json!({"suite_version":"test/v1","label":"SimpleQA fixture",
        "job":{"id":"b883c63a-736f-4cf2-a4e5-7f856d2c3d64","kind":"research.answer",
        "prompt":"unused", "policy_mode":"strict", "objective":{"kind":"maximise_quality"},
        "constraints": if denied {json!([{"kind":"denied_provider","provider":"exa"}])} else {json!([])}, "evidence_requirements":[]},
        "model_plan":{"name":"fixture-answer","version":"1","model":"answer-model"},
        "result_limit":5,"providers":["exa"]})).unwrap()
}

fn batch(suite: Suite) -> Benchmark<'static> {
    Benchmark {
        dataset_bytes: SMOKE,
        judge_model: commonmeasure_types::ModelPlan {
            name: "fixture-grader".into(),
            version: "1".into(),
            model: "judge-model".into(),
        },
        suite,
        limit: 1,
        seed: 0,
        policy_identity: json!({"fixture":true}),
    }
}

#[test]
fn actual_adapters_separate_gold_from_answers_and_preserve_grader_evidence() {
    let search_calls = Arc::new(AtomicUsize::new(0));
    let observed_search = search_calls.clone();
    let search_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed_requests = search_requests.clone();
    let provider = Server::bind("127.0.0.1:0").unwrap().spawn(move |request| {
        observed_search.fetch_add(1, Ordering::SeqCst);
        observed_requests.lock().unwrap().push(serde_json::from_slice(&request.body).unwrap());
        Response::json(200, r#"{"results":[{"url":"https://example.org/public","title":"Public source","text":"Public fixture information supporting a short answer."}]}"#)
    }).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let gateway = Server::bind("127.0.0.1:0").unwrap().spawn(move |request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let reply = if body["model"] == "judge-model" {"A"} else {"Fixture prediction"};
        observed.lock().unwrap().push(body);
        Response::json(200, &json!({"model":"fixture-executed-model","choices":[{"message":{"content":reply}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}).to_string())
    }).unwrap();
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("batch");
    let config = batch(suite(false));
    let summary = benchmark::execute(&config, &output, |path| {
        let url = provider.url();
        RunOptions {
            output: path.to_owned(),
            allow_external_acquisition: true,
            suppliers: Box::new(move |_| Ok(Box::new(ExaAdapter::new(&url, "fixture-key")))),
            backend: Some(Box::new(TensorZeroBackend::new(gateway.url()).unwrap())),
            replay: None,
            allowance: None,
            provenance_signing: SigningIdentity::Unconfigured,
        }
    })
    .unwrap();
    assert_eq!(search_calls.load(Ordering::SeqCst), 1);
    assert_eq!(summary["providers"][1]["correct"], 1);
    assert_eq!(summary["providers"][1]["accuracy_all_selected"], 1.0);
    let dataset: benchmark::Dataset = serde_json::from_slice(SMOKE).unwrap();
    let case = dataset.sample(1, 0).unwrap()[0];
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4); // baseline + provider answers and their graders
    for request in requests.iter() {
        let text = request["messages"].to_string();
        if request["model"] == "answer-model" {
            assert!(text.contains(&case.question));
            assert!(!text.contains(&case.reference_answer));
        } else {
            assert!(text.contains(&case.reference_answer));
            assert!(text.contains("Fixture prediction"));
        }
        assert_eq!(request["temperature"], 0);
    }
    let searches = search_requests.lock().unwrap();
    assert_eq!(searches[0]["query"], case.question);
    assert!(!searches[0].to_string().contains(&case.reference_answer));
    let private: Value =
        serde_json::from_slice(&std::fs::read(output.join("results.private.json")).unwrap())
            .unwrap();
    assert_eq!(
        private[1]["judge"]["detail"]["response"]["executed_model"],
        "fixture-executed-model"
    );
    assert!(private[1]["judge"]["detail"]["cost"]["money"].is_null());
    for filename in ["summary.json", "summary.csv"] {
        let public = std::fs::read_to_string(output.join(filename)).unwrap();
        for secret in [
            &case.question,
            &case.reference_answer,
            "Fixture prediction",
            "fixture-key",
            "https://example.org/public",
        ] {
            assert!(
                !public.contains(secret),
                "{filename} contains private case content"
            );
        }
    }
    assert!(
        benchmark::execute(&config, &output, |_| panic!(
            "existing output must fail before calls"
        ))
        .is_err()
    );
}

#[test]
fn refusal_and_malformed_judge_stay_distinct_and_no_supplier_is_called() {
    let gateway = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(|_| {
            Response::json(
                200,
                r#"{"choices":[{"message":{"content":"not a grade"}}]}"#,
            )
        })
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("refused");
    let report = benchmark::execute(&batch(suite(true)), &output, |path| RunOptions {
        output: path.to_owned(),
        allow_external_acquisition: true,
        suppliers: Box::new(|_| panic!("provider must be refused before adapter resolution")),
        backend: Some(Box::new(TensorZeroBackend::new(gateway.url()).unwrap())),
        replay: None,
        allowance: None,
        provenance_signing: SigningIdentity::Unconfigured,
    })
    .unwrap();
    assert_eq!(report["providers"][1]["refused"], 1);
    assert_eq!(report["providers"][1]["unmeasured"], 1);
    assert_eq!(report["providers"][0]["completed"], 1);
    assert_eq!(report["providers"][0]["unmeasured"], 1);
    assert_eq!(report["providers"][0]["not_attempted"], 0);
    assert!(report["providers"][0]["accuracy_measured"].is_null());
    let events = std::fs::read_to_string(output.join("evidence.ndjson")).unwrap();
    assert!(events.contains("benchmark_finished"));
    assert!(events.contains("exactly A, B or C"));
}

#[test]
fn interrupted_case_does_not_dispatch_later_cases_or_publish_success() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("interrupted");
    let calls = AtomicUsize::new(0);
    let mut config = batch(suite(false));
    config.limit = 2;
    std::fs::write(root.path().join("obstruction"), b"not a directory").unwrap();
    let result = benchmark::execute(&config, &output, |_path| {
        calls.fetch_add(1, Ordering::SeqCst);
        // Obstruct the run directory's staging parent; this is a real IO
        // failure before dispatch, not a simulated successful source record.
        RunOptions {
            output: root.path().join("obstruction/child"),
            allow_external_acquisition: false,
            suppliers: Box::new(|_| panic!("no supplier call")),
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing: SigningIdentity::Unconfigured,
        }
    });
    assert!(result.unwrap_err().contains("evidence IO failure"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!output.join("summary.json").exists());
    let evidence = std::fs::read_to_string(output.join("evidence.ndjson")).unwrap();
    assert!(evidence.contains("case_started"));
    assert!(!evidence.contains("benchmark_finished"));
}
