//! The routing experiment's failure path (`demo/jobs/commerce-routing/README.md`,
//! `docs/FAIL-POLICY.md` §5): a holdout job whose frozen-rule route is
//! unavailable publishes that outcome explicitly — an unavailable plan with
//! a gap naming the unreadable corpus root — never a silent fallback to
//! another corpus, a zeroed measurement or an invented answer.

use std::path::Path;

use commonmeasure_runtime::{RunOptions, execute, load_suite};
use commonmeasure_supply::{InternalCorpusAdapter, SupplyAdapter};
use serde_json::Value;

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read committed artefact"))
        .expect("parse committed artefact")
}

#[test]
fn a_holdout_job_whose_frozen_route_is_unavailable_publishes_the_outcome() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let jobs = repository.join("demo/jobs/commerce-routing");

    // The job under test comes from the registration, not from this file: a
    // holdout question of the availability class, run under the condition
    // the frozen rule actually routes that class to.
    let split = read(&jobs.join("split.json"));
    let rule = read(&jobs.join("rule.json"));
    let question = split["split"]["availability"]["holdout"][0]
        .as_str()
        .expect("a registered holdout question");
    let route = rule["routes"]["availability"]["condition"]
        .as_str()
        .expect("the frozen route");
    let suite = load_suite(&jobs.join(format!("{route}-{question}.json")))
        .expect("the registered suite loads");

    // The route is unavailable: the real adapter, rooted at a directory that
    // does not exist. Nothing else is resolvable, so a fallback would fail
    // the run rather than pass unnoticed.
    let directory = tempfile::tempdir().expect("tempdir");
    let missing = directory.path().join("missing-corpus");
    let missing_root = missing.clone();
    let output = directory.path().join("run");
    let report = execute(
        &suite,
        &RunOptions {
            allow_external_acquisition: false,
            output: output.clone(),
            suppliers: Box::new(move |_| {
                Ok(Box::new(InternalCorpusAdapter::new(&missing_root)) as Box<dyn SupplyAdapter>)
            }),
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        },
    )
    .expect("the run publishes; an unavailable route is an outcome, not a crash");

    let summary = report.summary;
    let plan = summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["provider"] == "internal")
        .expect("the routed plan is in the record");

    assert_eq!(plan["status"], "unavailable");
    let gaps = plan["gaps"].as_array().expect("gaps");
    let named = gaps.iter().any(|gap| {
        gap["reason"] == "provider_unavailable"
            && gap["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(&missing.display().to_string()))
    });
    assert!(
        named,
        "the gap must name the unreadable corpus root: {gaps:?}"
    );

    // Never a weaker successful mode: no source admitted from anywhere, no
    // token count invented, coverage unmeasured with its reason rather than
    // zero, and no answer.
    assert_eq!(plan["sources"].as_array().expect("sources").len(), 0);
    assert_eq!(plan["context_tokens_admitted"], Value::Null);
    assert_eq!(plan["evaluation"]["coverage"]["fraction"], Value::Null);
    assert!(
        plan["evaluation"]["coverage"]["unmeasured"]
            .as_str()
            .is_some_and(|reason| reason.contains("no admitted window")),
        "coverage abstains with its reason"
    );
    assert_eq!(plan["answer"], Value::Null);

    // The outcome is published, not just returned: the sealed record is on
    // disk for a reader.
    assert!(output.join("manifest.json").is_file());
    assert!(output.join("summary.json").is_file());
    assert!(output.join("evidence.ndjson").is_file());
}
