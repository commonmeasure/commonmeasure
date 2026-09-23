//! Local operator entry point for reproducible SimpleQA batches.

use clap::Args;
use commonmeasure_harness::{home_dir, policy::SessionPolicy};
use commonmeasure_runtime::{RunOptions, Suite, allowance::AllowanceContext};
use serde_json::json;
use std::path::PathBuf;

#[derive(Args)]
pub struct Benchmark {
    /// Normalised SimpleQA JSON from the pinned importer (or bundled smoke set).
    #[arg(long)]
    dataset: PathBuf,
    /// New directory for this attempt. Existing directories are refused.
    #[arg(long)]
    output: PathBuf,
    /// Search adapters to compare. The no-context baseline is also recorded.
    #[arg(long, value_delimiter = ',', default_value = "exa,tavily")]
    providers: Vec<String>,
    /// Explicitly select how many cases to run; defaults to a small smoke batch.
    #[arg(long, default_value_t = 5)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    seed: u64,
    #[arg(long, default_value_t = 5)]
    result_limit: u32,
    /// Answer model requested through the configured inference gateway.
    #[arg(long, default_value = "gpt-4.1")]
    model: String,
    /// Correctness grader requested through the same gateway.
    #[arg(long, default_value = "gpt-4.1")]
    judge_model: String,
    /// Enable supplier and model calls, including disclosure of questions,
    /// retrieved context, reference answers and predictions to the gateway.
    /// Without this flag, record unavailable results without external calls.
    #[arg(long)]
    live: bool,
}

pub fn run(args: Benchmark) -> Result<(), String> {
    let home = home_dir().map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let policy = SessionPolicy::load(&home, cwd.to_str())?;
    let canonical = policy.canonical();
    // Batch Suite supports shared constraints, but not all session screening
    // and terms settings. Refuse unsupported policy rather than dropping it.
    if !canonical["fail_closed"].is_null()
        || policy.refuse_on_pii()
        || !policy.internal_prefixes().is_empty()
        || canonical["terms"]
            .as_array()
            .is_some_and(|terms| !terms.is_empty())
    {
        return Err("This source policy cannot be represented by the benchmark batch path (unresolved principal policy, PII override, internal-source prefixes or terms overlays). No calls were made.".into());
    }
    let suite: Suite = serde_json::from_value(json!({
        "suite_version": "simpleqa/v1", "label": "SimpleQA",
        "job": {"id": "b883c63a-736f-4cf2-a4e5-7f856d2c3d64", "kind": "research.answer",
            "prompt": "Question replaced by the selected SimpleQA case", "policy_mode": policy.mode(),
            "objective": {"kind": "maximise_quality"}, "constraints": policy.constraints(), "evidence_requirements": []},
        "model_plan": {"name": "simpleqa-answer", "version": "1", "model": args.model},
        "result_limit": args.result_limit, "providers": args.providers,
        "require_cited_answer": false
    })).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&args.dataset).map_err(|e| e.to_string())?;
    let benchmark = commonmeasure_runtime::benchmark::Benchmark {
        dataset_bytes: &bytes,
        suite,
        judge_model: commonmeasure_types::ModelPlan {
            name: "simpleqa-judge".into(),
            version: "1".into(),
            model: args.judge_model,
        },
        limit: args.limit,
        seed: args.seed,
        policy_identity: json!({"identity": policy.identity(), "policy": canonical}),
    };
    if args.live
        && std::env::var("COMMONMEASURE_INFERENCE_ENDPOINT")
            .ok()
            .is_none_or(|s| s.trim().is_empty())
    {
        return Err(
            "A scored benchmark needs COMMONMEASURE_INFERENCE_ENDPOINT. No calls were made.".into(),
        );
    }
    if args.live {
        if RunOptions::from_environment(args.output.clone(), true)
            .backend
            .is_none()
        {
            return Err(
                "The inference gateway endpoint or credential configuration is unusable. No calls were made.".into(),
            );
        }
        commonmeasure_supply::credentials::apply(&home)?;
    }
    let report = commonmeasure_runtime::benchmark::execute(&benchmark, &args.output, |path| {
        let mut options = RunOptions::from_environment(path.to_owned(), args.live);
        options.allowance = Some(AllowanceContext::new(
            &home,
            policy.principal().into(),
            policy.allowances().to_vec(),
        ));
        if !args.live {
            options.backend = None;
        }
        options
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    println!(
        "Benchmark recorded in {}. Share summary.json or summary.csv; the other files are private source records.",
        args.output.display()
    );
    Ok(())
}
