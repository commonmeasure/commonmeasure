//! The rankable measures, exercised end to end over the committed replay
//! captures and the committed rubric suite — the rankable-evaluation demonstration:
//! the evaluation changes a routing decision, on committed evidence, offline.
//!
//! No gateway is configured anywhere here. The suite's objective weights only
//! coverage and freshness, both measured on the admitted window, so the
//! router ranks the bought context without any plan running to an answer —
//! and the determinism proof holds the records to being byte-identical
//! across two replay runs, because both evaluators are pure functions of
//! suite input and window content.

use std::path::{Path, PathBuf};

use commonmeasure_runtime::{ReplaySupply, Suite, execute, load_suite};
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-runtime sits two levels below the repository root")
        .to_path_buf()
}

fn rubric_suite() -> Suite {
    load_suite(&repo_root().join("demo/jobs/eu-ai-act-replay-rubric.json"))
        .expect("the committed rubric suite loads")
}

/// One replay run of the committed suite, no gateway configured.
fn replay_run(suite: &Suite) -> Value {
    let supply = ReplaySupply::start(&repo_root().join("demo/recon"), &suite.providers)
        .expect("the committed manifest covers every suite provider");
    let directory = tempfile::tempdir().expect("tempdir");
    let mut options = supply.run_options(directory.path().join("run"));
    // Explicitly none, whatever the environment holds: the demonstration is
    // that the coverage/freshness objective ranks without an answer.
    options.backend = None;
    execute(suite, &options)
        .expect("the replay run should complete")
        .summary
}

fn plan<'a>(summary: &'a Value, id: &str) -> &'a Value {
    summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == id)
        .unwrap_or_else(|| panic!("no plan {id}"))
}

/// The committed demonstration: the router selects on measured coverage and
/// freshness, offline, with no plan holding an answer — a populated
/// selection whose explanation names the rubric that chose the plan.
#[test]
fn the_committed_rubric_suite_selects_a_plan_offline() {
    let suite = rubric_suite();
    let summary = replay_run(&suite);

    let selection = &summary["selection"];
    assert_eq!(
        selection["plan_id"], "exa-only",
        "the recorded captures cover different rubric items, and Exa's window covers most"
    );
    assert!(
        selection["method"]
            .as_str()
            .is_some_and(|method| method.contains("coverage") && method.contains("freshness")),
        "the method must name the weighted objective's measured terms: {}",
        selection["method"]
    );
    assert!(
        selection["explanation"]
            .as_str()
            .is_some_and(|explanation| explanation.contains("rubric eu-ai-act-cop/1")),
        "the explanation must name the rubric the ranking leaned on: {}",
        selection["explanation"]
    );
    assert_eq!(selection["unavailable_inputs"], Value::Array(vec![]));

    // The ranking demonstrates something only because the providers' windows
    // genuinely cover different items.
    let covered = |id: &str| -> Vec<String> {
        plan(&summary, id)["evaluation"]["coverage"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|item| item["covered"] == Value::Bool(true))
            .map(|item| item["item"].as_str().expect("a name").to_owned())
            .collect()
    };
    let (exa, firecrawl, tavily) = (
        covered("exa-only"),
        covered("firecrawl-only"),
        covered("tavily-only"),
    );
    assert!(exa.len() > firecrawl.len() && exa.len() > tavily.len());
    assert_ne!(
        firecrawl, tavily,
        "two providers covering identical item sets would demonstrate no ranking"
    );

    // No answer exists anywhere — no gateway was configured — yet the window
    // measurements stand and the winner still names them.
    for plan_record in summary["plans"].as_array().expect("plans") {
        assert!(plan_record["answer"].is_null());
    }
    let winner = plan(&summary, "exa-only");
    assert_eq!(winner["verification_state"], "replay-tested");
    assert_eq!(
        winner["evaluation"]["coverage"]["rubric"]["name"],
        "eu-ai-act-cop"
    );
    assert_eq!(winner["evaluation"]["freshness"]["fraction"], 1.0);
    assert_eq!(
        winner["evaluation"]["freshness"]["parts"][0]["date_provenance"]
            .as_str()
            .map(|provenance| provenance.contains("replay recording capture date")),
        Some(true),
        "every trusted date must name what declared it"
    );
}

/// The determinism half of the definition of done: two replay runs of the
/// rubric suite produce byte-identical coverage and freshness records —
/// both are pure functions of suite input and window content, so nothing in
/// them may vary with the run.
#[test]
fn two_replay_runs_produce_byte_identical_coverage_and_freshness_records() {
    let suite = rubric_suite();
    let first = replay_run(&suite);
    let second = replay_run(&suite);

    assert_eq!(
        first["run"]["manifest_hash"], second["run"]["manifest_hash"],
        "one suite must seal one manifest"
    );
    for plan_record in first["plans"].as_array().expect("plans") {
        let id = plan_record["id"].as_str().expect("an id");
        for section in ["coverage", "freshness"] {
            let first_bytes = serde_json::to_vec(&plan_record["evaluation"][section])
                .expect("a record serialises");
            let second_bytes = serde_json::to_vec(&plan(&second, id)["evaluation"][section])
                .expect("a record serialises");
            assert_eq!(
                first_bytes, second_bytes,
                "plan {id}'s {section} record differed between two replay runs of one suite"
            );
        }
    }
    // And the selection they feed is the same decision both times.
    assert_eq!(first["selection"], second["selection"]);
}

/// A rubric the captures could never satisfy still publishes measured zeros
/// for assembled windows — a measurement of the supply, not an absence —
/// while the selection then ranks nothing above anything and the smallest
/// plan id resolves the joint minimum deterministically.
#[test]
fn a_rubric_no_capture_satisfies_measures_zero_coverage_not_unknown() {
    let mut suite = rubric_suite();
    let rubric = suite.coverage_rubric.as_mut().expect("the suite's rubric");
    for item in &mut rubric.items {
        item.any_of = vec!["words no recorded capture has ever contained".to_owned()];
    }
    let summary = replay_run(&suite);
    for id in ["exa-only", "firecrawl-only", "tavily-only"] {
        let coverage = &plan(&summary, id)["evaluation"]["coverage"];
        assert_eq!(coverage["unmeasured"], Value::Null);
        assert_eq!(coverage["covered_count"], 0);
        assert_eq!(coverage["fraction"], 0.0);
    }
    assert!(
        summary["selection"]["plan_id"].is_string(),
        "equal measured scores are still an ordering; the objective is satisfied by any \
         joint maximum"
    );
}
