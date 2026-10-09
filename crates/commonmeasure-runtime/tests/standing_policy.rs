//! Fixture-tested admission through the real Exa transport and parser aimed
//! at a loopback origin. No external supplier or inference call is made.
//! Managed activation and identity are tested through the CLI separately.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use commonmeasure_http::{Response, Server};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_runtime::processor::provenance::SigningIdentity;
use commonmeasure_runtime::{ResolvedSourcePolicy, RunOptions, Suite, execute};
use commonmeasure_supply::ExaAdapter;
use commonmeasure_types::PolicyMode;
use serde_json::{Value, json};

fn fixture(
    home: Value,
    suite_constraints: Value,
    mode: PolicyMode,
    target: Option<&str>,
) -> (Value, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    let origin = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_| {
            requests.fetch_add(1, Ordering::SeqCst);
            Response::json(200, &json!({
            "requestId":"fixture-request",
            "results":[{"url":"https://forbidden.example/article", "title":"Fixture",
                "text":"The fixture source supplies enough text to exceed a one token budget."}],
            "costDollars":{"total":0.004},
        }).to_string())
        })
        .unwrap();
    let base = origin.url();
    let directory = tempfile::tempdir().unwrap();
    let mut suite: Suite = serde_json::from_value(json!({
        "suite_version":"standing-policy-fixture/v1", "label":"Standing policy fixture",
        "job":{"id":"3d8beea4-eb91-4f2d-9426-2fa2f06fef8b", "kind":"research.answer",
            "prompt":"fixture source", "policy_mode":mode,
            "objective":{"kind":"maximise_quality"}, "constraints":suite_constraints,
            "evidence_requirements":[]},
        "model_plan":{"name":"fixture", "version":"1", "model":"unconfigured"},
        "result_limit":1, "providers":["exa"],
    }))
    .unwrap();
    suite.fetch_target = target.map(str::to_owned);
    let mut job = suite.job.clone();
    job.policy_mode = serde_json::from_value(home["policy_mode"].clone()).unwrap();
    job.constraints = serde_json::from_value(home["constraints"].clone()).unwrap();
    let options = RunOptions {
        allow_external_acquisition: true,
        output: directory.path().join("run"),
        suppliers: Box::new(move |_| Ok(Box::new(ExaAdapter::new(&base, "fixture-key")))),
        backend: None,
        replay: None,
        allowance: None,
        source_policy: Some(ResolvedSourcePolicy {
            job,
            authority: Ruling::Allowed,
            record: json!({"fixture":"standing policy"}),
        }),
        provenance_signing: SigningIdentity::Unconfigured,
    };
    let report = execute(&suite, &options).unwrap();
    let evidence = std::fs::read_to_string(report.output.join("evidence.ndjson")).unwrap();
    let records: Vec<Value> = evidence
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let plan = report.summary["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["provider"] == "exa")
        .unwrap();
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "plan_completed" && record["payload"] == *plan)
    );
    (plan.clone(), calls.load(Ordering::SeqCst))
}

#[test]
fn provider_allow_lists_are_not_unioned() {
    let (plan, calls) = fixture(
        json!({"policy_mode":"strict", "constraints":[
        {"kind":"allowed_provider", "provider":"tavily"}]}),
        json!([{"kind":"allowed_provider", "provider":"exa"}]),
        PolicyMode::Observe,
        None,
    );
    assert_eq!(calls, 0);
    assert_eq!(plan["status"], "refused");
    assert!(plan["acquisition"].is_null());
    assert_eq!(plan["policy_decisions"][0]["policy_basis"], "source_policy");
}

#[test]
fn suite_exceptions_and_ordered_rules_cannot_relax_home_admission() {
    let relaxing = json!([
        {"kind":"allowed_source_provider", "provider":"exa"},
        {"kind":"access_rule", "host":"*", "action":"allow"},
    ]);
    for constraints in [
        json!([{"kind":"allowed_source_host", "host":"allowed.example"}]),
        json!([{"kind":"denied_source_host", "host":"forbidden.example"}]),
        json!([{"kind":"access_rule", "host":"*.example", "action":"refuse"}]),
        json!([{"kind":"required_licence", "licence":"operator-agreement"}]),
    ] {
        let (plan, calls) = fixture(
            json!({"policy_mode":"strict", "constraints":constraints}),
            relaxing.clone(),
            PolicyMode::Observe,
            None,
        );
        assert_eq!(calls, 1, "source checks follow the real provider response");
        assert_eq!(plan["status"], "unavailable");
        assert_eq!(plan["policy_decisions"][0]["decision"], "refuse");
        assert_eq!(plan["source_count"], 0);
        assert_eq!(plan["sources"][0]["admitted"], false);
        assert_eq!(plan["policy_decisions"][0]["policy_basis"], "source_policy");
    }
}

#[test]
fn suite_can_only_narrow_mode_and_source_lists() {
    let home = json!({"policy_mode":"observe", "constraints":[
        {"kind":"denied_source_host", "host":"forbidden.example"}]});
    let (observed, _) = fixture(home.clone(), json!([]), PolicyMode::Observe, None);
    assert_eq!(observed["source_count"], 1);
    assert_eq!(observed["policy_decisions"][0]["decision"], "admit");
    let (narrowed, _) = fixture(home, json!([]), PolicyMode::Strict, None);
    assert_eq!(narrowed["source_count"], 0);
    assert_eq!(narrowed["status"], "unavailable");
    assert_eq!(narrowed["policy_decisions"][0]["decision"], "refuse");
    let (disjoint, _) = fixture(
        json!({"policy_mode":"strict", "constraints":[
        {"kind":"allowed_source_host", "host":"forbidden.example"}]}),
        json!([{"kind":"allowed_source_host", "host":"other.example"}]),
        PolicyMode::Observe,
        None,
    );
    assert_eq!(disjoint["source_count"], 0);
    assert_eq!(disjoint["policy_decisions"][0]["policy_basis"], "suite");
}

#[test]
fn known_disallowed_fetch_target_never_reaches_the_supplier() {
    let (plan, calls) = fixture(
        json!({"policy_mode":"strict", "constraints":[
        {"kind":"denied_source_host", "host":"forbidden.example"}]}),
        json!([{"kind":"allowed_source_provider", "provider":"exa"}]),
        PolicyMode::Observe,
        Some("https://forbidden.example/article"),
    );
    assert_eq!(calls, 0);
    assert_eq!(plan["status"], "refused");
    assert!(plan["acquisition"].is_null());
    assert_eq!(
        plan["policy_decisions"][0]["source_url"],
        "https://forbidden.example/article"
    );
}

#[test]
fn home_context_budget_is_not_relaxed_by_the_suite() {
    let (plan, calls) = fixture(
        json!({"policy_mode":"strict", "constraints":[
        {"kind":"maximum_context_tokens", "tokens":1}]}),
        json!([{"kind":"maximum_context_tokens", "tokens":1000}]),
        PolicyMode::Observe,
        None,
    );
    assert_eq!(calls, 1);
    assert_eq!(plan["source_count"], 0);
    assert_eq!(plan["context_tokens_admitted"], 0);
    assert!(
        plan["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["reason"] == "budget_exhausted")
    );
}
