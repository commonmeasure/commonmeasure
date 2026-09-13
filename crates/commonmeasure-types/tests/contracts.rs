//! The domain's own invariants: what a job may not declare, what a plan may not
//! promise, and what a record may not omit.

use commonmeasure_types::{
    AssuranceBasis, Constraint, ContextJob, Decision, DecisionRecord, EvidenceError,
    EvidenceRequirement, Gap, GapReason, JobError, ModelPlan, Money, Objective, PlanError,
    PolicyMode, ProviderCapability, ProviderRef, SupplyPlan, SupplyStep,
};
use uuid::Uuid;

fn provider(name: &str, capabilities: Vec<ProviderCapability>) -> ProviderRef {
    ProviderRef {
        name: name.into(),
        adapter_version: "0.0.0-test".into(),
        capabilities,
    }
}

fn job(constraints: Vec<Constraint>) -> ContextJob {
    ContextJob {
        id: Uuid::new_v4(),
        kind: "research.answer".into(),
        prompt: "Which evidence supports the claim?".into(),
        policy_mode: PolicyMode::Strict,
        objective: Objective::MaximiseQuality,
        constraints,
        evidence_requirements: vec![EvidenceRequirement {
            description: "A primary source".into(),
            required: true,
        }],
    }
}

#[test]
fn context_job_round_trips_without_erasing_policy() {
    let job = job(vec![
        Constraint::AllowedProvider {
            provider: "exa".into(),
        },
        Constraint::MaximumAcquisitionCost {
            amount: Money::new("usd", 10_000),
        },
    ]);

    job.validate().unwrap();
    let encoded = serde_json::to_string(&job).unwrap();
    let decoded: ContextJob = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, job);
}

#[test]
fn conflicting_provider_policy_is_rejected() {
    let job = job(vec![
        Constraint::AllowedProvider {
            provider: "exa".into(),
        },
        Constraint::DeniedProvider {
            provider: "exa".into(),
        },
    ]);

    assert_eq!(
        job.validate(),
        Err(JobError::ConflictingProviderPolicy("exa".into()))
    );
}

/// Two caps in different currencies have no defensible reading, and picking one
/// at execution time would make the run unexplainable.
#[test]
fn budgets_in_two_currencies_are_rejected() {
    let job = job(vec![
        Constraint::MaximumAcquisitionCost {
            amount: Money::new("USD", 50_000),
        },
        Constraint::MaximumAcquisitionCost {
            amount: Money::new("GBP", 40_000),
        },
    ]);
    assert_eq!(job.validate(), Err(JobError::MixedBudgetCurrencies));
}

/// Nothing in the runtime gives a negative weight a meaning: the fraction
/// arm's arithmetic honours the sign while the single-term cost and latency
/// arms defer to their minimising selectors regardless of it — two readings
/// of one field. Refused at validation, before anything runs.
#[test]
fn a_negative_objective_weight_is_rejected() {
    let mut job = job(vec![]);
    job.objective = Objective::Weighted {
        quality: 0.0,
        coverage: 1.0,
        freshness: 0.0,
        cost: -0.4,
        latency: 0.0,
        policy_risk: 0.0,
    };
    assert_eq!(
        job.validate(),
        Err(JobError::NegativeObjectiveWeight("cost"))
    );
}

#[test]
fn supply_steps_require_declared_capabilities() {
    let plan = SupplyPlan {
        name: "exa-search".into(),
        version: "1".into(),
        steps: vec![SupplyStep {
            provider: provider("exa", vec![ProviderCapability::Fetch]),
            capability: ProviderCapability::Search,
            limit: 5,
        }],
        maximum_cost: None,
    };

    assert_eq!(
        plan.validate(),
        Err(PlanError::MissingCapability {
            provider: "exa".into(),
            capability: ProviderCapability::Search,
        })
    );
}

#[test]
fn model_plan_requires_an_explicit_model() {
    let plan = ModelPlan {
        name: "fixed".into(),
        version: "1".into(),
        model: " ".into(),
    };
    assert_eq!(plan.validate(), Err(PlanError::EmptyModel));
}

/// A model plan governs the crossing through its model name and nothing else.
/// A provider pin, a fallback list or a per-model cap would be sealed into the
/// manifest as an operative control while the gateway request carried no trace
/// of it, so the job file is refused instead of quietly ignored.
#[test]
fn a_model_plan_declaring_a_control_the_runtime_lacks_is_a_load_error() {
    for extra in [
        r#""provider":"openai""#,
        r#""fallbacks":["fallback-model"]"#,
        r#""maximum_cost":{"currency":"USD","micros":1}"#,
    ] {
        let encoded = format!(r#"{{"name":"m","version":"1","model":"x",{extra}}}"#);
        assert!(
            serde_json::from_str::<ModelPlan>(&encoded).is_err(),
            "accepted an inert control: {encoded}"
        );
    }
    serde_json::from_str::<ModelPlan>(r#"{"name":"m","version":"1","model":"x"}"#).unwrap();
}

#[test]
fn refusal_requires_an_explicit_gap() {
    let record = DecisionRecord {
        id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        decision: Decision::Refuse,
        reason: "source policy".into(),
        assurance: AssuranceBasis::Observed,
        plan: Some("firecrawl-only".into()),
        provider: Some("firecrawl".into()),
        source_url: Some("https://example.invalid/commentary".into()),
        model: None,
        observed_cost: None,
        gaps: vec![],
    };

    assert_eq!(record.validate(), Err(EvidenceError::RefusalWithoutGap));

    let record = DecisionRecord {
        gaps: vec![Gap::new(GapReason::PolicyRefused, "host not allowed")],
        ..record
    };
    record.validate().unwrap();
}

/// A provider's decimal charge must survive the trip into the domain exactly.
/// `0.007` through an `f64` and back is the kind of drift that turns an
/// observed charge into an approximately observed one.
#[test]
fn decimal_charges_are_parsed_exactly() {
    assert_eq!(
        Money::from_decimal_str("USD", "0.007"),
        Some(Money::new("USD", 7_000))
    );
    assert_eq!(
        Money::from_decimal_str("USD", "12"),
        Some(Money::new("USD", 12_000_000))
    );
    assert_eq!(
        Money::from_decimal_str("USD", "0.007")
            .unwrap()
            .as_decimal_string(),
        "0.007000"
    );
    // More precision than micro-units can hold is refused rather than rounded:
    // an amount this runtime cannot represent exactly stays unknown.
    assert_eq!(Money::from_decimal_str("USD", "0.0000001"), None);
    assert_eq!(Money::from_decimal_str("USD", "-1"), None);
    assert_eq!(Money::from_decimal_str("USD", "1.2e3"), None);
}

/// The upper-case invariant holds on every construction path, deserialisation
/// included. A cap typed `"usd"` in a job file otherwise compares unequal to
/// an adapter's `"USD"` charge, and an incomparable cap is recorded as
/// unenforceable — a declared budget silently reduced to commentary by case.
#[test]
fn a_lower_case_currency_normalises_on_deserialisation() {
    let decoded: Money = serde_json::from_str(r#"{"currency":"usd","micros":50000}"#).unwrap();
    assert_eq!(decoded, Money::new("USD", 50_000));
    assert_eq!(
        decoded.compare(&Money::new("USD", 7_000)),
        Some(std::cmp::Ordering::Greater)
    );
}

/// A misspelled key is a load error, never a quieter job. Every policy list
/// defaults to empty, so `"contraints"` would otherwise deserialise into a
/// job with no policy at all — and validate.
#[test]
fn unknown_keys_in_jobs_and_plans_are_load_errors() {
    let mut encoded = serde_json::to_value(job(vec![Constraint::DeniedProvider {
        provider: "exa".into(),
    }]))
    .unwrap();
    let constraints = encoded["constraints"].take();
    encoded["contraints"] = constraints;
    encoded.as_object_mut().unwrap().remove("constraints");
    assert!(serde_json::from_value::<ContextJob>(encoded).is_err());

    assert!(
        serde_json::from_str::<SupplyPlan>(
            r#"{"name":"p","version":"1","steps":[],"maximum_cots":null}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<ModelPlan>(r#"{"name":"m","version":"1","model":"x","suffix":"x"}"#)
            .is_err()
    );
}

/// The guard has to reach the types a job nests, or it kills only the typos an
/// operator was least likely to make. `requird` inside a requirement, or
/// `dollars` inside a cap, would otherwise load as a default and validate.
///
/// A `Constraint` cannot carry the guard — serde refuses it on an internally
/// tagged enum — but every variant's payload is mandatory, so the same typo
/// fails there as a missing field.
#[test]
fn unknown_keys_inside_nested_types_are_load_errors() {
    let mut encoded = serde_json::to_value(job(vec![])).unwrap();
    encoded["evidence_requirements"] =
        serde_json::json!([{"description": "primary source", "requird": true}]);
    assert!(serde_json::from_value::<ContextJob>(encoded).is_err());

    assert!(
        serde_json::from_str::<Money>(r#"{"currency":"USD","micros":10,"dollars":99}"#).is_err()
    );

    assert!(
        serde_json::from_str::<Constraint>(r#"{"kind":"maximum_context_tokens","tokns":4000}"#)
            .is_err()
    );
}

/// The three scalar caps are read by first declaration, so a second one is not
/// a tighter policy — it is a policy the run drops from enforcement and from
/// the record without saying so. The list constraints are read whole and are
/// meant to repeat.
#[test]
fn a_second_scalar_cap_is_rejected_rather_than_dropped() {
    assert_eq!(
        job(vec![
            Constraint::MaximumAcquisitionCost {
                amount: Money::new("USD", 1_000_000),
            },
            Constraint::MaximumAcquisitionCost {
                amount: Money::new("USD", 50_000),
            },
        ])
        .validate(),
        Err(JobError::DuplicateConstraint("maximum_acquisition_cost"))
    );
    assert_eq!(
        job(vec![
            Constraint::MaximumContextTokens { tokens: 4_000 },
            Constraint::MaximumContextTokens { tokens: 100 },
        ])
        .validate(),
        Err(JobError::DuplicateConstraint("maximum_context_tokens"))
    );
    assert_eq!(
        job(vec![
            Constraint::MaximumLatencyMs {
                milliseconds: 5_000
            },
            Constraint::MaximumLatencyMs { milliseconds: 10 },
        ])
        .validate(),
        Err(JobError::DuplicateConstraint("maximum_latency_ms"))
    );

    job(vec![
        Constraint::AllowedSourceHost {
            host: "www.ofgem.gov.uk".into(),
        },
        Constraint::AllowedSourceHost {
            host: "www.gov.uk".into(),
        },
        Constraint::MaximumContextTokens { tokens: 4_000 },
    ])
    .validate()
    .unwrap();
}

/// Amounts in different currencies are not comparable without a rate source
/// this runtime does not have, so the answer is "cannot say", never a guess.
#[test]
fn cross_currency_comparison_refuses_rather_than_guesses() {
    let usd = Money::new("USD", 10_000);
    let gbp = Money::new("GBP", 1);
    assert_eq!(usd.compare(&gbp), None);
    assert_eq!(usd.try_add(&gbp), None);
    assert_eq!(
        usd.compare(&Money::new("USD", 20_000)),
        Some(std::cmp::Ordering::Less)
    );
}

/// The three pattern forms and nothing else: an exact host, a wildcard that
/// covers a domain and its apex, and the pattern for every host. A source
/// with no host matches none of them.
#[test]
fn a_host_pattern_matches_exactly_by_domain_or_everywhere() {
    use commonmeasure_types::HostPattern;
    let exact = HostPattern::parse("Docs.Example.COM.").unwrap();
    assert_eq!(exact.as_written(), "docs.example.com");
    assert!(exact.matches("docs.example.com"));
    assert!(!exact.matches("api.example.com"));
    assert!(!exact.matches("example.com"));

    let domain = HostPattern::parse("*.example.com").unwrap();
    assert!(domain.matches("example.com"));
    assert!(domain.matches("docs.example.com"));
    assert!(domain.matches("a.b.example.com"));
    assert!(!domain.matches("notexample.com"));
    assert!(!domain.matches("example.com.evil.example"));

    let every = HostPattern::parse("*").unwrap();
    assert!(every.matches("anything.example"));
    assert!(!every.matches(""), "a hostless source is about no host");
    assert!(!exact.matches(""));
    assert!(!domain.matches(""));

    assert!(HostPattern::parse("").is_err());
    assert!(HostPattern::parse("   ").is_err());
    assert!(HostPattern::parse("a.*.example").is_err());
    assert!(HostPattern::parse("*.").is_err());
    assert!(HostPattern::parse("*.*").is_err());
}

/// The access rule is written as one flat object, round-trips as written,
/// and a pattern that names no host fails while the job is being read rather
/// than at the first crossing.
#[test]
fn an_access_rule_round_trips_and_a_bad_pattern_fails_at_read() {
    use commonmeasure_types::AccessAction;
    let written = r#"[
        {"kind":"access_rule","host":"docs.example.com","action":"allow"},
        {"kind":"access_rule","host":"*.example.com","action":"refuse"},
        {"kind":"access_rule","host":"publisher.example","action":"require_licence","licence":"rsl:p/1"},
        {"kind":"access_rule","host":"*","action":"require_mediation"}
    ]"#;
    let rules: Vec<Constraint> = serde_json::from_str(written).unwrap();
    let ruled = job(rules.clone());
    assert_eq!(ruled.validate(), Ok(()));
    let (index, pattern, action) = ruled.access_rule_for("api.example.com").unwrap();
    assert_eq!((index, pattern.as_written().as_str()), (1, "*.example.com"));
    assert_eq!(action, &AccessAction::Refuse);
    assert_eq!(
        ruled.access_rule_for("docs.example.com").unwrap().2,
        &AccessAction::Allow,
        "the earlier rule wins"
    );
    assert_eq!(
        ruled.access_rule_for("publisher.example").unwrap().2,
        &AccessAction::RequireLicence {
            licence: "rsl:p/1".into()
        }
    );
    assert_eq!(
        ruled.access_rule_for("other.example").unwrap().2,
        &AccessAction::RequireMediation
    );
    assert_eq!(ruled.access_rule_for(""), None);

    let encoded = serde_json::to_value(&rules).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!([
            {"kind":"access_rule","host":"docs.example.com","action":"allow"},
            {"kind":"access_rule","host":"*.example.com","action":"refuse"},
            {"kind":"access_rule","host":"publisher.example","action":"require_licence","licence":"rsl:p/1"},
            {"kind":"access_rule","host":"*","action":"require_mediation"}
        ])
    );

    let error = serde_json::from_str::<Vec<Constraint>>(
        r#"[{"kind":"access_rule","host":"","action":"refuse"}]"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cannot be empty"), "{error}");

    let nameless = job(vec![Constraint::AccessRule {
        host: commonmeasure_types::HostPattern::parse("example.com").unwrap(),
        action: AccessAction::RequireLicence { licence: "".into() },
    }]);
    assert!(matches!(
        nameless.validate(),
        Err(JobError::InvalidAccessRule(0, _))
    ));
}

/// A pattern and a plain host entry are normalised by one function, so the
/// cases below hold on both sides of every comparison admission makes.
#[test]
fn a_pattern_and_a_host_entry_are_normalised_alike() {
    use commonmeasure_types::{HostPattern, normalised_host};
    for (written, expected) in [
        ("Docs.Example.COM.", "docs.example.com"),
        ("  www.gov.uk ", "www.gov.uk"),
        ("bänned.example", "xn--bnned-gra.example"),
        ("EXAMPLE.com", "example.com"),
    ] {
        assert_eq!(normalised_host(written), expected, "{written}");
        assert_eq!(
            HostPattern::parse(written).unwrap().as_written(),
            expected,
            "{written}"
        );
        assert!(HostPattern::parse(written).unwrap().matches(expected));
    }
    // An entry that names no host: the plain normaliser keeps it, lowercased,
    // so a denial is not lost to a typo; a pattern refuses it.
    assert_eq!(normalised_host("not a host"), "not a host");
    assert!(HostPattern::parse("not a host").is_err());
}
