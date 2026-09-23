//! Skill supply through the whole run path: planning, the execution gate,
//! admission, the sealed result and the invocation record.
//!
//! The supply is a real bundle on disk and a real catalogue naming it; the
//! adapter, the containment check, the spawn, the policy, the evaluators, the
//! evidence log and the published directory are the production ones. What
//! this file is for is the half `crates/commonmeasure-supply/tests/skill_invocation.rs`
//! cannot show: that a produced result is ranked, admitted and recorded by
//! the same machinery as a retrieved one, and that the differences between
//! them stay visible instead of being smoothed away.

use std::path::Path;

use commonmeasure_runtime::{CoverageRubric, RubricItem, RunOptions, Suite, execute};
use commonmeasure_supply::{LocalSkillAdapter, SkillCatalogue};
use commonmeasure_types::{Constraint, ContextJob, Objective, PolicyMode};
use serde_json::Value;
use uuid::Uuid;

/// A bundle whose entrypoint prints a verdict on whatever path it is given,
/// and — where `marker` is set — records that it ran at all, so a test can
/// prove a refusal happened *before* the spawn rather than after it.
fn bundle(root: &Path, marker: Option<&Path>) {
    std::fs::create_dir_all(root.join("scripts")).expect("mkdir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: publishable-check\nlicense: Apache-2.0\n---\n\n# Publishable check\n",
    )
    .expect("write SKILL.md");
    let trace = marker
        .map(|path| format!("touch {}\n", path.display()))
        .unwrap_or_default();
    std::fs::write(
        root.join("scripts/check.sh"),
        format!(
            "#!/bin/sh\n{trace}printf 'frontmatter carries an unexpected key: version\\n'\n\
             printf 'the bundle is not publishable as an Agent Skill until it is removed\\n'\n"
        ),
    )
    .expect("write entrypoint");
}

fn catalogue(directory: &Path, root: &Path) -> std::path::PathBuf {
    let path = directory.join("skills.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"skills": {{"publishable-check": {{
                 "root": {:?},
                 "entrypoint": "scripts/check.sh",
                 "interpreter": "/bin/sh",
                 "arguments": ["{{job}}"],
                 "timeout_ms": 10000,
                 "maximum_output_bytes": 65536}}}}}}"#,
            root.display()
        ),
    )
    .expect("write catalogue");
    path
}

fn suite(providers: Vec<String>, objective: Objective, constraints: Vec<Constraint>) -> Suite {
    Suite {
        suite_version: "skill-test/v1".into(),
        label: "Skill supply through the run path".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "review.verdict".into(),
            // The job's own value is the target path, substituted into the
            // operator's declared argument vector.
            prompt: "bundles/target".into(),
            policy_mode: PolicyMode::Strict,
            objective,
            constraints,
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 4,
        providers,
        require_cited_answer: false,
        coverage_rubric: Some(CoverageRubric {
            name: "publishable".into(),
            version: "1".into(),
            items: vec![
                RubricItem {
                    name: "names-the-fault".into(),
                    any_of: vec!["unexpected key".into()],
                },
                RubricItem {
                    name: "names-the-remedy".into(),
                    any_of: vec!["until it is removed".into()],
                },
                RubricItem {
                    name: "names-the-packaging-contract".into(),
                    any_of: vec!["plugin.json".into()],
                },
            ],
        }),
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

struct Run {
    _directory: tempfile::TempDir,
    output: std::path::PathBuf,
    summary: Value,
}

impl Run {
    fn execute(suite: &Suite, catalogue_path: &Path, live: bool) -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let output = directory.path().join("published");
        let catalogue_path = catalogue_path.to_path_buf();
        let options = RunOptions {
            allow_external_acquisition: live,
            output: output.clone(),
            // A real adapter over a real catalogue and a real bundle. What a
            // test supplies here is the *supply*, exactly as a loopback
            // origin supplies recorded provider bytes; the adapter, the
            // containment check and the spawn are the production ones.
            suppliers: Box::new(move |provider| {
                let name = provider.trim_start_matches("skill:");
                let catalogue = SkillCatalogue::load(&catalogue_path)?;
                let declaration = catalogue.skills.get(name).expect("declared").clone();
                Ok(
                    Box::new(LocalSkillAdapter::new(&catalogue_path, name, declaration))
                        as Box<dyn commonmeasure_supply::SupplyAdapter>,
                )
            }),
            // No gateway: this file is about supply, and a plan whose
            // inference never ran is still ranked on what it produced and
            // admitted.
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        let report = execute(suite, &options).expect("the run publishes");
        Self {
            _directory: directory,
            output,
            summary: report.summary,
        }
    }

    fn plan(&self, id: &str) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == id)
            .unwrap_or_else(|| panic!("plan {id} is missing"))
    }
}

#[test]
fn an_invoked_skill_is_planned_admitted_and_recorded_like_any_other_supply() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("publishable-check");
    bundle(&root, None);
    let path = catalogue(directory.path(), &root);
    let suite = suite(
        vec!["skill:publishable-check".into()],
        Objective::MinimiseLatency,
        vec![],
    );

    let run = Run::execute(&suite, &path, true);
    assert_eq!(run.summary["schema_version"], "contextops-run/v7");
    let plan = run.plan("skill:publishable-check-only");
    assert_eq!(plan["capability"], "invoke");
    // The execution happened and its result is sealed, which is what the
    // verification state claims — the same standard a local corpus read meets.
    assert_eq!(plan["verification_state"], "live-verified");
    assert_eq!(plan["source_count"], 1);

    // No HTTP, no price. An exit code is not a status code and an
    // undisclosed charge is unknown, never zero.
    let acquisition = &plan["acquisition"];
    assert_eq!(acquisition["http_status"], Value::Null);
    assert_eq!(acquisition["charge"], serde_json::json!({}));
    assert_eq!(acquisition["result_count"], 1);

    // The invocation record: what ran, under whose declaration, and what it
    // exited with.
    let invocation = &acquisition["invocation"];
    assert_eq!(invocation["declared_name"], "publishable-check");
    assert_eq!(invocation["catalogue_name"], "publishable-check");
    // The format declares no version, so the record carries the absence
    // rather than a guess.
    assert_eq!(invocation["declared_version"], Value::Null);
    assert_eq!(invocation["exit_code"], 0);
    assert_eq!(invocation["termination"], "exited");
    assert!(
        invocation["skill_md_sha256"]
            .as_str()
            .expect("a SKILL.md digest")
            .starts_with("sha256:")
    );
    assert!(
        invocation["entrypoint_sha256"]
            .as_str()
            .expect("an entrypoint digest")
            .starts_with("sha256:")
    );
    // Every element of the executed command line, with the provenance that
    // says which parts the job supplied and which the operator declared.
    assert_eq!(invocation["argv"][0], "/bin/sh");
    assert_eq!(invocation["argv"][2], "bundles/target");
    assert_eq!(
        invocation["argv_source"],
        serde_json::json!(["operator", "operator", "job"])
    );
    assert!(
        invocation["environment"]
            .as_str()
            .expect("an environment policy")
            .starts_with("cleared")
    );
    // The step's limit is recorded and stated not to have governed: one
    // invocation, one result.
    assert_eq!(invocation["requested_limit"], 4);
    assert_eq!(invocation["limit_applied"], false);

    // The sealed bytes are on disk and hash to what the plan claims, so the
    // envelope can be re-derived from them.
    let sealed = std::fs::read(
        run.output.join(
            acquisition["response_ref"]
                .as_str()
                .expect("a sealed response"),
        ),
    )
    .expect("the sealed response is published");
    assert_eq!(
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&sealed)
        ),
        acquisition["response_hash"].as_str().expect("a hash")
    );

    // The produced result entered the window, and it is measured against the
    // suite's rubric like any other admitted text. Two of three items are
    // covered: this skill names the fault and the remedy, and says nothing
    // about packaging — an honest partial answer, not a passing grade.
    assert_eq!(plan["evaluation"]["coverage"]["covered_count"], 2);
    assert_eq!(plan["evaluation"]["coverage"]["item_count"], 3);

    // Nothing published a produced result, so nothing dated it.
    assert_eq!(
        plan["evaluation"]["freshness"]["fraction"],
        Value::Null,
        "a produced result has no declared date to measure"
    );
    let source = &plan["sources"][0];
    assert_eq!(source["licence"]["state"], "unknown");
    assert_eq!(source["host"], "");
    assert!(
        source["url"]
            .as_str()
            .expect("a url")
            .starts_with("file://"),
        "a produced result's source is the bundle that produced it"
    );
}

#[test]
fn a_run_that_was_not_authorised_to_execute_spawns_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("publishable-check");
    let marker = directory.path().join("executed.marker");
    bundle(&root, Some(&marker));
    let path = catalogue(directory.path(), &root);
    let suite = suite(
        vec!["skill:publishable-check".into()],
        Objective::MinimiseLatency,
        vec![],
    );

    // The --live gate is about money leaving the operator *or code
    // executing*: without it, an invocation is as unauthorised as a billable
    // call.
    let run = Run::execute(&suite, &path, false);
    let plan = run.plan("skill:publishable-check-only");
    assert_eq!(plan["status"], "unavailable");
    assert_eq!(plan["acquisition"], Value::Null);
    let gap = plan["gaps"][0]["detail"].as_str().expect("a gap");
    assert!(gap.contains("no process was spawned"), "{gap}");
    assert!(
        !marker.exists(),
        "an unauthorised run must not execute the skill it declined to invoke"
    );
}

#[test]
fn a_denied_skill_provider_is_refused_before_anything_is_spawned() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("publishable-check");
    let marker = directory.path().join("executed.marker");
    bundle(&root, Some(&marker));
    let path = catalogue(directory.path(), &root);
    let suite = suite(
        vec!["skill:publishable-check".into()],
        Objective::MinimiseLatency,
        vec![Constraint::DeniedProvider {
            provider: "skill:publishable-check".into(),
        }],
    );

    // Job policy is the second independent layer: the operator's catalogue
    // admits the skill to the machine, and the job still decides whether this
    // job may reach it. A skill is an ordinary provider to that policy.
    let run = Run::execute(&suite, &path, true);
    let plan = run.plan("skill:publishable-check-only");
    assert_eq!(plan["status"], "refused");
    assert_eq!(plan["eligibility"]["eligible"], false);
    assert!(
        !marker.exists(),
        "a denied provider must never reach the spawn"
    );
}

#[test]
fn a_freshness_weighted_objective_abstains_over_a_skill_plan_and_names_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("publishable-check");
    bundle(&root, None);
    let path = catalogue(directory.path(), &root);
    let mut suite = suite(
        vec!["skill:publishable-check".into()],
        Objective::Weighted {
            cost: 0.0,
            latency: 0.0,
            coverage: 1.0,
            freshness: 1.0,
            quality: 0.0,
            policy_risk: 0.0,
        },
        vec![],
    );
    suite.as_of = Some(
        serde_json::from_value(serde_json::json!({
            "date": "2026-08-06",
            "maximum_age_days": 90
        }))
        .expect("an as_of"),
    );

    let run = Run::execute(&suite, &path, true);
    // The capability set differing is visible in the artefact rather than
    // papered over: nothing published a produced result, so no date attaches
    // to it, and an objective that weights freshness cannot rank it. The
    // router abstains naming the plan and the measurement instead of scoring
    // the undated plan as stale or as fresh.
    assert_eq!(run.summary["selection"]["plan_id"], Value::Null);
    // `unavailable_inputs` names measurements; the explanation names the plan
    // that withheld one, so a reader sees both which measure was missing and
    // which route could not supply it.
    assert_eq!(
        run.summary["selection"]["unavailable_inputs"],
        serde_json::json!(["freshness"])
    );
    let explanation = run.summary["selection"]["explanation"]
        .as_str()
        .expect("an explanation");
    assert!(
        explanation.contains("skill:publishable-check-only"),
        "{explanation}"
    );
}
