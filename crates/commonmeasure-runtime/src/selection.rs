//! Route selection, and the discipline of abstaining.
//!
//! The router picks a plan only when the job's objective can be computed from
//! measurements this run actually obtained. Three evaluators measure here:
//! grounding verifies citations (an answer property no objective term names),
//! coverage measures the admitted window against a suite-declared rubric, and
//! freshness measures declared dates against a suite-declared as-of reference
//! — so a `Weighted` objective over coverage and freshness alone is
//! computable when every candidate carries both measurements. Everything else
//! keeps its abstention: quality needs a judge no run yet seals, and a
//! fraction has no exchange rate against a currency or a millisecond any more
//! than a millisecond has one against a currency. Breaking a tie on latency
//! because latency happens to be observable would publish a provider verdict
//! the run did not measure.

use commonmeasure_types::{Money, Objective};

/// One plan as the router sees it: identity plus the measurements that exist.
///
/// Cost and latency exist only for a plan that ran to an answer; coverage and
/// freshness exist as soon as the window was assembled and evaluated, which
/// is why they are carried independently of `completed` — a run with no
/// gateway still buys, admits and measures context, and an objective over
/// those measurements alone can rank it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub plan_id: String,
    /// Whether the plan ran to an answer. Cost and latency arms rank only
    /// completed plans, exactly as before coverage existed.
    pub completed: bool,
    pub latency_ms: Option<u64>,
    pub observed_cost: Option<Money>,
    /// The coverage evaluator's measured fraction for this plan's admitted
    /// window, with the rubric it was measured against. `None` when the
    /// record says unmeasured.
    pub coverage: Option<MeasuredCoverage>,
    /// The freshness evaluator's measured fraction. `None` when unmeasured.
    pub freshness: Option<f64>,
}

/// A coverage fraction never travels without its basis: the fraction is
/// agreement with a named rubric's author, and a selection that leaned on it
/// must say whose definition of coverage chose the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredCoverage {
    pub fraction: f64,
    /// The rubric identity, `name/version`.
    pub rubric: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub plan_id: Option<String>,
    pub method: String,
    pub explanation: String,
    /// Measurements the objective needs and this run does not have.
    pub unavailable_inputs: Vec<String>,
}

/// Apply the job's objective to the candidates.
///
/// `replayed` says this run was served from recordings. It reaches only the
/// latency arm: a replayed exchange's charge and content are the recorded
/// provider's own, but its latency is measured afresh around the loopback
/// origin serving the recording, so ranking on it would publish a provider
/// verdict the run did not measure — and one that varies between replays of
/// one suite, putting the headline output inside the declared-volatile set.
pub fn select(objective: &Objective, candidates: &[Candidate], replayed: bool) -> Selection {
    if candidates.is_empty() {
        return no_candidates();
    }

    match objective {
        Objective::MinimiseLatency => select_on_latency(candidates, replayed),
        Objective::MinimiseCost => select_on_cost(candidates),
        Objective::MaximiseQuality => abstain(
            "maximum measured quality",
            "Quality of the answer is a judgment, and every installed evaluator measures \
             something narrower: grounding measures citation support, and — where the \
             suite declares a rubric and an as-of date — coverage and freshness measure \
             the admitted window. None of those is a quality figure, and mapping one onto \
             quality would publish a verdict the run did not measure.",
            vec!["quality".to_owned()],
        ),
        Objective::Weighted {
            quality,
            coverage,
            freshness,
            cost,
            latency,
            policy_risk,
        } => {
            let mut missing = Vec::new();
            let mut absences: Vec<String> = Vec::new();
            if *quality != 0.0 {
                missing.push("quality".to_owned());
                absences.push(
                    "quality is a judgment that needs a judge, and this runtime seals none \
                     (the installed evaluators measure citation support, rubric coverage \
                     and declared-date freshness, none of which is quality)"
                        .to_owned(),
                );
            }
            if *coverage != 0.0 {
                let unmeasured = without(candidates, |candidate| candidate.coverage.is_some());
                if !unmeasured.is_empty() {
                    missing.push("coverage".to_owned());
                    absences.push(format!(
                        "{} carr{} no measured coverage (coverage is measured only \
                         against a suite-declared coverage_rubric, over an assembled \
                         window)",
                        unmeasured.join(", "),
                        crate::carries(unmeasured.len()),
                    ));
                }
            }
            if *freshness != 0.0 {
                let unmeasured = without(candidates, |candidate| candidate.freshness.is_some());
                if !unmeasured.is_empty() {
                    missing.push("freshness".to_owned());
                    absences.push(format!(
                        "{} carr{} no measured freshness (freshness is measured only \
                         against a suite-declared as_of date, over dated window parts)",
                        unmeasured.join(", "),
                        crate::carries(unmeasured.len()),
                    ));
                }
            }
            if *policy_risk != 0.0 {
                missing.push("policy_risk".to_owned());
                absences.push("policy risk has no measure beyond the recorded gaps".to_owned());
            }
            if !missing.is_empty() {
                return abstain(
                    "weighted utility",
                    &format!(
                        "The objective puts weight on measurements this run did not \
                         obtain: {}. Substituting the terms the run does have would \
                         publish a verdict it did not measure.",
                        absences.join("; ")
                    ),
                    missing,
                );
            }

            let fraction_terms = *coverage != 0.0 || *freshness != 0.0;
            let observed_terms = *cost != 0.0 || *latency != 0.0;
            match (fraction_terms, observed_terms) {
                // Every weighted term is measured, but the fractions and the
                // observations do not share a unit, and this runtime holds no
                // exchange rate to trade them — the same clause that refuses
                // to weigh a currency against a millisecond.
                (true, true) => {
                    let mut rates = Vec::new();
                    if *cost != 0.0 {
                        rates.push("fraction_cost_exchange_rate".to_owned());
                    }
                    if *latency != 0.0 {
                        rates.push("fraction_latency_exchange_rate".to_owned());
                    }
                    abstain(
                        "weighted utility",
                        "Coverage and freshness are measured this run as unitless \
                         fractions, and the objective also weights cost or latency; a \
                         fraction has no price and no duration, and this runtime holds \
                         no recorded exchange rate between them.",
                        rates,
                    )
                }
                (true, false) => rank_on_fractions(candidates, *coverage, *freshness),
                (false, true) => match (*cost != 0.0, *latency != 0.0) {
                    // Only cost and latency carry weight, and both are
                    // observable. Which one decides is still not this
                    // function's guess to make without a stated trade-off, so
                    // it defers to the single weighted term. Weights are
                    // non-negative by construction (`ContextJob::validate`
                    // refuses a negative), so deferring to a minimising arm
                    // cannot invert a declared preference.
                    (true, false) => select_on_cost(candidates),
                    (false, true) => select_on_latency(candidates, replayed),
                    (true, true) => abstain(
                        "weighted utility",
                        "Cost and latency are weighted together and this runtime has no \
                         recorded exchange rate between a currency and a millisecond.",
                        vec!["cost_latency_exchange_rate".to_owned()],
                    ),
                    (false, false) => unreachable!("observed_terms requires one non-zero"),
                },
                // Nothing is missing here: every term is present and every
                // one is weighted zero, so the objective ranks nothing.
                // Naming a missing measurement would blame the run for a
                // job that asked for no ordering.
                (false, false) => abstain(
                    "weighted utility",
                    "No term in this objective carries weight, so it expresses no preference \
                     between the completed plans.",
                    Vec::new(),
                ),
            }
        }
    }
}

/// The plans among `candidates` failing `has`, by id — for prose that names
/// which plans withheld a measurement, while `unavailable_inputs` keeps
/// naming measurements.
fn without(candidates: &[Candidate], has: impl Fn(&Candidate) -> bool) -> Vec<&str> {
    candidates
        .iter()
        .filter(|candidate| !has(candidate))
        .map(|candidate| candidate.plan_id.as_str())
        .collect()
}

/// Rank on the measured fractions alone: arithmetic on declared weights, no
/// unit conversion anywhere. Callable only once every candidate carries every
/// weighted fraction.
fn rank_on_fractions(candidates: &[Candidate], coverage: f64, freshness: f64) -> Selection {
    // One rubric, one scale. Every candidate's coverage was measured by this
    // run's one suite, so differing rubric identities mean the caller mixed
    // runs — and fractions of different rubrics are not one ordering.
    if coverage != 0.0 {
        let mut rubrics: Vec<&str> = candidates
            .iter()
            .filter_map(|candidate| candidate.coverage.as_ref())
            .map(|measured| measured.rubric.as_str())
            .collect();
        rubrics.sort_unstable();
        rubrics.dedup();
        if rubrics.len() > 1 {
            return abstain(
                "weighted utility",
                &format!(
                    "The candidates' coverage was measured against different rubrics \
                     ({}), and fractions of different rubrics are not one scale.",
                    rubrics.join(", ")
                ),
                vec!["coverage".to_owned()],
            );
        }
    }

    let mut terms = Vec::new();
    if coverage != 0.0 {
        terms.push("coverage");
    }
    if freshness != 0.0 {
        terms.push("freshness");
    }
    let method = format!(
        "maximum weighted utility over measured {}",
        terms.join(" and ")
    );

    let mut scored: Vec<(f64, &Candidate)> = candidates
        .iter()
        .map(|candidate| {
            let mut score = 0.0;
            if coverage != 0.0 {
                score += coverage
                    * candidate
                        .coverage
                        .as_ref()
                        .expect("rank_on_fractions requires measured coverage")
                        .fraction;
            }
            if freshness != 0.0 {
                score += freshness
                    * candidate
                        .freshness
                        .expect("rank_on_fractions requires measured freshness");
            }
            (score, candidate)
        })
        .collect();
    // Score descending; joint maxima resolve by plan id, the same
    // deterministic order `select_on_cost` takes over equal charges.
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .partial_cmp(left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.plan_id.cmp(&right.plan_id))
    });
    let (top_score, winner) = scored[0];
    // When that order decided the selection the record must say so: a reader
    // of two equal figures and a winner is otherwise left to trust an
    // unstated rule (the first live selection was decided by exactly this).
    let tied: Vec<&str> = scored
        .iter()
        .filter(|(score, _)| *score == top_score)
        .map(|(_, candidate)| candidate.plan_id.as_str())
        .collect();
    let tie = if tied.len() > 1 {
        format!(
            " {} tie at {top_score:.2}; the tie resolves to the first plan id in lexical \
             order, a deterministic rule rather than a measured preference.",
            tied.join(", "),
        )
    } else {
        String::new()
    };

    let basis = match (coverage != 0.0, freshness != 0.0) {
        (true, true) => format!(
            "coverage against rubric {} and freshness against the suite's as_of date",
            rubric_of(winner)
        ),
        (true, false) => format!("coverage against rubric {}", rubric_of(winner)),
        (false, _) => "freshness against the suite's as_of date".to_owned(),
    };
    let ranking = scored
        .iter()
        .map(|(score, candidate)| {
            let mut figures = Vec::new();
            if coverage != 0.0
                && let Some(measured) = &candidate.coverage
            {
                figures.push(format!("coverage {:.2}", measured.fraction));
            }
            if freshness != 0.0
                && let Some(fraction) = candidate.freshness
            {
                figures.push(format!("freshness {fraction:.2}"));
            }
            format!("{} {score:.2} ({})", candidate.plan_id, figures.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ");
    Selection {
        plan_id: Some(winner.plan_id.clone()),
        method,
        explanation: format!(
            "Every candidate's admitted window is measured this run — {basis} — and {} \
             unitless fraction{}, so the objective is arithmetic on the declared weights: \
             {ranking}. The ranking reads the evaluation records of the admitted windows, not \
             the answers.{tie}",
            // One weighted term or two: saying "both terms" over a
            // single-term objective describes an arithmetic that did not
            // happen, and the ranking below would then list one figure
            // against a sentence promising two.
            if coverage != 0.0 && freshness != 0.0 {
                "both terms are"
            } else {
                "that term is a"
            },
            if coverage != 0.0 && freshness != 0.0 {
                "s"
            } else {
                ""
            },
        ),
        unavailable_inputs: Vec::new(),
    }
}

fn rubric_of(candidate: &Candidate) -> &str {
    candidate
        .coverage
        .as_ref()
        .map(|measured| measured.rubric.as_str())
        .unwrap_or("unknown")
}

/// The latency arm, over the plans that ran to an answer: an uncompleted
/// plan's latency is a partial leg, not a comparable figure.
fn select_on_latency(candidates: &[Candidate], replayed: bool) -> Selection {
    // Before anything else, so that two replays of one suite abstain
    // identically whatever their loopback timings did. The replayed figure
    // times the origin serving the recording, not the recorded provider
    // (ARCHITECTURE §Optimisation model: substituting the terms a run does
    // have would publish a provider verdict it did not measure).
    if replayed {
        return abstain(
            "minimum observed latency",
            "A replayed exchange's latency times the loopback origin serving the \
             recording, not the recorded provider, so a replay run has no latency \
             measurement of the providers to rank on.",
            vec!["latency".to_owned()],
        );
    }
    let done: Vec<&Candidate> = candidates.iter().filter(|c| c.completed).collect();
    if done.is_empty() {
        return no_candidates();
    }
    let latencies: Option<Vec<(u64, String)>> = done
        .iter()
        .map(|candidate| {
            candidate
                .latency_ms
                .map(|ms| (ms, candidate.plan_id.clone()))
        })
        .collect();
    match latencies.and_then(|latencies| latencies.into_iter().min()) {
        Some((_, plan_id)) => Selection {
            plan_id: Some(plan_id),
            method: "minimum observed latency".to_owned(),
            explanation: "Latency is measured by this runtime around each exchange, so the \
                          objective is computable from observed evidence alone."
                .to_owned(),
            unavailable_inputs: Vec::new(),
        },
        None => abstain(
            "minimum observed latency",
            "At least one completed plan reported no latency.",
            vec!["latency".to_owned()],
        ),
    }
}

fn select_on_cost(candidates: &[Candidate]) -> Selection {
    let done: Vec<&Candidate> = candidates.iter().filter(|c| c.completed).collect();
    if done.is_empty() {
        return no_candidates();
    }
    let currencies: Vec<&str> = done
        .iter()
        .filter_map(|candidate| candidate.observed_cost.as_ref())
        .map(|money| money.currency())
        .collect();
    if currencies.len() != done.len() {
        // `unavailable_inputs` names measurements, not plans: a reader
        // comparing it against another arm's "latency" or "quality" would
        // otherwise be reading two vocabularies under one field name. Which
        // plans withheld the measurement is prose, and prose has a field.
        let silent: Vec<&str> = done
            .iter()
            .filter(|candidate| candidate.observed_cost.is_none())
            .map(|candidate| candidate.plan_id.as_str())
            .collect();
        return abstain(
            "minimum observed cost",
            &format!(
                "{} reported no charge, and an unreported charge is unknown rather than free.",
                silent.join(", ")
            ),
            vec!["observed_cost".to_owned()],
        );
    }
    if currencies.windows(2).any(|pair| pair[0] != pair[1]) {
        return abstain(
            "minimum observed cost",
            "Completed plans reported charges in different currencies and this runtime holds \
             no rate source.",
            vec!["currency_conversion".to_owned()],
        );
    }
    let cheapest = done
        .iter()
        .filter_map(|candidate| {
            candidate
                .observed_cost
                .as_ref()
                .map(|money| (money.micros, candidate.plan_id.clone()))
        })
        .min();
    match cheapest {
        Some((_, plan_id)) => Selection {
            plan_id: Some(plan_id),
            method: "minimum observed cost".to_owned(),
            explanation: "Every completed plan reported a charge in the same currency, so the \
                          objective is computable from observed evidence alone."
                .to_owned(),
            unavailable_inputs: Vec::new(),
        },
        None => abstain(
            "minimum observed cost",
            "No completed plan reported a charge.",
            vec!["observed_cost".to_owned()],
        ),
    }
}

/// Nothing this arm could rank: either no plan completed, or — for the
/// arms that need an answer — none of the candidates has one.
fn no_candidates() -> Selection {
    Selection {
        plan_id: None,
        method: "no-candidates".to_owned(),
        explanation: "No plan completed, so there was nothing to choose between.".to_owned(),
        unavailable_inputs: Vec::new(),
    }
}

fn abstain(method: &str, explanation: &str, unavailable_inputs: Vec<String>) -> Selection {
    Selection {
        plan_id: None,
        method: method.to_owned(),
        explanation: explanation.to_owned(),
        unavailable_inputs,
    }
}
