//! The dispatch gate through the whole run path: the real Exa and Tavily
//! adapters, over the real transport, against loopback origins serving the
//! recorded captures of 1 August 2026 (`demo/recon/`). Under test are the
//! run contract's v7 claims for acquisitions that are not quote-then-buy:
//! Exa's spec-verified published price is reserved before dispatch and
//! reconciled against the observed receipt, a sequence of individually
//! affordable searches is stopped before the first excess one, an unpriced
//! provider under an exhausted allowance refuses before dispatch in strict
//! and proceeds with the breach recorded in observe, and every unit the
//! ledger holds is explained by its own file.

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chrono::Utc;
use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_runtime::allowance::{Account, AllowanceContext};
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::{ExaAdapter, TavilyAdapter};
use commonmeasure_types::{
    AllowanceDeclaration, AllowancePeriod, ContextJob, Money, Objective, PolicyMode,
};
use serde_json::Value;
use uuid::Uuid;

fn recorded_body(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-runtime sits two levels below the repository root")
        .join("demo/recon")
        .join(relative);
    let capture: Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .expect("a recon capture should be valid JSON");
    // Recon captures store the redacted response body as parsed JSON; the
    // wire form is its serialisation, which is what the adapter parses here.
    serde_json::to_vec(&capture["response"]["body"]).expect("the recorded body")
}

/// A loopback origin serving one recorded response body, counting the
/// requests that actually reach it — which is how a pre-dispatch refusal is
/// proved to have dispatched nothing.
fn origin(relative: &str) -> (ServerHandle, Arc<AtomicUsize>) {
    origin_with(relative, || {})
}

/// The same origin, running `on_request` before it answers: the one moment
/// that lies between the reservation and the settlement of a dispatch.
fn origin_with(
    relative: &str,
    on_request: impl Fn() + Send + Sync + 'static,
) -> (ServerHandle, Arc<AtomicUsize>) {
    let body = recorded_body(relative);
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&hits);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            on_request();
            let mut response = Response::new(200, body.clone());
            response.headers.set("Content-Type", "application/json");
            response
        })
        .expect("spawn");
    (handle, hits)
}

fn suite(mode: PolicyMode, provider: &str) -> Suite {
    Suite {
        suite_version: "allowance-test/v1".into(),
        label: "Dispatch gate".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "UK energy price cap".into(),
            policy_mode: mode,
            objective: Objective::MinimiseLatency,
            constraints: vec![],
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 2,
        providers: vec![provider.into()],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

fn day_usd(home: &Path, micros: u64) -> AllowanceContext {
    AllowanceContext::new(
        home,
        "os-user:test".to_owned(),
        vec![AllowanceDeclaration {
            period: AllowancePeriod::Day,
            amount: Money::new("USD", micros),
            timezone: "UTC".to_owned(),
        }],
    )
}

fn run_exa(home: &Path, mode: PolicyMode, micros: u64) -> (Value, usize) {
    run_exa_against(home, mode, micros, origin("exa/exa-search.json"))
}

fn run_exa_against(
    home: &Path,
    mode: PolicyMode,
    micros: u64,
    (origin, hits): (ServerHandle, Arc<AtomicUsize>),
) -> (Value, usize) {
    let base = origin.url();
    let directory = tempfile::tempdir().expect("tempdir");
    let options = RunOptions {
        allow_external_acquisition: true,
        output: directory.path().join("run"),
        suppliers: Box::new(move |provider| {
            assert_eq!(provider, "exa");
            Ok(Box::new(ExaAdapter::new(&base, "test-key")))
        }),
        backend: None,
        replay: None,
        allowance: Some(day_usd(home, micros)),
        provenance_signing:
            commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
    };
    let report = execute(&suite(mode, "exa"), &options).expect("the run should publish");
    let plan = report.summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == "exa-only")
        .expect("the exa plan")
        .clone();
    (plan, hits.load(Ordering::SeqCst))
}

/// The package's acceptance line, on the one provider whose price is real in
/// currency: two searches at Exa's published 0.007000 USD reserve and
/// reconcile against the observed receipt; the third would exceed a 0.020000
/// USD day and is refused before anything is dispatched.
#[test]
fn the_third_search_is_refused_before_dispatch_when_it_would_exceed_the_day() {
    let home = tempfile::tempdir().expect("tempdir");
    for _ in 0..2 {
        let (plan, hits) = run_exa(home.path(), PolicyMode::Strict, 20_000);
        let allowance = &plan["acquisition"]["allowance"];
        assert_eq!(allowance["decision"], "reserved");
        assert!(
            allowance["price_basis"]
                .as_str()
                .expect("the reservation names its basis")
                .contains("published price"),
        );
        assert_eq!(allowance["settlement"]["reconciled"], true);
        assert!(
            allowance["settlement"]["note"]
                .as_str()
                .expect("the settlement note")
                .contains("receipt matches the quote"),
            "the observed 0.007000 USD matches the published price: {}",
            allowance["settlement"]["note"]
        );
        assert_eq!(hits, 1, "one search reached the origin");
    }

    let (plan, hits) = run_exa(home.path(), PolicyMode::Strict, 20_000);
    assert_eq!(plan["status"], "refused");
    let allowance = &plan["acquisition"]["allowance"];
    assert_eq!(allowance["decision"], "declined");
    assert!(
        allowance["reason"]
            .as_str()
            .expect("a declined dispatch says why")
            .contains("0.006000 USD remaining"),
        "the reason carries the arithmetic: {}",
        allowance["reason"]
    );
    assert_eq!(hits, 0, "a refusal before dispatch reaches no origin");

    // The ledger explains every unit: two commits, nothing still reserved.
    let context = day_usd(home.path(), 20_000);
    let state = context
        .ledger
        .state(&context.account(), Utc::now())
        .expect("the ledger is readable");
    assert_eq!(state[0].committed, Money::new("USD", 14_000));
    assert_eq!(state[0].reserved, Money::new("USD", 0));
}

/// Observe mode records the same arithmetic and buys: the breach is in the
/// record, the ledger shows the overdraft, and nothing is hidden.
#[test]
fn observe_mode_buys_past_the_day_with_the_breach_and_the_overdraft_recorded() {
    let home = tempfile::tempdir().expect("tempdir");
    for _ in 0..2 {
        let (plan, _) = run_exa(home.path(), PolicyMode::Observe, 10_000);
        assert_ne!(plan["status"], "refused");
    }
    let (plan, hits) = run_exa(home.path(), PolicyMode::Observe, 10_000);
    assert_ne!(plan["status"], "refused");
    let allowance = &plan["acquisition"]["allowance"];
    assert_eq!(allowance["decision"], "proceeded_with_breach");
    assert_eq!(hits, 1, "observe dispatches");

    let context = day_usd(home.path(), 10_000);
    let state = context
        .ledger
        .state(&context.account(), Utc::now())
        .expect("the ledger is readable");
    assert_eq!(state[0].committed, Money::new("USD", 21_000));
    assert!(state[0].exceeded, "the overdraft is stated, not hidden");
}

/// Tavily declares no price and reports no charge. With room in the
/// allowance the dispatch proceeds and nothing is committed (unknown stays
/// unknown, never zero); with the allowance exhausted, strict refuses before
/// dispatch because nothing further can be accounted.
#[test]
fn an_unpriced_provider_is_refused_only_once_the_allowance_is_exhausted() {
    let home = tempfile::tempdir().expect("tempdir");
    let run = |mode: PolicyMode| {
        let (origin, hits) = origin("tavily/tavily-search.json");
        let base = origin.url();
        let directory = tempfile::tempdir().expect("tempdir");
        let options = RunOptions {
            allow_external_acquisition: true,
            output: directory.path().join("run"),
            suppliers: Box::new(move |provider| {
                assert_eq!(provider, "tavily");
                Ok(Box::new(TavilyAdapter::new(&base, "test-key")))
            }),
            backend: None,
            replay: None,
            allowance: Some(day_usd(home.path(), 5_000)),
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        let report = execute(&suite(mode, "tavily"), &options).expect("the run should publish");
        let plan = report.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == "tavily-only")
            .expect("the tavily plan")
            .clone();
        (plan, hits.load(Ordering::SeqCst))
    };

    let (plan, hits) = run(PolicyMode::Strict);
    assert_ne!(plan["status"], "refused");
    let allowance = &plan["acquisition"]["allowance"];
    assert_eq!(allowance["decision"], "unpriced_within_allowance");
    assert!(
        allowance.get("settlement").is_none(),
        "no charge was observed, so nothing was committed: {allowance}"
    );
    assert_eq!(hits, 1);

    // Exhaust the allowance through the real ledger, as spend would.
    let context = day_usd(home.path(), 5_000);
    context
        .ledger
        .commit_observed(
            &Account {
                principal: "os-user:test",
                declarations: &context.declarations,
            },
            &Money::new("USD", 5_000),
            Utc::now(),
            "test setup",
            "spend that exhausts the day",
        )
        .expect("the ledger accepts the commit");

    let (plan, hits) = run(PolicyMode::Strict);
    assert_eq!(plan["status"], "refused");
    let allowance = &plan["acquisition"]["allowance"];
    assert_eq!(allowance["decision"], "declined");
    assert!(
        allowance["reason"]
            .as_str()
            .expect("the refusal says why")
            .contains("declares no price"),
        "{}",
        allowance["reason"]
    );
    assert_eq!(hits, 0, "an exhausted allowance dispatches nothing");

    let (plan, hits) = run(PolicyMode::Observe);
    assert_ne!(plan["status"], "refused");
    assert_eq!(
        plan["acquisition"]["allowance"]["decision"],
        "proceeded_with_breach"
    );
    assert_eq!(hits, 1, "observe records the breach and proceeds");
}

/// The double fault: the reservation was written, the search ran and the
/// receipt arrived, and the ledger refused the commit twice. The plan says
/// so where a reader will find it — a gap naming the reservation, the
/// observed charge and the reason — because the ledger's own account is now
/// short of the receipt: the commit is missing, and the reservation stays
/// until the expiry sweep releases it as unsettled.
#[test]
fn a_settlement_the_ledger_cannot_write_is_a_gap_naming_the_money_that_moved() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = tempfile::tempdir().expect("tempdir");
    let ledger_file = home.path().join("allowance/ledger.ndjson");
    // The origin answers after making the ledger read-only: the reservation
    // is already on disk, and the settlement that follows the receipt is
    // the first write the ledger refuses.
    let refuse_from_here = ledger_file.clone();
    let origin = origin_with("exa/exa-search.json", move || {
        std::fs::set_permissions(&refuse_from_here, std::fs::Permissions::from_mode(0o444))
            .expect("the reservation line exists before the origin is reached");
    });
    let (plan, hits) = run_exa_against(home.path(), PolicyMode::Strict, 20_000, origin);
    assert_eq!(hits, 1);
    assert_ne!(plan["status"], "refused");

    let allowance = &plan["acquisition"]["allowance"];
    assert_eq!(allowance["decision"], "reserved");
    let reservation_id = allowance["reservation_id"]
        .as_str()
        .expect("the reservation is named");
    let settlement = &allowance["settlement"];
    assert_eq!(settlement["reconciled"], false);
    assert_eq!(settlement["reservation_id"], reservation_id);
    assert_eq!(settlement["observed"]["micros"], 7_000);
    assert!(
        settlement["error"]
            .as_str()
            .expect("the error is stated")
            .contains("retried once under the lock"),
        "{settlement}"
    );

    let gaps = plan["gaps"].as_array().expect("gaps");
    let detail = gaps
        .iter()
        .filter(|gap| gap["reason"] == "evidence_missing")
        .filter_map(|gap| gap["detail"].as_str())
        .find(|detail| detail.starts_with("Money moved"))
        .unwrap_or_else(|| panic!("the plan states the uncounted charge: {gaps:?}"));
    assert!(detail.contains(reservation_id), "{detail}");
    assert!(detail.contains("0.007000 USD"), "{detail}");
    assert!(detail.contains("expiry sweep"), "{detail}");

    // The ledger's own account after the fault: the reservation still held,
    // nothing committed — the state the gap describes.
    std::fs::set_permissions(&ledger_file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let context = day_usd(home.path(), 20_000);
    let state = context
        .ledger
        .state(&context.account(), Utc::now())
        .expect("the ledger is readable");
    assert_eq!(state[0].committed, Money::new("USD", 0));
    assert_eq!(state[0].reserved, Money::new("USD", 7_000));
}
