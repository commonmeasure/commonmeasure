//! Fidelity verification, exercised end to end: recorded supply through the
//! real adapters and policy, real inference transport against a loopback
//! gateway, the deterministic verifier over the answer that came back, and
//! the optional model judge measured against it.
//!
//! The gateway here is scripted, the one substitution these tests make: a
//! local process stands in for the uncontrolled network, never for this
//! runtime's own path. The scripted answer model reads the SUPPLIED CONTEXT
//! it is genuinely sent and reproduces spans from the middle of it, so every
//! span the verifier records is checked against real window content; the
//! scripted judge reads the numbered claims it is genuinely sent.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::processor::judge;
use commonmeasure_runtime::{ReplaySupply, Suite, execute};
use commonmeasure_types::{Constraint, ContextJob, Money, Objective, PolicyMode};
use serde_json::Value;
use uuid::Uuid;

/// The maintainer's recorded fixtures (`COMMONMEASURE_PRIVATE_EVIDENCE`,
/// resolved by `build.rs`).
fn evidence() -> PathBuf {
    PathBuf::from(env!("COMMONMEASURE_EVIDENCE_DIR"))
}

/// The first SOURCE block of the window as the model received it: URL and
/// exact text. `None` for the baseline, whose window is `(none)`.
fn first_block(user_message: &str) -> Option<(String, String)> {
    let (_, context) = user_message.split_once("SUPPLIED CONTEXT\n")?;
    let block = context.strip_prefix("SOURCE ")?;
    let (header, rest) = block.split_once('\n')?;
    let url = header.split(" [").next()?.trim().to_owned();
    let text = rest.split("\n\nSOURCE ").next()?.to_owned();
    Some((url, text))
}

/// The window part of a request, so two requests can be compared for having
/// received the same context.
fn window_of(user_message: &str) -> String {
    user_message
        .split_once("SUPPLIED CONTEXT\n")
        .map(|(_, window)| window.to_owned())
        .unwrap_or_default()
}

/// `length` consecutive words of `text` starting at or after word
/// `at_least`, none of which ends a sentence, so the run sits inside one
/// claim when the answer reproduces it.
fn mid_run(text: &str, at_least: usize, length: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    (at_least..words.len().saturating_sub(length))
        .map(|start| &words[start..start + length])
        .find(|run| {
            run.iter()
                .all(|word| !word.ends_with(['.', '!', '?', ':']) && !word.contains("CITATION"))
        })
        .map(|run| run.join(" "))
        .expect("the window text holds a run of words that ends no sentence")
}

/// What the scripted judge replies with, given the number of claims.
type JudgeScript = Arc<dyn Fn(usize) -> Response + Send + Sync>;

struct Gateway {
    handle: ServerHandle,
    /// The window of every request, in order of arrival.
    windows: Arc<Mutex<Vec<(String, String)>>>,
}

/// A loopback gateway serving two models: the answer model, which quotes
/// mid-text runs from the first block it received, and the judge, scripted
/// per test.
fn gateway(judge: JudgeScript) -> Gateway {
    let windows = Arc::new(Mutex::new(Vec::new()));
    let seen = windows.clone();
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
            let system = body["messages"][0]["content"].as_str().unwrap_or_default();
            let user = body["messages"][1]["content"].as_str().unwrap_or_default();
            if system == judge::INSTRUCTION {
                seen.lock()
                    .unwrap()
                    .push(("judge".to_owned(), window_of(user)));
                let claims = user
                    .split("\n\nSUPPLIED CONTEXT")
                    .next()
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.starts_with("CLAIM "))
                    .count();
                return judge(claims);
            }
            seen.lock()
                .unwrap()
                .push(("answer".to_owned(), window_of(user)));
            let Some((url, text)) = first_block(user) else {
                return completion("The supplied context cannot establish this.");
            };
            // The recorded texts are short (the recon captures elide the
            // page body), so the runs start a few words in.
            let cited = mid_run(&text, 2, 6);
            let reproduced = mid_run(&text, 10, 7);
            let content = format!(
                "The source states {cited}.\nIt also says that {reproduced}.\nNothing in the \
                 sources mentions the price of tea in Peru.\nCITATION: {url} :: \"{cited}\""
            );
            completion(&content)
        })
        .expect("spawn");
    Gateway { handle, windows }
}

fn completion(content: &str) -> Response {
    Response::json(
        200,
        &serde_json::json!({
            "id": "tz-fidelity",
            "model": "local-test-model",
            "choices": [{"message": {"content": content}}],
            "usage": {"prompt_tokens": 40, "completion_tokens": 9}
        })
        .to_string(),
    )
}

fn suite(fidelity_judge: bool) -> Suite {
    Suite {
        suite_version: "fidelity-test/v1".into(),
        label: "Fidelity verification path".into(),
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
        providers: vec!["exa".into()],
        require_cited_answer: true,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        output_provenance: None,
        fidelity_judge,
    }
}

/// The summary, the sealed manifest and the evidence log of one run.
fn run(suite: &Suite, gateway: &Gateway) -> (Value, Value, Vec<Value>) {
    let supply = ReplaySupply::start(&evidence().join("recon"), &suite.providers)
        .expect("the committed manifest covers every suite provider");
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("run");
    let mut options = supply.run_options(output.clone());
    options.backend = Some(Box::new(
        TensorZeroBackend::new(gateway.handle.url()).expect("a loopback gateway endpoint"),
    ));
    let report = execute(suite, &options).expect("the replay run should complete");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
    let log = std::fs::read_to_string(output.join("evidence.ndjson")).unwrap();
    let records = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (report.summary, manifest["manifest"].clone(), records)
}

fn exa_plan(summary: &Value) -> &Value {
    let plan = summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == "exa-only")
        .expect("the exa plan");
    assert_eq!(plan["status"], "completed", "inference must have run");
    plan
}

fn invocation<'a>(plan: &'a Value, name: &str) -> Option<&'a Value> {
    plan["processors"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|invocation| invocation["processor"]["name"] == name)
}

/// The verifier records a claim supported by its citation, a claim supported
/// by a run of words from the middle of a window part with no citation at
/// all, and an invented claim as unsupported; the judge, disagreeing on the
/// invented claim, is measured against those verdicts and never replaces
/// them.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn mid_text_reproductions_are_supported_and_the_judge_is_measured_against_the_verifier() {
    let gateway = gateway(Arc::new(|claims| {
        assert_eq!(
            claims, 3,
            "the judge is sent every claim the verifier segmented"
        );
        completion(
            "CLAIM 1: supported :: \"stated verbatim\"\nCLAIM 2: supported :: \"stated \
             verbatim\"\nCLAIM 3: supported :: \"the judge claims a paraphrase\"",
        )
    }));
    let (summary, manifest, records) = run(&suite(true), &gateway);
    let plan = exa_plan(&summary);

    let verifier = invocation(plan, "fidelity-verifier").expect("the verifier ran");
    assert_eq!(verifier["stage"], "verify");
    assert_eq!(verifier["assurance"], "observed");
    let detail = &verifier["detail"];
    assert_eq!(detail["claim_count"], 3, "{detail}");
    assert_eq!(detail["claim_counts"]["supported"], 2);
    assert_eq!(detail["claim_counts"]["unsupported"], 1);
    let claims = detail["claims"].as_array().unwrap();
    assert_eq!(claims[0]["basis"], "citation");
    assert_eq!(claims[0]["citation"], 0);
    assert_eq!(claims[1]["basis"], "window");
    let evidence = &claims[1]["evidence"];
    assert!(
        evidence["start"].as_u64().unwrap() > 0,
        "the reproduced run sits mid-text: {evidence}"
    );
    assert!(evidence["words"].as_u64().unwrap() >= 7);
    // The span is pinned to a part the grounding record's own inputs list.
    let inputs = plan["evaluation"]["grounding"]["inputs"]
        .as_array()
        .unwrap();
    assert!(inputs.iter().any(|input| {
        input["content_hash"] == evidence["content_hash"]
            && input["reference"] == evidence["reference"]
    }));
    // The claim's own bytes excise to the recorded text.
    let answer = plan["answer"].as_str().unwrap();
    for claim in claims {
        let (start, end) = (
            claim["start"].as_u64().unwrap() as usize,
            claim["end"].as_u64().unwrap() as usize,
        );
        assert_eq!(&answer[start..end], claim["text"].as_str().unwrap());
    }
    assert_eq!(claims[2]["verdict"], "unsupported");

    let judge = invocation(plan, "fidelity-judge").expect("the judge ran");
    assert_eq!(judge["assurance"], "declared");
    assert_eq!(judge["decision"], "abstain");
    let detail = &judge["detail"];
    assert_eq!(detail["judged"], true);
    assert_eq!(detail["judge"]["executed_model"], "local-test-model");
    assert_eq!(detail["agreement"]["compared"], 3);
    assert_eq!(detail["agreement"]["agreed"], 2);
    assert_eq!(detail["agreement"]["matrix"]["unsupported"]["supported"], 1);
    assert_eq!(detail["verdicts"][2]["verifier"], "unsupported");
    assert_eq!(detail["verdicts"][2]["judge"], "supported");
    assert_eq!(detail["verdicts"][2]["agrees"], false);
    assert_eq!(detail["cost"]["money"], Value::Null);

    // The judge received exactly the window the answer model received.
    let windows = gateway.windows.lock().unwrap();
    // The baseline's window is `(none)`; the exa plan's is the one to match.
    let answer_window = windows
        .iter()
        .find(|(kind, window)| kind == "answer" && window != "(none)")
        .map(|(_, window)| window.clone())
        .unwrap();
    let judge_window = windows
        .iter()
        .find(|(kind, _)| kind == "judge")
        .map(|(_, window)| window.clone())
        .unwrap();
    assert_eq!(answer_window, judge_window);

    // Sealed: a suite that declares the judge is a different experiment.
    assert_eq!(manifest["fidelity_judge"], true);
    assert!(
        manifest["processors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "fidelity-judge")
    );
    // Both invocations are in the append-only trail as well as the plan.
    let invoked: Vec<&str> = records
        .iter()
        .filter(|record| record["event"] == "processor_invoked")
        .filter_map(|record| record["payload"]["processor"]["name"].as_str())
        .collect();
    assert!(invoked.contains(&"fidelity-verifier") && invoked.contains(&"fidelity-judge"));
}

/// Without the opt-in the verifier still runs on every answer and no judge
/// exchange happens; the manifest carries no `fidelity_judge` key, so a
/// suite that never declared one keeps its shape.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_undeclared_judge_records_nothing_and_the_verifier_still_runs() {
    let gateway = gateway(Arc::new(|_| panic!("no judge exchange may happen")));
    let (summary, manifest, _) = run(&suite(false), &gateway);
    let plan = exa_plan(&summary);
    assert!(invocation(plan, "fidelity-verifier").is_some());
    assert!(invocation(plan, "fidelity-judge").is_none());
    assert!(manifest.get("fidelity_judge").is_none());
    // The baseline's answer is verified against its empty window: every
    // claim unsupported, which is the fact about it.
    let baseline = summary["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["id"] == "no-context")
        .unwrap();
    let verifier = invocation(baseline, "fidelity-verifier").expect("the baseline is verified");
    assert_eq!(verifier["detail"]["claim_counts"]["supported"], 0);
}

/// A judge reply the parser cannot read leaves every claim unavailable and
/// the agreement figure null, never a number manufactured from nothing.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_unreadable_judge_reply_leaves_every_claim_unavailable() {
    let gateway = gateway(Arc::new(|_| {
        completion("I think the first claim is fine and the rest are questionable.")
    }));
    let (summary, _, _) = run(&suite(true), &gateway);
    let judge = invocation(exa_plan(&summary), "fidelity-judge").expect("the judge ran");
    let detail = &judge["detail"];
    assert_eq!(detail["judged"], true);
    assert_eq!(detail["agreement"]["compared"], 0);
    assert_eq!(detail["agreement"]["fraction"], Value::Null);
    for verdict in detail["verdicts"].as_array().unwrap() {
        assert_eq!(verdict["judge"], "unavailable");
        assert_eq!(verdict["agrees"], Value::Null);
    }
}

/// A judge exchange the gateway refuses is recorded as not judged, with the
/// gap naming the failure; the verifier's record stands beside it.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_failed_judge_exchange_records_the_gap_and_the_verifier_stands() {
    let gateway = gateway(Arc::new(|_| {
        Response::json(500, r#"{"error":"the judge model is not loaded"}"#)
    }));
    let (summary, _, _) = run(&suite(true), &gateway);
    let plan = exa_plan(&summary);
    assert_eq!(
        invocation(plan, "fidelity-verifier").unwrap()["detail"]["claim_count"],
        3
    );
    let judge = invocation(plan, "fidelity-judge").expect("the judge's failure is recorded");
    assert_eq!(judge["decision"], "abstain");
    assert_eq!(judge["detail"]["judged"], false);
    let gap = &judge["gaps"][0];
    assert_eq!(gap["reason"], "evidence_missing");
    assert!(
        gap["detail"]
            .as_str()
            .unwrap()
            .contains("the judge model is not loaded"),
        "{gap}"
    );
}
