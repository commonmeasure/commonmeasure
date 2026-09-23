//! Policy and selection decisions that a full run cannot reach without either
//! spending money or hiding the decision inside a larger assertion, plus the
//! two places where a plan's published admission reason has to agree with the
//! plan's own gap. Those last two run the whole path over supply held in
//! memory: the decision is about what the runtime says, not about a provider.

use std::sync::Arc;

use commonmeasure_runtime::policy::{self, Ruling};
use commonmeasure_runtime::selection::{self, Candidate};
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::{Acquisition, SupplyAdapter, SupplyError};
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, Constraint, ContextEnvelope, ContextJob, LicenceState,
    ModelPlan, Money, NativeCharge, Objective, PolicyMode, ProviderCapability,
};
use serde_json::Value;
use uuid::Uuid;

fn job(policy_mode: PolicyMode, constraints: Vec<Constraint>) -> ContextJob {
    ContextJob {
        id: Uuid::new_v4(),
        kind: "research.answer".into(),
        prompt: "test".into(),
        policy_mode,
        objective: Objective::MinimiseLatency,
        constraints,
        evidence_requirements: vec![],
    }
}

fn charge(money: Money) -> AcquisitionCharge {
    AcquisitionCharge {
        money: Some(money),
        native: None,
    }
}

fn envelope(url: &str, host: &str, text: Option<&str>) -> ContextEnvelope {
    ContextEnvelope {
        source_url: url.into(),
        host: host.into(),
        title: Some("A source".into()),
        text: text.map(str::to_owned),
        content_hash: text.map(|_| "sha256:test".to_owned()),
        licence: LicenceState::Unknown,
        declared_date: None,
        native_metadata: serde_json::json!({}),
        retrieval_rank: 1,
    }
}

/// A denied provider is refused before any request is sent, so it never sees
/// the job's prompt.
#[test]
fn a_denied_provider_is_refused_under_strict_policy() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::DeniedProvider {
            provider: "tollbit".into(),
        }],
    );
    let ruling = policy::provider_eligibility(&job, "tollbit");
    assert!(ruling.is_refusal());
    assert_eq!(
        ruling.gap().map(|gap| gap.reason),
        Some(commonmeasure_types::GapReason::PolicyRefused)
    );
    assert!(!policy::provider_eligibility(&job, "exa").is_refusal());
}

/// An allow-list excludes everything it does not name.
#[test]
fn an_allow_list_excludes_providers_it_does_not_name() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::AllowedProvider {
            provider: "exa".into(),
        }],
    );
    assert!(policy::provider_eligibility(&job, "firecrawl").is_refusal());
    assert!(!policy::provider_eligibility(&job, "exa").is_refusal());
}

/// Under `observe` the same breach is carried rather than refused, and it is
/// still recorded. A mode changes enforcement, never the record.
#[test]
fn observe_mode_records_the_breach_it_does_not_enforce() {
    let job = job(
        PolicyMode::Observe,
        vec![Constraint::DeniedProvider {
            provider: "tollbit".into(),
        }],
    );
    let ruling = policy::provider_eligibility(&job, "tollbit");
    assert!(!ruling.is_refusal());
    assert!(matches!(ruling, Ruling::AllowedWithBreach { .. }));
    assert!(
        ruling.gap().is_some(),
        "an unenforced breach is still a gap"
    );
}

/// Latency is observed after the fact, so the cap cannot pre-empt a slow
/// provider — it stops a result that arrived too late from being used. The
/// breach names both numbers, because "too slow" without them is unauditable.
/// A job that declares no cap has nothing to breach.
#[test]
fn a_plan_that_overran_the_latency_cap_is_refused_with_both_numbers() {
    let capped = job(
        PolicyMode::Strict,
        vec![Constraint::MaximumLatencyMs {
            milliseconds: 5_000,
        }],
    );
    assert!(!policy::total_latency(&capped, 5_000).is_refusal());

    let ruling = policy::total_latency(&capped, 28_710);
    assert!(ruling.is_refusal());
    assert_eq!(
        ruling.gap().map(|gap| gap.reason),
        Some(commonmeasure_types::GapReason::PolicyRefused)
    );
    let stated = format!("{ruling:?}");
    assert!(
        stated.contains("28710") && stated.contains("5000"),
        "the breach must state the observed latency against the cap: {stated}"
    );

    // Under observe the same overrun is carried and still recorded.
    let observed = job(
        PolicyMode::Observe,
        vec![Constraint::MaximumLatencyMs {
            milliseconds: 5_000,
        }],
    );
    let carried = policy::total_latency(&observed, 28_710);
    assert!(!carried.is_refusal());
    assert!(
        carried.gap().is_some(),
        "an unenforced overrun is still a gap"
    );

    // No declared cap, nothing to breach, however long it took.
    assert!(matches!(
        policy::total_latency(&job(PolicyMode::Strict, vec![]), u64::MAX),
        Ruling::Allowed
    ));
}

/// A result with no excerpt is refused whatever the mode: it cannot ground an
/// answer, and admitting it would put a citation in the record with nothing
/// behind it.
#[test]
fn a_source_without_text_is_refused_in_every_mode() {
    for mode in [PolicyMode::Observe, PolicyMode::Prefer, PolicyMode::Strict] {
        let ruling = policy::source_admission(
            &job(mode, vec![]),
            &envelope("https://example.test/a", "example.test", None),
        );
        assert!(
            ruling.is_refusal(),
            "mode {mode:?} admitted a textless source"
        );
    }
}

/// A job that demands a named licence is not satisfied by absence. No open-web
/// provider returns licence metadata, so this is the ordinary case, and the
/// answer must be "unknown, therefore refused", never "unknown, therefore fine".
#[test]
fn a_required_licence_is_not_satisfied_by_an_unknown_one() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::RequiredLicence {
            licence: "cc-by-4.0".into(),
        }],
    );
    let ruling = policy::source_admission(
        &job,
        &envelope("https://example.test/a", "example.test", Some("text")),
    );
    assert!(ruling.is_refusal());

    let mut licensed = envelope("https://example.test/a", "example.test", Some("text"));
    licensed.licence = LicenceState::Declared {
        reference: "cc-by-4.0".into(),
    };
    assert!(!policy::source_admission(&job, &licensed).is_refusal());
}

/// A host list is matched against the host the envelope actually carries,
/// which came out of a URL parser: lowercased, IDNA-encoded, no trailing dot.
/// An operator spells hosts the way a human writes them, and a deny-list that
/// misses on spelling is not a deny-list.
#[test]
fn a_denied_host_is_matched_however_the_operator_spelled_it() {
    for entry in ["banned.example.", "Banned.Example", "BANNED.EXAMPLE."] {
        let job = job(
            PolicyMode::Strict,
            vec![Constraint::DeniedSourceHost { host: entry.into() }],
        );
        let ruling = policy::source_admission(
            &job,
            &envelope("https://banned.example./a", "banned.example", Some("text")),
        );
        assert!(ruling.is_refusal(), "the entry {entry:?} failed to deny");
    }

    // The envelope's host is the A-label the URL parser produced. An operator
    // writing the name in its own script is naming that host.
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::DeniedSourceHost {
            host: "bänned.example".into(),
        }],
    );
    let ruling = policy::source_admission(
        &job,
        &envelope(
            "https://xn--bnned-gra.example/a",
            "xn--bnned-gra.example",
            Some("text"),
        ),
    );
    assert!(
        ruling.is_refusal(),
        "a Unicode deny entry must match the punycode host it names"
    );

    // Normalisation widens nothing: a different host is still a different host.
    let ruling = policy::source_admission(
        &job,
        &envelope("https://allowed.example/a", "allowed.example", Some("text")),
    );
    assert!(!ruling.is_refusal());
}

/// An internal-corpus document has no host. A host allow-list is a containment
/// control, so a source it cannot name stays outside it — and the refusal says
/// the source has no host rather than printing a blank where one would be.
#[test]
fn a_hostless_source_is_outside_an_allow_list_and_the_refusal_says_why() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::AllowedSourceHost {
            host: "www.gov.uk".into(),
        }],
    );
    let ruling =
        policy::source_admission(&job, &envelope("file:///corpus/note.md", "", Some("text")));
    assert!(ruling.is_refusal());
    let reason = ruling.reason().expect("a refusal carries a reason");
    assert!(
        reason.contains("no host"),
        "the refusal must name the case rather than print a blank host: {reason}"
    );
    assert!(
        !reason.contains("Host  "),
        "the refusal printed an empty host: {reason}"
    );
}

/// An unreported charge cannot be checked against a cap. Treating it as zero
/// would let the one provider that discloses nothing pass every budget.
#[test]
fn an_unreported_charge_makes_the_cap_unenforceable_not_satisfied() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        }],
    );
    let ruling = policy::acquisition_cost(&job, &AcquisitionCharge::default());
    assert!(!ruling.is_refusal(), "an unknown charge is not a breach");
    assert_eq!(
        ruling.gap().map(|gap| gap.reason),
        Some(commonmeasure_types::GapReason::EvidenceMissing),
        "an unenforceable cap must leave a gap saying so"
    );

    // A charge in another currency is equally unenforceable, not equally fine.
    let ruling = policy::acquisition_cost(&job, &charge(Money::new("GBP", 10)));
    assert!(ruling.gap().is_some());
    assert!(!ruling.is_refusal());
}

/// A charge disclosed in the provider's own units, too fine to carry at micro
/// precision, is a disclosed price. The cap is unenforceable either way, but
/// saying the provider reported nothing would misdescribe the provider.
#[test]
fn a_price_too_fine_for_micros_is_not_reported_as_no_price() {
    let job = job(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        }],
    );
    let ruling = policy::acquisition_cost(
        &job,
        &AcquisitionCharge {
            money: None,
            native: Some(NativeCharge {
                unit: "USD".into(),
                amount: serde_json::Number::from_f64(0.0000005).unwrap(),
                basis: ChargeBasis::Observed,
                note: Some("finer than micro precision".into()),
            }),
        },
    );
    assert!(!ruling.is_refusal(), "an unenforceable cap is not a breach");
    let reason = ruling.reason().expect("a breach carries a reason");
    assert!(
        !reason.contains("no charge"),
        "a disclosed price must not be published as no price: {reason}"
    );
    assert!(
        reason.contains("could not be enforced"),
        "the cap is genuinely unenforceable and the reason must say so: {reason}"
    );
    let detail = &ruling
        .gap()
        .expect("an unenforceable cap leaves a gap")
        .detail;
    assert!(
        detail.contains("finer than micro precision"),
        "the gap must carry the provider's own reason: {detail}"
    );

    // Silence still reads as silence.
    let ruling = policy::acquisition_cost(&job, &AcquisitionCharge::default());
    assert!(
        ruling.reason().expect("a reason").contains("no charge"),
        "an undisclosed charge must still say the provider disclosed nothing"
    );
}

/// The purchase decision runs before money moves, which is what changes the
/// posture: a comparable quote over the cap is declined in strict mode, and
/// so is an unverifiable one — pre-purchase, "buy at a price the cap cannot
/// be checked against" is itself the decision, where the post-hoc check on a
/// done deed can only record a gap.
#[test]
fn a_strict_purchase_decision_declines_over_cap_and_unverifiable_quotes() {
    let strict = job(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        }],
    );

    let over_cap = policy::purchase_decision(&strict, &charge(Money::new("USD", 60_000)), false);
    assert!(
        over_cap.is_refusal(),
        "a comparable over-cap quote declines"
    );
    assert_eq!(
        over_cap.gap().map(|gap| gap.reason),
        Some(commonmeasure_types::GapReason::BudgetExhausted)
    );

    let unverifiable = policy::purchase_decision(
        &strict,
        &AcquisitionCharge {
            money: None,
            native: Some(NativeCharge {
                unit: "credits".into(),
                amount: serde_json::Number::from_f64(0.03).unwrap(),
                basis: ChargeBasis::Quoted,
                note: None,
            }),
        },
        false,
    );
    assert!(
        unverifiable.is_refusal(),
        "strict mode does not buy at a price the cap cannot be checked against"
    );
    assert!(
        unverifiable
            .reason()
            .expect("a declined quote says why")
            .contains("unverifiable price")
    );

    let silent = policy::purchase_decision(&strict, &AcquisitionCharge::default(), false);
    assert!(silent.is_refusal(), "no quoted price at all declines too");
}

/// Trial coverage is the supplier stating no charge applies, which is
/// verifiable coverage rather than an unknown price; observe mode buys at an
/// unverifiable price with the breach recorded; and without a cap there is
/// nothing to decline.
#[test]
fn trial_coverage_observe_mode_and_capless_jobs_all_proceed() {
    let strict = job(
        PolicyMode::Strict,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        }],
    );
    assert_eq!(
        policy::purchase_decision(&strict, &AcquisitionCharge::default(), true),
        Ruling::Allowed,
        "trial coverage proceeds whatever the cap"
    );

    let observe = job(
        PolicyMode::Observe,
        vec![Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        }],
    );
    let bought = policy::purchase_decision(&observe, &AcquisitionCharge::default(), false);
    assert!(!bought.is_refusal(), "observe mode buys");
    assert!(
        bought.gap().is_some(),
        "the unverifiable cap is still a recorded breach"
    );

    let capless = job(PolicyMode::Strict, vec![]);
    assert_eq!(
        policy::purchase_decision(&capless, &AcquisitionCharge::default(), false),
        Ruling::Allowed
    );
}

/// A cap typed `"usd"` in the job file must still be
/// the cap. Deserialisation normalises the currency, so the comparison against
/// an adapter's `"USD"` charge happens — and an over-cap charge refuses in
/// strict instead of sliding down the incomparable-currencies branch, which
/// never refuses.
#[test]
fn a_lower_case_cap_still_refuses_in_strict() {
    let job: ContextJob = serde_json::from_value(serde_json::json!({
        "id": "6f2f9c58-0000-4000-8000-000000000001",
        "kind": "research.answer",
        "prompt": "What does the evidence say?",
        "policy_mode": "strict",
        "objective": {"kind": "maximise_quality"},
        "constraints": [
            {"kind": "maximum_acquisition_cost",
             "amount": {"currency": "usd", "micros": 50_000}}
        ]
    }))
    .unwrap();
    job.validate().unwrap();
    let ruling = policy::acquisition_cost(&job, &charge(Money::new("USD", 60_000)));
    assert!(
        ruling.is_refusal(),
        "an over-cap charge must refuse under strict, whatever case the job file used"
    );
}

/// Token counting is an approximation and is named as one, because a number
/// labelled "tokens" that no tokeniser produced is borrowed precision.
#[test]
fn the_token_basis_is_declared_rather_than_implied() {
    assert_eq!(policy::TOKEN_BASIS, "whitespace-words");
    assert_eq!(policy::approximate_tokens("one two three"), 3);
    assert_eq!(policy::approximate_tokens(""), 0);
}

/// The objective in the shipped suite weights quality and coverage, which no
/// evaluator measures. Breaking that tie on latency because latency happens to
/// be observable would publish a provider verdict the run did not measure.
#[test]
fn an_objective_needing_an_evaluator_abstains_and_names_what_is_missing() {
    let candidates = vec![
        Candidate {
            plan_id: "exa-only".into(),
            completed: true,
            latency_ms: Some(10),
            observed_cost: Some(Money::new("USD", 7_000)),
            coverage: None,
            freshness: None,
        },
        Candidate {
            plan_id: "tavily-only".into(),
            completed: true,
            latency_ms: Some(20),
            observed_cost: None,
            coverage: None,
            freshness: None,
        },
    ];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 1.0,
            coverage: 1.0,
            freshness: 0.8,
            cost: 0.4,
            latency: 0.2,
            policy_risk: 1.0,
        },
        &candidates,
        false,
    );
    assert!(selection.plan_id.is_none());
    for missing in ["quality", "coverage", "freshness", "policy_risk"] {
        assert!(
            selection.unavailable_inputs.iter().any(|m| m == missing),
            "{missing} should be named as an unavailable input"
        );
    }
}

/// Cost selection needs every candidate to have reported a charge. An
/// unreported charge is unknown, not free, so the router abstains, names the
/// measurement it lacks and says in prose which plans withheld it. One field,
/// one vocabulary: a reader comparing `unavailable_inputs` across arms must not
/// find plan identifiers under a field that elsewhere holds measurement names.
#[test]
fn cost_selection_abstains_when_a_candidate_disclosed_no_price() {
    let candidates = vec![
        Candidate {
            plan_id: "exa-only".into(),
            completed: true,
            latency_ms: Some(10),
            observed_cost: Some(Money::new("USD", 7_000)),
            coverage: None,
            freshness: None,
        },
        Candidate {
            plan_id: "tavily-only".into(),
            completed: true,
            latency_ms: Some(20),
            observed_cost: None,
            coverage: None,
            freshness: None,
        },
    ];
    let selection = selection::select(&Objective::MinimiseCost, &candidates, false);
    assert!(selection.plan_id.is_none());
    assert_eq!(
        selection.unavailable_inputs,
        vec!["observed_cost".to_owned()],
        "unavailable_inputs names the measurement, as every other arm does"
    );
    assert!(
        selection.explanation.contains("tavily-only"),
        "the plan that disclosed nothing belongs in the prose: {}",
        selection.explanation
    );
    assert!(
        !selection.explanation.contains("exa-only"),
        "the plan that did disclose must not be named as withholding: {}",
        selection.explanation
    );

    // With both disclosing in one currency the objective is computable.
    let candidates = vec![
        Candidate {
            plan_id: "exa-only".into(),
            completed: true,
            latency_ms: Some(10),
            observed_cost: Some(Money::new("USD", 7_000)),
            coverage: None,
            freshness: None,
        },
        Candidate {
            plan_id: "other".into(),
            completed: true,
            latency_ms: Some(20),
            observed_cost: Some(Money::new("USD", 1_000)),
            coverage: None,
            freshness: None,
        },
    ];
    assert_eq!(
        selection::select(&Objective::MinimiseCost, &candidates, false).plan_id,
        Some("other".to_owned())
    );
}

/// An objective whose every weight is zero ranks nothing. Saying cost and
/// latency are "weighted together" would describe a trade-off nobody asked for,
/// and naming a missing measurement would blame the run for a job that asked
/// for no ordering.
#[test]
fn an_objective_with_no_weight_anywhere_says_that_and_not_something_else() {
    let candidates = vec![Candidate {
        plan_id: "exa-only".into(),
        completed: true,
        latency_ms: Some(10),
        observed_cost: Some(Money::new("USD", 7_000)),
        coverage: None,
        freshness: None,
    }];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 0.0,
            freshness: 0.0,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert!(selection.plan_id.is_none());
    assert!(
        selection.explanation.contains("carries weight"),
        "the explanation must name the real situation: {}",
        selection.explanation
    );
    assert!(
        !selection.explanation.contains("exchange rate"),
        "no trade-off is being made, so none may be described: {}",
        selection.explanation
    );
    assert!(
        selection.unavailable_inputs.is_empty(),
        "nothing is missing; every term is present and weighted zero"
    );
}

/// `{"kind": "maximise_quality"}` is a valid objective, so this abstention is
/// published. An evaluator does run — it measures whether the answer's
/// citations are supported — and the explanation must not contradict the same
/// run's `manifest.evaluators` by claiming no evaluator is configured.
#[test]
fn the_quality_abstention_names_the_evaluator_that_does_run() {
    let selection = selection::select(
        &Objective::MaximiseQuality,
        &[Candidate {
            plan_id: "exa-only".into(),
            completed: true,
            latency_ms: Some(10),
            observed_cost: None,
            coverage: None,
            freshness: None,
        }],
        false,
    );
    assert!(selection.plan_id.is_none());
    let explanation = selection.explanation.to_lowercase();
    assert!(
        explanation.contains("grounding"),
        "the installed evaluator must be named: {}",
        selection.explanation
    );
    assert!(
        !explanation.contains("no evaluator"),
        "an evaluator is installed; saying otherwise contradicts the manifest: {}",
        selection.explanation
    );
}

/// A measured candidate for the rankable arm: coverage against one named
/// rubric, freshness against the suite's as-of date, no answer required.
fn measured(plan_id: &str, coverage: f64, freshness: f64) -> Candidate {
    Candidate {
        plan_id: plan_id.into(),
        // The offline demonstration's shape: the window was bought, admitted
        // and measured, and no gateway answered.
        completed: false,
        latency_ms: Some(10),
        observed_cost: None,
        coverage: Some(selection::MeasuredCoverage {
            fraction: coverage,
            rubric: "eu-ai-act-cop/1".into(),
        }),
        freshness: Some(freshness),
    }
}

/// The abstention this package lifts, and only this one: a weighted objective
/// over coverage and freshness alone computes when every candidate carries
/// both measurements — arithmetic on declared weights, no answer needed —
/// and the explanation names the rubric the ranking leaned on, so a reader
/// knows whose definition of coverage chose the plan.
#[test]
fn a_weighted_coverage_and_freshness_objective_computes_from_measured_windows() {
    let candidates = vec![
        measured("exa-only", 4.0 / 6.0, 1.0),
        measured("firecrawl-only", 0.5, 1.0),
        measured("tavily-only", 0.5, 1.0),
    ];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 1.0,
            freshness: 0.5,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert_eq!(selection.plan_id, Some("exa-only".to_owned()));
    assert!(
        selection.method.contains("coverage") && selection.method.contains("freshness"),
        "the method must name the weighted terms: {}",
        selection.method
    );
    assert!(
        selection.explanation.contains("rubric eu-ai-act-cop/1"),
        "the explanation must name the rubric the ranking leaned on: {}",
        selection.explanation
    );
    assert!(selection.unavailable_inputs.is_empty());

    // Freshness alone is equally computable, and the freshest window wins.
    let candidates = vec![measured("stale", 0.9, 0.0), measured("fresh", 0.1, 1.0)];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 0.0,
            freshness: 1.0,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert_eq!(selection.plan_id, Some("fresh".to_owned()));
    assert!(
        !selection.explanation.contains("tie"),
        "an outright winner needs no tie-break, so none may be claimed: {}",
        selection.explanation
    );
}

/// Two candidates measuring the same top score are split by plan id — a
/// deterministic rule, not a measurement — and the record must say so. The
/// first live selection was decided by exactly this rule, with an
/// explanation that listed two equal figures and no reason for the winner.
#[test]
fn a_tied_top_score_names_the_tie_break_in_the_explanation() {
    let candidates = vec![
        measured("tavily-only", 0.17, 0.0),
        measured("exa-only", 0.17, 0.0),
    ];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 1.0,
            freshness: 0.0,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert_eq!(
        selection.plan_id,
        Some("exa-only".to_owned()),
        "the lexically first of the tied ids wins"
    );
    assert!(
        selection
            .explanation
            .contains("exa-only, tavily-only tie at 0.17"),
        "the tied plans and their score must be stated: {}",
        selection.explanation
    );
    assert!(
        selection
            .explanation
            .contains("first plan id in lexical order"),
        "the rule that decided the selection must be stated, not trusted: {}",
        selection.explanation
    );
}

/// Mixing a measured fraction with cost or latency still abstains: a fraction
/// has no price and no duration, and no measurement this run obtained is an
/// exchange rate. The same clause that refuses to weigh a currency against a
/// millisecond.
#[test]
fn a_measured_fraction_still_does_not_buy_a_cost_or_latency_trade() {
    let candidates = vec![
        measured("exa-only", 1.0, 1.0),
        measured("tavily-only", 0.5, 1.0),
    ];
    for (cost, latency, expected) in [
        (0.4, 0.0, vec!["fraction_cost_exchange_rate"]),
        (0.0, 0.2, vec!["fraction_latency_exchange_rate"]),
        (
            0.4,
            0.2,
            vec![
                "fraction_cost_exchange_rate",
                "fraction_latency_exchange_rate",
            ],
        ),
    ] {
        let selection = selection::select(
            &Objective::Weighted {
                quality: 0.0,
                coverage: 1.0,
                freshness: 0.0,
                cost,
                latency,
                policy_risk: 0.0,
            },
            &candidates,
            false,
        );
        assert!(
            selection.plan_id.is_none(),
            "cost {cost} / latency {latency} must abstain on the missing exchange rate"
        );
        assert_eq!(selection.unavailable_inputs, expected);
        assert!(
            selection.explanation.contains("no recorded exchange rate"),
            "{}",
            selection.explanation
        );
    }
}

/// Quality weighted non-zero still abstains, with the prose naming what is
/// now measured and what still is not — and only quality in the missing
/// list, because coverage and freshness are measured this run.
#[test]
fn quality_weight_still_abstains_even_beside_measured_coverage() {
    let candidates = vec![measured("exa-only", 1.0, 1.0)];
    let selection = selection::select(
        &Objective::Weighted {
            quality: 1.0,
            coverage: 1.0,
            freshness: 0.5,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert!(selection.plan_id.is_none());
    assert_eq!(selection.unavailable_inputs, vec!["quality".to_owned()]);
    assert!(
        selection.explanation.contains("needs a judge"),
        "the abstention must say what quality still lacks: {}",
        selection.explanation
    );
}

/// A candidate without the weighted measurement blocks the ranking: ranking
/// the others would silently favour the plans that were measurable, the same
/// discipline that keeps an undisclosed charge from losing a cost race by
/// default. The prose names the plan; `unavailable_inputs` keeps naming
/// measurements.
#[test]
fn one_unmeasured_candidate_blocks_a_fraction_ranking_and_is_named() {
    // The empty-window shape: coverage measured (it covered nothing),
    // freshness unmeasured (nothing to date).
    let mut tollbit = measured("tollbit-only", 0.0, 0.0);
    tollbit.freshness = None;
    let candidates = vec![measured("exa-only", 1.0, 1.0), tollbit];

    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 1.0,
            freshness: 0.5,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert!(selection.plan_id.is_none());
    assert_eq!(selection.unavailable_inputs, vec!["freshness".to_owned()]);
    assert!(
        selection.explanation.contains("tollbit-only")
            && !selection.explanation.contains("exa-only"),
        "the plan without the measurement is prose, and only that plan: {}",
        selection.explanation
    );

    // Weighted on coverage alone, the same candidates rank: the empty window
    // measurably covered nothing and loses rather than vanishing.
    let mut tollbit = measured("tollbit-only", 0.0, 0.0);
    tollbit.freshness = None;
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 1.0,
            freshness: 0.0,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &[measured("exa-only", 1.0, 1.0), tollbit],
        false,
    );
    assert_eq!(selection.plan_id, Some("exa-only".to_owned()));
}

/// Coverage fractions of different rubrics are not one scale. This cannot
/// arise from one suite — the run has one rubric — so it guards the caller
/// that mixes runs.
#[test]
fn fractions_measured_against_different_rubrics_do_not_rank_together() {
    let mut other = measured("other", 1.0, 1.0);
    other.coverage = Some(selection::MeasuredCoverage {
        fraction: 1.0,
        rubric: "another-rubric/2".into(),
    });
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 1.0,
            freshness: 0.0,
            cost: 0.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &[measured("exa-only", 0.5, 1.0), other],
        false,
    );
    assert!(selection.plan_id.is_none());
    assert!(
        selection.explanation.contains("different rubrics"),
        "{}",
        selection.explanation
    );
}

/// The measured-window candidates never leak into the arms that need an
/// answer: an unanswered plan has no comparable latency or cost leg, so
/// latency and cost objectives still see nothing to choose between.
#[test]
fn unanswered_measured_candidates_do_not_widen_the_cost_and_latency_arms() {
    let candidates = vec![
        measured("exa-only", 1.0, 1.0),
        measured("tavily-only", 0.5, 1.0),
    ];
    for objective in [Objective::MinimiseLatency, Objective::MinimiseCost] {
        let selection = selection::select(&objective, &candidates, false);
        assert!(
            selection.plan_id.is_none(),
            "{objective:?} ranked a plan that never ran to an answer"
        );
        assert!(
            selection.explanation.contains("No plan completed"),
            "{}",
            selection.explanation
        );
    }
    // And the weighted single-term deferrals stay equally closed.
    let selection = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 0.0,
            freshness: 0.0,
            cost: 1.0,
            latency: 0.0,
            policy_risk: 0.0,
        },
        &candidates,
        false,
    );
    assert!(selection.plan_id.is_none());
}

/// Under replay the latency arm abstains before it looks at a figure: a
/// replayed exchange's latency times the loopback origin serving the
/// recording, not the recorded provider, and it varies between replays of one
/// suite, so two replays of one suite can disagree on `selection/plan_id`
/// under "minimum observed latency". Cost stays rankable,
/// because a replayed charge is the recorded provider's own disclosure.
#[test]
fn a_replay_run_under_minimise_latency_abstains_naming_latency() {
    let candidates = vec![
        Candidate {
            plan_id: "exa-only".into(),
            completed: true,
            latency_ms: Some(10),
            observed_cost: Some(Money::new("USD", 7_000)),
            coverage: None,
            freshness: None,
        },
        Candidate {
            plan_id: "firecrawl-only".into(),
            completed: true,
            latency_ms: Some(20),
            observed_cost: Some(Money::new("USD", 1_000)),
            coverage: None,
            freshness: None,
        },
    ];
    let selection = selection::select(&Objective::MinimiseLatency, &candidates, true);
    assert!(selection.plan_id.is_none());
    assert_eq!(selection.method, "minimum observed latency");
    assert_eq!(selection.unavailable_inputs, vec!["latency".to_owned()]);
    assert!(
        selection.explanation.contains("loopback origin"),
        "the abstention must name what a replayed latency actually times: {}",
        selection.explanation
    );

    // The weighted deferral into the latency arm abstains the same way.
    let deferred = selection::select(
        &Objective::Weighted {
            quality: 0.0,
            coverage: 0.0,
            freshness: 0.0,
            cost: 0.0,
            latency: 1.0,
            policy_risk: 0.0,
        },
        &candidates,
        true,
    );
    assert!(deferred.plan_id.is_none());
    assert_eq!(deferred.unavailable_inputs, vec!["latency".to_owned()]);

    // And the same candidates under the same replay still rank on cost.
    assert_eq!(
        selection::select(&Objective::MinimiseCost, &candidates, true).plan_id,
        Some("firecrawl-only".to_owned())
    );
}

/// A quoted charge is not an observed one, and the distinction has to survive
/// into the record a reader sees.
#[test]
fn a_quoted_charge_stays_distinguishable_from_an_observed_one() {
    let quoted = NativeCharge {
        unit: "credits".into(),
        amount: serde_json::Number::from(1u64),
        basis: ChargeBasis::Quoted,
        note: Some("published price".into()),
    };
    let encoded = serde_json::to_value(&quoted).unwrap();
    assert_eq!(encoded["basis"], "quoted");
}

/// Supply held in memory, so a test can put exactly the text it needs in front
/// of the admission loop. Nothing else about the path changes: the same
/// processors, the same budget arithmetic, the same published plan.
struct HeldSupply {
    envelopes: Vec<ContextEnvelope>,
}

impl SupplyAdapter for HeldSupply {
    fn provider(&self) -> &str {
        "exa"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        &[ProviderCapability::Search]
    }

    fn search(
        &self,
        _query: &str,
        _limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        Ok(Acquisition {
            provider: "exa".to_owned(),
            capability: ProviderCapability::Search,
            endpoint: "https://api.exa.test/search".to_owned(),
            http_status: Some(200),
            latency_ms: 1,
            provider_request_id: None,
            charge: AcquisitionCharge::default(),
            envelopes: self.envelopes.clone(),
            raw_response: b"{\"results\":[]}".to_vec(),
            invocation: None,
        })
    }
}

fn text_envelope(url: &str, text: &str) -> ContextEnvelope {
    ContextEnvelope {
        source_url: url.into(),
        host: "held.example".into(),
        title: Some("A source".into()),
        text: Some(text.to_owned()),
        content_hash: Some("sha256:test".to_owned()),
        licence: LicenceState::Unknown,
        declared_date: None,
        native_metadata: serde_json::json!({}),
        retrieval_rank: 1,
    }
}

/// Run one plan over held supply and return its published record.
fn plan_over(envelopes: Vec<ContextEnvelope>, constraints: Vec<Constraint>) -> Value {
    let suite = Suite {
        suite_version: "test/v1".into(),
        label: "Admission reasons".into(),
        job: job(PolicyMode::Strict, constraints),
        model_plan: ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: envelopes.len() as u32,
        providers: vec!["exa".into()],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    };
    let directory = tempfile::tempdir().expect("tempdir");
    let held = Arc::new(envelopes);
    let report = execute(
        &suite,
        &RunOptions {
            allow_external_acquisition: true,
            output: directory.path().join("latest"),
            suppliers: Box::new(move |_provider| {
                Ok(Box::new(HeldSupply {
                    envelopes: held.as_ref().clone(),
                }) as Box<dyn SupplyAdapter>)
            }),
            // No gateway: the admission decisions are complete before inference
            // would be attempted, and this test is about those.
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        },
    )
    .expect("the run should complete");
    report.summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == "exa-only")
        .cloned()
        .expect("the exa plan")
}

/// First fit keeps offering the remaining sources, so a source that did not fit
/// may be followed by one that did. A reason claiming the budget was "already
/// spent" beside a later admitted source is two artefacts in one plan
/// contradicting each other; the gap already states the fact exactly, so the
/// reason is the gap.
#[test]
fn a_source_that_did_not_fit_is_not_reported_as_a_spent_budget() {
    let words = |prefix: &str, count: usize| {
        (0..count)
            .map(|index| format!("{prefix}{index:02}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let plan = plan_over(
        vec![
            text_envelope("https://held.example/a", &words("alpha", 25)),
            text_envelope("https://held.example/b", &words("bravo", 10)),
            text_envelope("https://held.example/c", &words("cee", 5)),
        ],
        vec![Constraint::MaximumContextTokens { tokens: 30 }],
    );

    let sources = plan["sources"].as_array().expect("sources");
    assert_eq!(sources[0]["admitted"], true);
    assert_eq!(sources[1]["admitted"], false);
    assert_eq!(
        sources[2]["admitted"], true,
        "first fit keeps offering, which is exactly why the reason must not \
         claim the budget was spent"
    );

    let reason = sources[1]["admission_reason"].as_str().expect("a reason");
    assert!(
        !reason.contains("already spent"),
        "the budget was not spent; 5 of 30 remained: {reason}"
    );
    let gap = plan["gaps"]
        .as_array()
        .expect("gaps")
        .iter()
        .find(|gap| gap["reason"] == "budget_exhausted")
        .expect("a budget gap");
    assert!(
        reason.contains(gap["detail"].as_str().expect("a detail")),
        "the reason must be the gap's own account, not a second one: {reason}"
    );
    assert!(
        reason.contains("only 5 of the job's 30 remained"),
        "and that account states what was actually left: {reason}"
    );
}

/// A source the optimiser empties span by span is a whole-source duplicate
/// arrived at slowly, and zero tokens fit any budget. Admitting it would put
/// the hash of the empty string under its URL in the model's context, and a
/// citation of that URL could then only ever be judged contradicted.
#[test]
fn a_source_deduplicated_to_nothing_is_not_admitted_as_an_empty_window_part() {
    let first = "alpha beta gamma delta epsilon zeta\n\neta theta iota kappa lambda mu";
    // The same two blocks in the other order: not a whole-source duplicate by
    // canonical text, but every word of it is already kept above.
    let second = "eta theta iota kappa lambda mu\n\nalpha beta gamma delta epsilon zeta";
    let plan = plan_over(
        vec![
            text_envelope("https://held.example/first", first),
            text_envelope("https://held.example/second", second),
        ],
        vec![],
    );

    let sources = plan["sources"].as_array().expect("sources");
    assert_eq!(sources[0]["admitted"], true);
    assert_eq!(
        sources[1]["admitted"], false,
        "a source with nothing left to contribute is not admitted"
    );
    assert_eq!(
        plan["source_count"], 1,
        "the empty source must not be counted as evidence"
    );
    let reason = sources[1]["admission_reason"].as_str().expect("a reason");
    assert!(
        reason.contains("context-optimiser") && reason.contains("https://held.example/first"),
        "the reason names the optimiser and the source that carries the text: {reason}"
    );
}

#[test]
fn supplier_source_permission_does_not_grant_direct_or_other_supplier_access() {
    let policy = job(
        PolicyMode::Strict,
        vec![
            Constraint::AllowedSourceHost {
                host: "allowed.example".into(),
            },
            Constraint::AllowedSourceProvider {
                provider: "ozone".into(),
            },
        ],
    );
    let mut source = envelope(
        "https://publisher.example/article",
        "publisher.example",
        Some("Public fixture."),
    );
    source.native_metadata = serde_json::json!({"provider":"ozone"});
    assert!(!policy::source_admission_from(&policy, &source, Some("ozone")).is_refusal());
    assert!(policy::source_admission_from(&policy, &source, Some("exa")).is_refusal());
    assert!(policy::source_admission(&policy, &source).is_refusal());
    assert_eq!(source.licence, LicenceState::Unknown);

    for restriction in [
        Constraint::DeniedSourceHost { host: "publisher.example".into() },
        Constraint::RequiredLicence { licence: "agreement-required".into() },
        Constraint::DeniedProvider { provider: "ozone".into() },
        Constraint::AllowedProvider { provider: "exa".into() },
        serde_json::from_value(serde_json::json!({"kind":"access_rule","host":"*","action":"refuse"})).unwrap(),
        serde_json::from_value(serde_json::json!({"kind":"access_rule","host":"*","action":"require_licence","licence":"agreement-required"})).unwrap(),
    ] {
        let mut restricted = policy.clone();
        restricted.constraints.push(restriction);
        assert!(policy::source_admission_from(&restricted, &source, Some("ozone")).is_refusal());
    }
    source.text = None;
    assert!(policy::source_admission_from(&policy, &source, Some("ozone")).is_refusal());
}
