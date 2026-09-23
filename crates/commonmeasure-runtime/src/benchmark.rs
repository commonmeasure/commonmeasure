//! Deterministic SimpleQA batches around the ordinary governed run path.
//! Each case retains its source record; the batch adds pinned inputs, grader
//! invocations and explicit denominators. Existing output is never resumed.

use std::{collections::HashSet, fs, io::Write, path::Path};

use commonmeasure_types::{
    ModelPlan,
    canonical::{canonical_digest, sha256_digest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{EvidenceLog, RunOptions, Suite, processor::correctness};

/// Upstream revision used by the dataset importer and bundled smoke cases.
pub const SOURCE_COMMIT: &str = "99feb7decc9be67edb49a63d3985cf6094c873f2";
/// Exact bundled upstream CSV bytes, before normalisation.
pub const SOURCE_SHA256: &str =
    "sha256:feee3f7e7db3617e94e8fcf1977b756ec420ef8568f4e0fcbbe0e92e9d5fc032";
/// Batch source-record format; these private records are never relay input.
pub const SCHEMA: &str = "contextops-simpleqa-benchmark/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// One-based row in the pinned upstream CSV, e.g. simpleqa-0001.
    pub id: String,
    pub question: String,
    pub reference_answer: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub schema: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub source_rows: usize,
    pub cases: Vec<Case>,
}

impl Dataset {
    /// Validate input before creating evidence or dispatching any calls.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != "contextops-simpleqa-dataset/v1"
            || self.source_commit != SOURCE_COMMIT
            || self.source_sha256 != SOURCE_SHA256
            || self.source_rows != 4326
            || self.cases.is_empty()
        {
            return Err("Expected a nonempty dataset from the pinned SimpleQA importer.".into());
        }
        let mut seen = HashSet::new();
        for case in &self.cases {
            let row = case
                .id
                .strip_prefix("simpleqa-")
                .and_then(|s| s.parse::<usize>().ok());
            if !row
                .is_some_and(|n| (1..=4326).contains(&n) && case.id == format!("simpleqa-{n:04}"))
                || !seen.insert(&case.id)
                || case.question.trim().is_empty()
                || case.question.len() > 4096
                || case.reference_answer.trim().is_empty()
                || case.reference_answer.len() > 16384
            {
                return Err("Dataset has an invalid or duplicate case.".into());
            }
        }
        Ok(())
    }

    /// Stable hash-ranked selection, independent of input order and Rust RNG versions.
    pub fn sample(&self, limit: usize, seed: u64) -> Result<Vec<&Case>, String> {
        self.validate()?;
        if limit == 0 || limit > self.cases.len() {
            return Err(format!("Choose between 1 and {} cases.", self.cases.len()));
        }
        let mut cases: Vec<_> = self.cases.iter().collect();
        cases.sort_by_key(|case| {
            (
                sha256_digest(format!("simpleqa-sample/v1\n{seed}\n{}", case.id).as_bytes()),
                &case.id,
            )
        });
        cases.truncate(limit);
        Ok(cases)
    }
}

/// Fixed controls supplied by the CLI. The caller resolves principal policy
/// into the suite and allowance before entering this module.
pub struct Benchmark<'a> {
    pub dataset_bytes: &'a [u8],
    pub suite: Suite,
    pub judge_model: ModelPlan,
    pub limit: usize,
    pub seed: u64,
    pub policy_identity: Value,
}

/// Execute a batch sequentially. A started request is never retried; an existing
/// output directory is refused, including one left by an interrupted attempt.
/// The options factory preserves the ordinary runtime's adapter/gateway seam.
pub fn execute<F>(
    benchmark: &Benchmark<'_>,
    output: &Path,
    mut options_for: F,
) -> Result<Value, String>
where
    F: FnMut(&Path) -> RunOptions,
{
    let dataset: Dataset =
        serde_json::from_slice(benchmark.dataset_bytes).map_err(|e| e.to_string())?;
    let cases = dataset.sample(benchmark.limit, benchmark.seed)?;
    benchmark
        .suite
        .job
        .validate()
        .map_err(|e| format!("Invalid benchmark job: {e:?}"))?;
    benchmark
        .suite
        .model_plan
        .validate()
        .map_err(|e| format!("Invalid answer model: {e:?}"))?;
    benchmark
        .judge_model
        .validate()
        .map_err(|e| format!("Invalid judge model: {e:?}"))?;
    let mut seen = HashSet::new();
    if benchmark.suite.providers.is_empty()
        || benchmark.suite.providers.iter().any(|provider| {
            provider == "internal"
                || !commonmeasure_supply::IMPLEMENTED_PROVIDERS.contains(&provider.as_str())
                || !seen.insert(provider)
        })
        || benchmark.suite.result_limit == 0
        || benchmark.suite.result_limit > 100
        || benchmark.suite.fetch_target.is_some()
        || benchmark.suite.coverage_rubric.is_some()
        || benchmark.suite.as_of.is_some()
        || benchmark.suite.governance.is_some()
        || benchmark.suite.output_provenance.is_some()
        || benchmark.suite.fidelity_judge
    {
        return Err("SimpleQA requires distinct public search providers, a result limit of 1–100 and no unrelated suite processors.".into());
    }
    fs::create_dir(output).map_err(|e| {
        format!(
            "Cannot create new benchmark output {}: {e}. Existing attempts are never resumed.",
            output.display()
        )
    })?;
    let batch_id = Uuid::new_v4();
    let manifest = json!({"schema": SCHEMA, "batch_id": batch_id,
        "dataset_sha256": sha256_digest(benchmark.dataset_bytes), "dataset_source_commit": dataset.source_commit,
        "dataset_source_sha256": dataset.source_sha256, "available_cases": dataset.cases.len(),
        "source_rows": dataset.source_rows, "case_ids": cases.iter().map(|c| &c.id).collect::<Vec<_>>(),
        "sample": {"algorithm": "sha256-rank/v1", "seed": benchmark.seed},
        "suite_template": benchmark.suite, "judge_model": benchmark.judge_model,
        "judge": correctness::manifest(), "inference_request_shape": commonmeasure_inference::request_shape_digest(),
        "policy_identity": benchmark.policy_identity,
        "comparison": "Common Measure evaluation on SimpleQA; not a reproduction of Tavily scores",
        "sharing": "local_only", "automatic_retries": false});
    write_json(&output.join("manifest.json"), &manifest)?;
    write_new(&output.join("dataset.json"), benchmark.dataset_bytes)?;
    let manifest_hash = canonical_digest(&manifest);
    let mut log =
        EvidenceLog::create(&output.join("evidence.ndjson")).map_err(|e| e.to_string())?;
    append(
        &mut log,
        "benchmark_started",
        json!({"batch_id": batch_id, "manifest_hash": manifest_hash}),
    )?;
    let mut rows = Vec::new();
    for case in &cases {
        let mut suite = benchmark.suite.clone();
        suite.job.id = Uuid::new_v4();
        suite.job.prompt.clone_from(&case.question);
        suite.label = format!("SimpleQA {}", case.id);
        let run_path = output.join(&case.id);
        let options = options_for(&run_path);
        append(
            &mut log,
            "case_started",
            json!({"case_id": case.id, "job_id": suite.job.id,
            "external_acquisition": options.allow_external_acquisition, "inference_configured": options.backend.is_some()}),
        )?;
        match crate::execute(&suite, &options) {
            Ok(report) => {
                if report.summary["run"]["evidence_complete"] != true {
                    append(
                        &mut log,
                        "case_evidence_incomplete",
                        json!({"case_id": case.id, "run_id": report.run_id}),
                    )?;
                    return Err(format!(
                        "Case {} has incomplete source evidence. Subsequent cases and grading were stopped; requests may have incurred charges.",
                        case.id
                    ));
                }
                for plan in report.summary["plans"]
                    .as_array()
                    .ok_or("Run has no plans")?
                {
                    let mut row = json!({"case_id": case.id, "provider": plan["provider"], "plan_id": plan["id"],
                        "run_id": report.run_id, "manifest_hash": report.manifest_hash,
                        "run_directory": case.id, "status": plan["status"], "grade": null,
                        "source_record_complete": report.summary["run"]["evidence_complete"]});
                    if report.summary["run"]["evidence_complete"] == true
                        && plan["status"] == "completed"
                        && let Some(answer) = plan["answer"].as_str()
                    {
                        append(
                            &mut log,
                            "judge_started",
                            json!({"case_id": case.id, "provider": plan["provider"], "run_id": report.run_id}),
                        )?;
                        let invocation = correctness::invoke(
                            options.backend.as_deref(),
                            report.run_id,
                            &benchmark.judge_model,
                            &case.question,
                            &case.reference_answer,
                            answer,
                        )
                        .to_value();
                        row["grade"] = json!(correctness::grade(&invocation));
                        row["judge"] = invocation;
                    }
                    append(&mut log, "case_provider_finished", row.clone())?;
                    rows.push(row);
                }
            }
            Err(crate::RunError::Io(error)) => {
                return Err(format!(
                    "Case {} stopped after an evidence IO failure: {error}. Requests may have incurred charges; no automatic retry.",
                    case.id
                ));
            }
            Err(error) => {
                for provider in
                    std::iter::once("none").chain(suite.providers.iter().map(String::as_str))
                {
                    let row = json!({"case_id": case.id, "provider": provider, "status": "error", "grade": null, "error": error.to_string()});
                    append(&mut log, "case_provider_finished", row.clone())?;
                    rows.push(row);
                }
            }
        }
        append(&mut log, "case_finished", json!({"case_id": case.id}))?;
    }
    let summary = summary(&benchmark.suite.providers, cases.len(), &rows);
    write_json(&output.join("results.private.json"), &json!(rows))?;
    // Sharing includes only fixed provider names and numerical measures: no
    // source content, answers, reference answers, identity or diagnostics.
    let shared = json!({"schema": SCHEMA, "batch_id": batch_id, "manifest_hash": manifest_hash,
        "dataset_sha256": sha256_digest(benchmark.dataset_bytes), "source_commit": SOURCE_COMMIT,
        "selected_cases": cases.len(), "available_cases": dataset.cases.len(), "seed": benchmark.seed,
        "answer_model_requested": benchmark.suite.model_plan.model,
        "judge_model_requested": benchmark.judge_model.model,
        "result_limit_requested": benchmark.suite.result_limit,
        "judge_configuration_digest": correctness::manifest().configuration_digest,
        "providers": summary, "acquisition_cost": null, "answer_cost": null, "judge_cost": null,
        "cost_note": "Unknown aggregate costs. Per-call acquisition charges and inference usage remain in private source records.",
        "comparison": "Common Measure evaluation on SimpleQA; not a reproduction of Tavily scores"});
    write_json(&output.join("summary.json"), &shared)?;
    write_new(&output.join("summary.csv"), csv(&shared).as_bytes())?;
    append(
        &mut log,
        "benchmark_finished",
        json!({"summary_sha256": sha256_digest(&fs::read(output.join("summary.json")).map_err(|e| e.to_string())?)}),
    )?;
    Ok(shared)
}

fn summary(providers: &[String], selected: usize, rows: &[Value]) -> Vec<Value> {
    std::iter::once("none").chain(providers.iter().map(String::as_str)).map(|provider| {
        let rows: Vec<_> = rows.iter().filter(|r| r["provider"] == provider).collect();
        let count_grade = |grade| rows.iter().filter(|r| r["grade"] == grade).count();
        let correct = count_grade("correct");
        let incorrect = count_grade("incorrect");
        let abstained = count_grade("not_attempted");
        let measured = correct + incorrect + abstained;
        json!({"provider": provider, "selected": selected, "recorded": rows.len(),
            "completed": rows.iter().filter(|r| r["status"] == "completed").count(),
            "refused": rows.iter().filter(|r| r["status"] == "refused").count(),
            "errors": rows.iter().filter(|r| r["status"] == "error").count(),
            "unavailable": rows.iter().filter(|r| r["status"] == "unavailable").count(),
            "correct": correct, "incorrect": incorrect, "not_attempted": abstained,
            "measured": measured, "unmeasured": selected - measured,
            "accuracy_measured": if measured == 0 {None} else {Some(correct as f64 / measured as f64)},
            "accuracy_all_selected": if measured == selected {Some(correct as f64 / selected as f64)} else {None}})
    }).collect()
}

fn csv(summary: &Value) -> String {
    let columns = [
        "provider",
        "selected",
        "recorded",
        "completed",
        "refused",
        "errors",
        "unavailable",
        "correct",
        "incorrect",
        "not_attempted",
        "measured",
        "unmeasured",
        "accuracy_measured",
        "accuracy_all_selected",
    ];
    let mut text = format!("{}\r\n", columns.join(","));
    for row in summary["providers"].as_array().expect("summary providers") {
        text.push_str(
            &columns
                .iter()
                .map(|key| match &row[key] {
                    Value::Null => String::new(),
                    Value::String(s) => s.clone(),
                    value => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        text.push_str("\r\n");
    }
    text
}

fn append(log: &mut EvidenceLog, event: &str, value: Value) -> Result<(), String> {
    log.append(event, value)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    write_new(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::File::create_new(path).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SMOKE: &[u8] = include_bytes!("../../../demo/benchmarks/simpleqa/smoke.json");

    #[test]
    fn sampling_is_stable_and_rejects_duplicate_rows() {
        let mut data: Dataset = serde_json::from_slice(SMOKE).unwrap();
        let selected = data
            .sample(3, 42)
            .unwrap()
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>();
        data.cases.reverse();
        assert_eq!(
            selected,
            data.sample(3, 42)
                .unwrap()
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>()
        );
        assert!(data.sample(6, 42).is_err());
        data.cases.push(data.cases[0].clone());
        assert!(data.validate().is_err());
    }

    #[test]
    fn failures_do_not_shrink_the_denominator_or_become_abstentions() {
        let rows = vec![
            json!({"provider":"exa","status":"completed","grade":"correct"}),
            json!({"provider":"exa","status":"refused","grade":null}),
            json!({"provider":"exa","status":"completed","grade":"not_attempted"}),
        ];
        let result = summary(&["exa".into()], 3, &rows);
        assert_eq!(result[1]["measured"], 2);
        assert_eq!(result[1]["unmeasured"], 1);
        assert_eq!(result[1]["accuracy_measured"], 0.5);
        assert!(result[1]["accuracy_all_selected"].is_null());
        assert_eq!(result[1]["not_attempted"], 1);
        assert!(result[0]["accuracy_measured"].is_null());
    }
}
