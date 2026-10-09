//! People Inc synthetic fixtures through planning, dispatch, admission and
//! sealing. The resolver changes only the origin, not the implementation.
//! No live provider or inference integration is claimed. The runner's live
//! flag permits dispatch to loopback and is not evidence of supplier access.

use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server};
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::remote_adapter;
use commonmeasure_types::{Constraint, ContextJob, ModelPlan, Objective, PolicyMode};
use serde_json::{Value, json};
use uuid::Uuid;

const TARGET: &str = "https://www.allrecipes.com/article/synthetic/";

fn suite(fetch: bool, limit: u32, constraints: Vec<Constraint>) -> Suite {
    Suite {
        suite_version: "peopleinc-fixture/v1".into(),
        label: "People Inc synthetic fixture".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "Synthetic question".into(),
            policy_mode: PolicyMode::Strict,
            objective: Objective::MinimiseLatency,
            constraints,
            evidence_requirements: vec![],
        },
        model_plan: ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "unconfigured".into(),
        },
        result_limit: limit,
        providers: vec!["peopleinc".into()],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        fetch_target: fetch.then(|| TARGET.into()),
        fidelity_judge: false,
        output_provenance: None,
    }
}

struct FixtureRun {
    _directory: tempfile::TempDir,
    output: std::path::PathBuf,
    summary: Value,
    seen: Vec<(String, Value)>,
    raw: Vec<u8>,
}

fn run(suite: &Suite) -> FixtureRun {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let body = if suite.fetch_target.is_some() {
        json!({"status":200,"markdown":"# Synthetic article\n\nFixture text."})
    } else {
        json!({"documents":[{"doc_id":"synthetic", "url":TARGET, "rank":1, "domain":"Lifestyle",
            "content":{"markdown":"# Synthetic article\n\nFixture text."}}]})
    };
    let raw = serde_json::to_vec(&body).unwrap();
    let served = raw.clone();
    let origin = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            recorder.lock().unwrap().push((
                request.target.clone(),
                serde_json::from_slice(&request.body).unwrap_or(Value::Null),
            ));
            Response::new(200, served.clone())
        })
        .unwrap();
    let url = origin.url();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("published");
    let options = RunOptions {
        allow_external_acquisition: true,
        output: output.clone(),
        suppliers: Box::new(move |provider| {
            Ok(remote_adapter(provider, Some(&url), "synthetic-peopleinc-key").unwrap())
        }),
        backend: None,
        replay: None,
        allowance: None,
        source_policy: None,
        provenance_signing:
            commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
    };
    let report = execute(suite, &options).unwrap();
    let requests = seen.lock().unwrap().clone();
    FixtureRun {
        _directory: directory,
        output,
        summary: report.summary,
        seen: requests,
        raw,
    }
}

fn plan(run: &FixtureRun) -> &Value {
    run.summary["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["id"] == "peopleinc-only")
        .unwrap()
}

#[test]
fn fetch_target_reaches_peopleinc_and_seals_the_served_bytes() {
    let run = run(&suite(true, 3, vec![]));
    assert_eq!(run.seen.len(), 1);
    let called = url::Url::parse(&format!("http://fixture{}", run.seen[0].0)).unwrap();
    assert_eq!(called.path(), "/v1/markdown");
    assert_eq!(
        called.query_pairs().collect::<Vec<_>>(),
        vec![("url".into(), TARGET.into())]
    );
    let plan = plan(&run);
    assert_eq!(plan["capability"], "fetch");
    assert_eq!(plan["source_count"], 1);
    assert_eq!(plan["sources"][0]["url"], TARGET);
    assert_eq!(plan["sources"][0]["admitted"], true);
    assert_eq!(plan["acquisition"]["charge"], json!({}));
    assert_eq!(plan["acquisition"]["http_status"], 200);
    assert_eq!(
        std::fs::read(run.output.join("responses/peopleinc-only.json")).unwrap(),
        run.raw
    );
    assert!(
        !std::fs::read_to_string(run.output.join("summary.json"))
            .unwrap()
            .contains("synthetic-peopleinc-key")
    );
}

#[test]
fn search_ceiling_is_recorded_as_coverage_evidence() {
    let run = run(&suite(false, 9, vec![]));
    assert_eq!(run.seen.len(), 1);
    assert_eq!(run.seen[0].1["limit"], 5);
    let plan = plan(&run);
    assert_eq!(plan["capability"], "search");
    let gaps = plan["gaps"].as_array().unwrap();
    assert!(
        gaps.iter().any(|gap| gap["reason"] == "coverage_gap"
            && gap.to_string().contains('9')
            && gap.to_string().contains('5')),
        "{gaps:?}"
    );
}

#[test]
fn denied_peopleinc_provider_is_refused_before_transport() {
    for fetch in [false, true] {
        let run = run(&suite(
            fetch,
            3,
            vec![Constraint::DeniedProvider {
                provider: "peopleinc".into(),
            }],
        ));
        assert!(run.seen.is_empty());
        assert_eq!(plan(&run)["status"], "refused");
    }
}

#[test]
fn returned_policy_host_is_checked_instead_of_the_domain_label() {
    let run = run(&suite(
        false,
        3,
        vec![Constraint::DeniedSourceHost {
            host: "www.allrecipes.com".into(),
        }],
    ));
    assert_eq!(run.seen.len(), 1);
    let plan = plan(&run);
    assert_eq!(plan["source_count"], 0);
    assert_eq!(plan["sources"][0]["admitted"], false);
    assert!(
        plan["sources"][0]["admission_reason"]
            .as_str()
            .unwrap()
            .contains("www.allrecipes.com")
    );
}
