//! Quote-then-buy through the whole run path: the real Redpine adapter, over
//! the real transport, against a loopback origin serving the recorded
//! captures of 4 August 2026 (`recon/redpine/`) — the substitution every
//! adapter test uses. What is under test here is the run contract's v5
//! claims: the quote's price and billing note enter plan evidence before any
//! purchase decision, a declined quote is a recorded refusal that never
//! reaches `confirm`, an unrecorded preview workflow buys nothing, and a
//! trial-covered receipt is recorded as trial coverage, never as free.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::allowance::AllowanceContext;
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::RedpineAdapter;
use commonmeasure_types::{
    AllowanceDeclaration, AllowancePeriod, Constraint, ContextJob, Money, Objective, PolicyMode,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn capture(name: &str) -> Value {
    let path = Path::new(env!("COMMONMEASURE_EVIDENCE_DIR"))
        .join("recon/redpine")
        .join(name);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes).expect("a recon capture should be valid JSON")
}

/// The committed capture as the wire delivered it: recorded status, recorded
/// body bytes, and the recorded `mcp-session-id` header where one exists.
fn recorded(name: &str) -> Response {
    let recorded = capture(name);
    let mut response = Response::new(
        recorded["status"].as_u64().expect("status") as u16,
        recorded["body"].as_str().expect("body").as_bytes().to_vec(),
    );
    response.headers.set("Content-Type", "application/json");
    if let Some(session) = recorded["headers"]["mcp-session-id"].as_str() {
        response.headers.set("mcp-session-id", session);
    }
    response
}

/// A loopback Redpine origin routing by JSON-RPC body. `preview_override`
/// substitutes the preview leg where a test exercises a shape the recon
/// could not record (a non-trial quote; the field vocabulary beyond `trial`
/// has never been observed). Every other leg serves the recorded bytes.
fn redpine_origin(preview_override: Option<Value>) -> (ServerHandle, Arc<Mutex<Vec<String>>>) {
    let legs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&legs);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).expect("JSON request");
            let leg = match body["method"].as_str() {
                Some("initialize") => "initialize".to_owned(),
                Some("tools/call") => body["params"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                other => panic!("unexpected method {other:?}"),
            };
            seen.lock().expect("legs").push(leg.clone());
            match leg.as_str() {
                "initialize" => recorded("redpine-mcp-initialize.json"),
                "get_balance" => recorded("redpine-balance-before.json"),
                "inspect-tool" => recorded("redpine-inspect-sample-2.json"),
                "preview" => match &preview_override {
                    Some(preview) => {
                        Response::json(200, &serde_json::to_string(preview).expect("preview JSON"))
                    }
                    None => recorded("redpine-preview-sample.json"),
                },
                "confirm" => recorded("redpine-confirm-sample.json"),
                other => panic!("no capture for leg {other}"),
            }
        })
        .expect("spawn");
    (handle, legs)
}

/// A preview in the documented shape but outside trial coverage. The recon
/// only ever saw `billing_status: "trial"`; what a funded account's preview
/// carries is unrecorded, so this variant exists to exercise the decline
/// path and is honest about being a documented-shape construction.
fn non_trial_preview() -> Value {
    json!({"jsonrpc": "2.0", "id": 70, "result": {"content": [{"type": "text", "text": "preview"}],
    "isError": false,
    "structuredContent": {
        "preview_id": "pv_8d491df883034fec",
        "tool": "media--sample",
        "workflow": "requires_confirm",
        "billing_status": "balance",
        "billing_note": "Confirming will charge your balance at the tool's configured price."
    }}})
}

fn gateway_origin() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|_| {
            Response::json(
                200,
                r#"{"id":"tz-quoted","model":"local-test-model",
                    "choices":[{"message":{"content":"The supplied mentions describe recent coverage."}}],
                    "usage":{"prompt_tokens":40,"completion_tokens":8}}"#,
            )
        })
        .expect("spawn")
}

fn suite(mode: PolicyMode, constraints: Vec<Constraint>) -> Suite {
    Suite {
        suite_version: "quoted-test/v1".into(),
        label: "Quote-then-buy path".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "NVIDIA".into(),
            policy_mode: mode,
            objective: Objective::MinimiseLatency,
            constraints,
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 3,
        providers: vec!["redpine".into()],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

struct QuotedRun {
    _origin: ServerHandle,
    _gateway: ServerHandle,
    legs: Arc<Mutex<Vec<String>>>,
    directory: tempfile::TempDir,
    summary: Value,
}

impl QuotedRun {
    fn execute(suite: &Suite, preview_override: Option<Value>) -> Self {
        Self::execute_with_allowance(suite, preview_override, None)
    }

    fn execute_with_allowance(
        suite: &Suite,
        preview_override: Option<Value>,
        allowance: Option<AllowanceContext>,
    ) -> Self {
        let (origin, legs) = redpine_origin(preview_override);
        let gateway = gateway_origin();
        let directory = tempfile::tempdir().expect("tempdir");
        let base = origin.url();
        let options = RunOptions {
            allow_external_acquisition: true,
            output: directory.path().join("run"),
            suppliers: Box::new(move |provider| {
                assert_eq!(provider, "redpine", "this resolver serves one provider");
                Ok(Box::new(RedpineAdapter::new(&base, "test-key")))
            }),
            backend: Some(Box::new(
                TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
            )),
            replay: None,
            allowance,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        let report = execute(suite, &options).expect("the run should publish");
        Self {
            _origin: origin,
            _gateway: gateway,
            legs,
            directory,
            summary: report.summary,
        }
    }

    fn plan(&self) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == "redpine-only")
            .expect("the redpine plan")
    }

    fn published(&self) -> PathBuf {
        self.directory.path().join("run")
    }

    fn legs_sent(&self) -> Vec<String> {
        self.legs.lock().expect("legs").clone()
    }
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_trial_covered_purchase_completes_with_the_quote_in_evidence_before_the_receipt() {
    let suite = suite(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::from_decimal_str("USD", "0.05").expect("a valid cap"),
        }],
    );
    let run = QuotedRun::execute(&suite, None);
    let plan = run.plan();

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["capability"], "search");
    let quote = &plan["acquisition"]["quote"];
    assert_eq!(quote["preview_id"], "pv_8d491df883034fec");
    assert_eq!(quote["workflow"], "requires_confirm");
    assert_eq!(quote["billing_status"], "trial");
    assert!(
        quote["billing_note"]
            .as_str()
            .expect("the billing note enters evidence verbatim")
            .contains("Covered by free trial")
    );
    assert_eq!(quote["balance_before"], "0.00");
    assert_eq!(quote["decision"], "confirmed");
    assert!(
        quote["decision_reason"]
            .as_str()
            .expect("a confirmed purchase says why")
            .contains("covered by the trial")
    );
    // The quoted price: inspect-tool's annotation, in the provider's own
    // unit, marked quoted.
    assert_eq!(quote["charge"]["native"]["unit"], "credits");
    assert_eq!(quote["charge"]["native"]["basis"], "quoted");
    assert!(quote["charge"]["money"].is_null());

    // Both quote legs are sealed with hashes a reader can recheck.
    for (reference, hash) in [
        ("inspect_ref", "inspect_hash"),
        ("preview_ref", "preview_hash"),
    ] {
        let sealed = run
            .published()
            .join(quote[reference].as_str().expect("a sealed quote leg"));
        let bytes = std::fs::read(&sealed)
            .unwrap_or_else(|error| panic!("read {}: {error}", sealed.display()));
        assert_eq!(
            quote[hash],
            json!(format!("sha256:{:x}", {
                use sha2::Digest;
                sha2::Sha256::digest(&bytes)
            })),
            "the sealed {reference} must match its recorded hash"
        );
    }

    // The receipt: trial coverage observed, never free money.
    let charge = &plan["acquisition"]["charge"];
    assert!(charge["money"].is_null(), "no currency was observed");
    assert_eq!(charge["native"]["unit"], "trial_queries");
    assert_eq!(charge["native"]["basis"], "observed");
    assert!(
        charge["native"]["note"]
            .as_str()
            .expect("the receipt survives in the note")
            .contains("cost_charged 0.000000")
    );

    // Three licensed mentions, each dated by its supplier and carrying an
    // explicitly unknown licence: permitted use is not machine-readable
    // anywhere in the responses.
    let sources = plan["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), 3);
    for source in sources {
        assert_eq!(source["licence"]["state"], "unknown");
        assert_eq!(source["admitted"], true);
    }

    // The wire order is the contract: quote legs, then — only after the
    // decision — the one spending call.
    assert_eq!(
        run.legs_sent(),
        [
            "initialize",
            "get_balance",
            "inspect-tool",
            "preview",
            "confirm"
        ]
    );
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_declined_quote_is_a_recorded_refusal_and_confirm_is_never_sent() {
    // Strict mode with a declared cap, and a quote outside trial coverage
    // whose price (credits) the cap (USD) cannot be checked against: buying
    // at an unverifiable price is declined before any money moves.
    let suite = suite(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::from_decimal_str("USD", "0.05").expect("a valid cap"),
        }],
    );
    let run = QuotedRun::execute(&suite, Some(non_trial_preview()));
    let plan = run.plan();

    assert_eq!(plan["status"], "refused");
    let quote = &plan["acquisition"]["quote"];
    assert_eq!(quote["decision"], "declined");
    assert!(
        quote["decision_reason"]
            .as_str()
            .expect("a declined quote says why")
            .contains("unverifiable price"),
        "the decision names what could not be checked: {}",
        quote["decision_reason"]
    );
    assert_eq!(quote["billing_status"], "balance");
    // The decision is in the append-only trail, not only in the summary.
    let decisions = plan["policy_decisions"].as_array().expect("decisions");
    assert!(
        decisions
            .iter()
            .any(|decision| decision["decision"] == "refuse"),
        "a declined quote is a recorded refusal"
    );
    assert!(
        !plan["gaps"].as_array().expect("gaps").is_empty(),
        "the refusal leaves a recorded gap"
    );
    // Nothing was bought: the origin never saw a confirm.
    assert!(
        !run.legs_sent().iter().any(|leg| leg == "confirm"),
        "declining a quote must not settle it: {:?}",
        run.legs_sent()
    );
    // The quote legs are still sealed evidence.
    assert!(
        run.published()
            .join("responses/redpine-only-quote.json")
            .exists()
    );
    assert!(
        run.published()
            .join("responses/redpine-only-inspect.json")
            .exists()
    );
    // No receipt exists, so no confirm response was sealed.
    assert!(!run.published().join("responses/redpine-only.json").exists());
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_unverifiable_price_under_observe_buys_with_the_breach_recorded() {
    let suite = suite(
        PolicyMode::Observe,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::from_decimal_str("USD", "0.05").expect("a valid cap"),
        }],
    );
    let run = QuotedRun::execute(&suite, Some(non_trial_preview()));
    let plan = run.plan();

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["acquisition"]["quote"]["decision"], "confirmed");
    assert!(
        plan["gaps"]
            .as_array()
            .expect("gaps")
            .iter()
            .any(|gap| gap["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("could not be verified")),
        "observe mode buys, and the unverifiable cap is a recorded breach"
    );
    assert!(run.legs_sent().iter().any(|leg| leg == "confirm"));
}

fn usd_day_allowance(home: &Path) -> AllowanceContext {
    AllowanceContext::new(
        home,
        "os-user:test".to_owned(),
        vec![AllowanceDeclaration {
            period: AllowancePeriod::Day,
            amount: Money::from_decimal_str("USD", "0.02").expect("a valid amount"),
            timezone: "UTC".to_owned(),
        }],
    )
}

/// The run contract's v7 claim, on the one real quote shape that exists
/// today: Redpine prices in credits, a monetary allowance holds no rate
/// source, so strict declines before `confirm` — the allowance is the second
/// gate and its record is separate from the job cap's.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_credits_quote_under_a_monetary_allowance_is_declined_in_strict_before_confirm() {
    let home = tempfile::tempdir().expect("tempdir");
    // No job cap: the job's own gate allows, so what declines is the
    // principal's allowance alone.
    let suite = suite(PolicyMode::Strict, vec![]);
    let run = QuotedRun::execute_with_allowance(
        &suite,
        Some(non_trial_preview()),
        Some(usd_day_allowance(home.path())),
    );
    let plan = run.plan();

    assert_eq!(plan["status"], "refused");
    let allowance = &plan["acquisition"]["quote"]["allowance"];
    assert_eq!(allowance["consulted"], true);
    assert_eq!(allowance["principal"], "os-user:test");
    assert_eq!(allowance["declared"], true);
    assert_eq!(allowance["decision"], "declined");
    assert!(
        allowance["reason"]
            .as_str()
            .expect("a declined allowance says why")
            .contains("monetary allowance cannot be checked against"),
        "the reason names the incomparable price: {}",
        allowance["reason"]
    );
    assert!(
        !run.legs_sent().iter().any(|leg| leg == "confirm"),
        "an allowance refusal must never settle: {:?}",
        run.legs_sent()
    );
    // Nothing was reserved: an unpriceable quote never touches the ledger.
    assert!(
        !home.path().join("allowance/ledger.ndjson").exists(),
        "no ledger entry may exist for a purchase that never reserved"
    );
    // The refusal is in the append-only trail with the allowance's own gap.
    assert!(
        plan["policy_decisions"]
            .as_array()
            .expect("decisions")
            .iter()
            .any(|decision| decision["decision"] == "refuse"),
        "the allowance refusal is a recorded decision"
    );
}

/// Observe mode records spending it cannot verify instead of preventing it:
/// the purchase completes, the breach is a recorded gap, and the ledger
/// holds nothing because there was no money figure to hold.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_incomparable_quote_under_observe_buys_with_the_allowance_breach_recorded() {
    let home = tempfile::tempdir().expect("tempdir");
    let suite = suite(PolicyMode::Observe, vec![]);
    let run = QuotedRun::execute_with_allowance(
        &suite,
        Some(non_trial_preview()),
        Some(usd_day_allowance(home.path())),
    );
    let plan = run.plan();

    assert_eq!(plan["status"], "completed");
    let allowance = &plan["acquisition"]["quote"]["allowance"];
    assert_eq!(allowance["decision"], "proceeded_with_breach");
    assert!(
        plan["gaps"]
            .as_array()
            .expect("gaps")
            .iter()
            .any(|gap| gap["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("cumulative allowance")),
        "the allowance breach is a recorded gap: {}",
        plan["gaps"]
    );
    assert!(run.legs_sent().iter().any(|leg| leg == "confirm"));
}

/// Trial coverage is verifiable "no charge applies", not an unknown price:
/// nothing is reserved and the record says so in words.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_trial_covered_purchase_reserves_nothing_and_says_so() {
    let home = tempfile::tempdir().expect("tempdir");
    let suite = suite(PolicyMode::Strict, vec![]);
    let run = QuotedRun::execute_with_allowance(&suite, None, Some(usd_day_allowance(home.path())));
    let plan = run.plan();

    assert_eq!(plan["status"], "completed");
    let allowance = &plan["acquisition"]["quote"]["allowance"];
    assert_eq!(allowance["decision"], "nothing_reserved");
    assert!(
        allowance["reason"]
            .as_str()
            .expect("the trial coverage is stated")
            .contains("trial-covered"),
    );
    assert!(!home.path().join("allowance/ledger.ndjson").exists());
}

/// A run holding no allowance context — started without principal policy, or
/// replaying — states the absence rather than implying an unlimited
/// allowance was checked.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_run_without_principal_policy_records_the_allowance_absence() {
    let suite = suite(PolicyMode::Strict, vec![]);
    let run = QuotedRun::execute(&suite, None);
    let allowance = &run.plan()["acquisition"]["quote"]["allowance"];
    assert_eq!(allowance["consulted"], false);
    assert!(
        allowance["reason"]
            .as_str()
            .expect("the absence is stated")
            .contains("no principal allowance context")
    );
}

/// A principal that declares no allowance is a different fact from a missing
/// context, and the record keeps them apart.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_principal_with_no_declared_allowance_is_stated_as_such() {
    let home = tempfile::tempdir().expect("tempdir");
    let suite = suite(PolicyMode::Strict, vec![]);
    let context = AllowanceContext::new(home.path(), "os-user:test".to_owned(), Vec::new());
    let run = QuotedRun::execute_with_allowance(&suite, None, Some(context));
    let allowance = &run.plan()["acquisition"]["quote"]["allowance"];
    assert_eq!(allowance["consulted"], true);
    assert_eq!(allowance["declared"], false);
    assert!(
        allowance["reason"]
            .as_str()
            .expect("the undeclared state is stated")
            .contains("declares no allowance")
    );
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_unrecorded_preview_workflow_buys_nothing_and_takes_nothing_from_the_teaser() {
    // `workflow: "complete"` is documented to mean the data is free and
    // already unlocked — a settlement path with no recorded evidence, so the
    // run records the gap instead of guessing at the shape.
    let mut preview = non_trial_preview();
    preview["result"]["structuredContent"]["workflow"] = json!("complete");
    let suite = suite(PolicyMode::Strict, vec![]);
    let run = QuotedRun::execute(&suite, Some(preview));
    let plan = run.plan();

    assert_eq!(plan["status"], "unavailable");
    assert_eq!(plan["acquisition"]["quote"]["decision"], "not_attempted");
    assert!(
        plan["gaps"]
            .as_array()
            .expect("gaps")
            .iter()
            .any(|gap| gap["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("workflow")),
        "the gap names the unrecorded workflow"
    );
    assert!(!run.legs_sent().iter().any(|leg| leg == "confirm"));
    assert!(
        plan["sources"].as_array().expect("sources").is_empty(),
        "nothing from the teaser may be offered as supply"
    );
}
