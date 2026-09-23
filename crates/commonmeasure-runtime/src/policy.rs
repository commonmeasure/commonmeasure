//! Deterministic eligibility and admission.
//!
//! Everything here is a pure function of the job's declared policy and what was
//! observed. No score, no learned weight, no tie-break invented at execution
//! time: deterministic policy comes before learned optimisation, because a
//! decision a human cannot re-derive from the artefacts is not evidence.
//!
//! The three policy modes differ in one thing only — what happens to a breach.
//! `Strict` refuses it, `Prefer` and `Observe` let it through with the breach
//! recorded. No mode hides it.

use commonmeasure_types::{
    AccessAction, AcquisitionCharge, ContextEnvelope, ContextJob, Gap, GapReason, LicenceState,
    PolicyMode,
};

/// The outcome of one policy check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ruling {
    /// No constraint applies, or every applicable one is satisfied.
    Allowed,
    /// A constraint was breached and the mode refuses it.
    Refused { reason: String, gap: Gap },
    /// A constraint was breached and the mode carries on. The breach is
    /// recorded either way; the difference is whether the content is used.
    AllowedWithBreach { reason: String, gap: Gap },
}

impl Ruling {
    pub fn is_refusal(&self) -> bool {
        matches!(self, Ruling::Refused { .. })
    }

    pub fn gap(&self) -> Option<&Gap> {
        match self {
            Ruling::Allowed => None,
            Ruling::Refused { gap, .. } | Ruling::AllowedWithBreach { gap, .. } => Some(gap),
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Ruling::Allowed => None,
            Ruling::Refused { reason, .. } | Ruling::AllowedWithBreach { reason, .. } => {
                Some(reason)
            }
        }
    }

    /// The single place mode discipline lives: `Strict` refuses a breach,
    /// `Prefer` and `Observe` carry it with the breach recorded. Processors
    /// route their verdicts through here rather than re-implementing the
    /// match.
    pub(crate) fn breach(mode: PolicyMode, reason: String, gap: Gap) -> Self {
        match mode {
            PolicyMode::Strict => Ruling::Refused { reason, gap },
            PolicyMode::Prefer | PolicyMode::Observe => Ruling::AllowedWithBreach { reason, gap },
        }
    }
}

/// May this provider be used at all? Checked before any request is sent, so a
/// denied provider never sees the job's prompt.
pub fn provider_eligibility(job: &ContextJob, provider: &str) -> Ruling {
    if job.denied_providers().any(|denied| denied == provider) {
        return Ruling::breach(
            job.policy_mode,
            format!("The job denies provider {provider}."),
            Gap::new(
                GapReason::PolicyRefused,
                format!("Provider {provider} is on the job's denied list, so no request was sent."),
            ),
        );
    }
    let mut allowed = job.allowed_providers().peekable();
    if allowed.peek().is_some() && !job.allowed_providers().any(|name| name == provider) {
        return Ruling::breach(
            job.policy_mode,
            format!("The job's allowed-provider list does not include {provider}."),
            Gap::new(
                GapReason::PolicyRefused,
                format!(
                    "Provider {provider} is outside the job's allowed-provider list, so no \
                     request was sent."
                ),
            ),
        );
    }
    Ruling::Allowed
}

/// May this source's bytes enter the context window?
///
/// Host policy first, then rights, in a fixed order: the denied-host list,
/// then the ordered access rules, then the allowed-host list, then the
/// required licences. The denied-host list is a set and comes first because a
/// denial is absolute: no access rule can allow past it. An access rule that
/// matches ends the host check: `allow` passes the host, `refuse` is a
/// breach, `require_licence` is judged on the licence the supplier declared
/// and `require_mediation` is met by arriving here at all, because admission
/// is the governed crossing the rule demands. No matching rule leaves the
/// host to the allowed-host list as before. A source that carries no text is
/// refused whatever the mode: it cannot ground an answer, and admitting it
/// would put a citation in the record with nothing behind it.
pub fn source_admission(job: &ContextJob, envelope: &ContextEnvelope) -> Ruling {
    source_admission_from(job, envelope, None)
}

/// Admit supply from the adapter that actually acquired it. `provider` must
/// come from the dispatch path, never a URL or supplier-native metadata.
/// Direct fetches and observed sources pass `None`, so they cannot inherit
/// a supplier-specific exception to the allowed-host list.
pub fn source_admission_from(
    job: &ContextJob,
    envelope: &ContextEnvelope,
    provider: Option<&str>,
) -> Ruling {
    let provider_ruling = provider
        .map(|name| provider_eligibility(job, name))
        .unwrap_or(Ruling::Allowed);
    if provider_ruling.is_refusal() {
        return provider_ruling;
    }
    if envelope.text.is_none() {
        return Ruling::Refused {
            reason: "The provider returned no excerpt for this result.".to_owned(),
            gap: Gap::new(
                GapReason::EvidenceMissing,
                format!(
                    "{} was discovered but carried no text, so it could not ground anything.",
                    envelope.source_url
                ),
            ),
        };
    }
    if job
        .denied_source_hosts()
        .any(|denied| normalised_host(denied) == envelope.host)
    {
        return Ruling::breach(
            job.policy_mode,
            format!("The job denies host {}.", envelope.host),
            Gap::new(
                GapReason::PolicyRefused,
                format!("{} is on the job's denied-host list.", envelope.source_url),
            ),
        );
    }
    let mut host_allowed_by_rule = false;
    if let Some((index, pattern, action)) = job.access_rule_for(&envelope.host) {
        // Positions are stated one-based, as an operator counts entries in
        // the file, and the pattern is stated as written so the reader can
        // find the rule without counting.
        let rule = format!("access rule {} ({pattern})", index + 1);
        match action {
            AccessAction::Allow | AccessAction::RequireMediation => {
                host_allowed_by_rule = true;
            }
            AccessAction::Refuse => {
                return Ruling::breach(
                    job.policy_mode,
                    format!("{rule} refuses host {}.", envelope.host),
                    Gap::new(
                        GapReason::PolicyRefused,
                        format!("{} is refused by {rule}.", envelope.source_url),
                    ),
                );
            }
            AccessAction::RequireLicence { licence } => match &envelope.licence {
                LicenceState::Declared { reference } if reference == licence => {
                    host_allowed_by_rule = true;
                }
                LicenceState::Declared { reference } => {
                    return Ruling::breach(
                        job.policy_mode,
                        format!(
                            "{rule} requires licence {licence:?} for host {}, and the supplier \
                             declared {reference:?}.",
                            envelope.host
                        ),
                        Gap::new(
                            GapReason::PolicyRefused,
                            format!(
                                "{} was declared under licence {reference:?}, not the \
                                 {licence:?} that {rule} requires.",
                                envelope.source_url
                            ),
                        ),
                    );
                }
                LicenceState::Unknown => {
                    return Ruling::breach(
                        job.policy_mode,
                        format!(
                            "{rule} requires licence {licence:?} for host {}, and no supplier \
                             declared one.",
                            envelope.host
                        ),
                        Gap::new(
                            GapReason::EvidenceMissing,
                            format!(
                                "{} arrived with no machine-readable licence, and absent licence \
                                 evidence is unknown rather than the {licence:?} that {rule} \
                                 requires.",
                                envelope.source_url
                            ),
                        ),
                    );
                }
            },
        }
    }
    let mut allowed = job.allowed_source_hosts().peekable();
    if !host_allowed_by_rule
        && !provider.is_some_and(|name| job.allows_provider_sources(name))
        && allowed.peek().is_some()
        && !job
            .allowed_source_hosts()
            .any(|host| normalised_host(host) == envelope.host)
    {
        // An internal-corpus document has no host at all. An allow-list is a
        // containment control — only the sources it names may be used — and a
        // source it cannot name is outside it, so the refusal stands and says
        // which case it is rather than printing an empty host.
        let (reason, detail) = if envelope.host.is_empty() {
            (
                "This source has no host, and the job's allowed-host list admits only the hosts \
                 it names."
                    .to_owned(),
                format!(
                    "{} has no host to match against the job's allowed-host list, so it is not a \
                     permitted source under this job's source policy.",
                    envelope.source_url
                ),
            )
        } else {
            (
                format!(
                    "Host {} is outside the job's allowed-host list.",
                    envelope.host
                ),
                format!(
                    "{} is not a permitted source under this job's source policy.",
                    envelope.source_url
                ),
            )
        };
        return Ruling::breach(
            job.policy_mode,
            reason,
            Gap::new(GapReason::PolicyRefused, detail),
        );
    }
    let mut required = job.required_licences().peekable();
    if required.peek().is_some() {
        match &envelope.licence {
            LicenceState::Declared { reference }
                if job.required_licences().any(|wanted| wanted == reference) => {}
            _ => {
                return Ruling::breach(
                    job.policy_mode,
                    "No supplier declared a licence this job accepts.".to_owned(),
                    Gap::new(
                        GapReason::EvidenceMissing,
                        format!(
                            "{} arrived with no machine-readable licence, and absent licence \
                             evidence is unknown rather than permitted.",
                            envelope.source_url
                        ),
                    ),
                );
            }
        }
    }
    provider_ruling
}

/// The one host normaliser, defined beside the vocabulary so a pattern and a
/// denied-host entry cannot spell "the same host" two ways. Re-exported here
/// because admission and the console's deny-list editor read it from the
/// policy engine.
pub use commonmeasure_types::normalised_host;

/// Was the acquisition inside the job's spend cap?
///
/// A cap can only be enforced against a charge in a comparable currency. Two
/// different things stop that, and the record has to tell them apart: the
/// provider disclosed nothing, or it disclosed a figure this runtime could not
/// express at micro precision. Neither is resolved by treating the charge as
/// zero — which would let the one provider that discloses nothing pass every
/// budget — and neither is honestly described by saying the provider was silent.
pub fn acquisition_cost(job: &ContextJob, charge: &AcquisitionCharge) -> Ruling {
    let Some(cap) = job.maximum_acquisition_cost() else {
        return Ruling::Allowed;
    };
    let Some(observed) = charge.money.as_ref() else {
        let (reason, detail) = match charge.native.as_ref() {
            Some(native) => (
                format!(
                    "The provider disclosed {} {}, which this runtime could not express at micro \
                     precision, so the acquisition cost cap could not be enforced.",
                    native.amount, native.unit
                ),
                format!(
                    "{} {} was disclosed but no comparable amount was derived{}, so the cap of {} \
                     {} was not enforceable for this plan.",
                    native.amount,
                    native.unit,
                    native
                        .note
                        .as_deref()
                        .map(|note| format!(" ({note})"))
                        .unwrap_or_default(),
                    cap.as_decimal_string(),
                    cap.currency()
                ),
            ),
            None => (
                "The provider disclosed no charge, so the acquisition cost cap could not be \
                 enforced."
                    .to_owned(),
                format!(
                    "No charge was disclosed, so the cap of {} {} was not enforceable for this \
                     plan.",
                    cap.as_decimal_string(),
                    cap.currency()
                ),
            ),
        };
        return Ruling::AllowedWithBreach {
            reason,
            gap: Gap::new(GapReason::EvidenceMissing, detail),
        };
    };
    match observed.compare(cap) {
        Some(std::cmp::Ordering::Greater) => Ruling::breach(
            job.policy_mode,
            format!(
                "The observed charge {} {} exceeds the cap of {} {}.",
                observed.as_decimal_string(),
                observed.currency(),
                cap.as_decimal_string(),
                cap.currency()
            ),
            Gap::new(
                GapReason::BudgetExhausted,
                "The acquired content was refused admission because its charge exceeded the \
                 job's acquisition cap."
                    .to_owned(),
            ),
        ),
        Some(_) => Ruling::Allowed,
        None => Ruling::AllowedWithBreach {
            reason: format!(
                "The charge is in {} and the cap is in {}; this runtime holds no rate source.",
                observed.currency(),
                cap.currency()
            ),
            gap: Gap::new(
                GapReason::EvidenceMissing,
                "The acquisition cost cap could not be enforced across currencies.".to_owned(),
            ),
        },
    }
}

/// May this quoted purchase proceed?
///
/// The one decision quote-then-buy adds. It runs *before* money moves —
/// unlike [`acquisition_cost`], which judges a charge already paid — so this
/// is the point where strict mode can actually prevent a spend rather than
/// refuse admission after it. That changes what an unverifiable price means:
/// post-hoc it is a gap on a done deed, but pre-purchase "buy at a price the
/// cap cannot be checked against" is itself the decision, so under a
/// declared cap it is a breach — strict declines the purchase, observe and
/// prefer buy with the breach recorded.
///
/// Trial coverage is the exception: the supplier states no charge applies to
/// this purchase (`billing_status: "trial"`, with its billing note in the
/// quote evidence), which is verifiable coverage, not an unknown price.
pub fn purchase_decision(
    job: &ContextJob,
    quoted: &AcquisitionCharge,
    trial_covered: bool,
) -> Ruling {
    let Some(cap) = job.maximum_acquisition_cost() else {
        return Ruling::Allowed;
    };
    if trial_covered {
        return Ruling::Allowed;
    }
    let Some(price) = quoted.money.as_ref() else {
        let quoted_as = match quoted.native.as_ref() {
            Some(native) => format!("{} {}", native.amount, native.unit),
            None => "no price at all".to_owned(),
        };
        return Ruling::breach(
            job.policy_mode,
            format!(
                "The job caps acquisition cost at {} {} and the quote is {quoted_as}, which \
                 the cap cannot be checked against; buying at an unverifiable price is \
                 declined rather than discovered on the receipt.",
                cap.as_decimal_string(),
                cap.currency()
            ),
            Gap::new(
                GapReason::EvidenceMissing,
                "The quoted price could not be verified against the job's acquisition cost \
                 cap before purchase.",
            ),
        );
    };
    match price.compare(cap) {
        Some(std::cmp::Ordering::Greater) => Ruling::breach(
            job.policy_mode,
            format!(
                "The quoted price {} {} exceeds the cap of {} {}.",
                price.as_decimal_string(),
                price.currency(),
                cap.as_decimal_string(),
                cap.currency()
            ),
            Gap::new(
                GapReason::BudgetExhausted,
                "The quote exceeded the job's acquisition cap, so the purchase was declined \
                 and nothing was bought.",
            ),
        ),
        Some(_) => Ruling::Allowed,
        None => Ruling::breach(
            job.policy_mode,
            format!(
                "The quote is in {} and the cap is in {}; this runtime holds no rate source, \
                 so the cap cannot be checked before purchase.",
                price.currency(),
                cap.currency()
            ),
            Gap::new(
                GapReason::EvidenceMissing,
                "The quoted price and the acquisition cost cap are in different currencies.",
            ),
        ),
    }
}

/// Did the plan stay inside the job's latency cap?
///
/// Latency is observed after the fact, so this cannot pre-empt a slow provider.
/// It can stop a result that arrived too late from being used, which is what a
/// declared hard limit means.
pub fn total_latency(job: &ContextJob, observed_ms: u64) -> Ruling {
    let Some(cap) = job.maximum_latency_ms() else {
        return Ruling::Allowed;
    };
    if observed_ms <= cap {
        return Ruling::Allowed;
    }
    Ruling::breach(
        job.policy_mode,
        format!("The plan took {observed_ms} ms against a cap of {cap} ms."),
        Gap::new(
            GapReason::PolicyRefused,
            format!("The plan exceeded the job's {cap} ms latency limit."),
        ),
    )
}

/// The token footprint of a piece of text.
///
/// Whitespace words, not a model tokeniser. It is an approximation and every
/// artefact that carries a token figure names this basis, because a number
/// labelled "tokens" that no tokeniser produced is exactly the kind of
/// borrowed precision this product exists to refuse.
pub const TOKEN_BASIS: &str = "whitespace-words";

pub fn approximate_tokens(text: &str) -> u64 {
    text.split_whitespace().count() as u64
}

/// Which sources fit inside the context budget, in the order they are
/// offered, each named by reference with the token footprint it would
/// actually contribute — after any transform-stage optimisation, since that
/// is what would enter the window.
///
/// Sources are admitted whole or not at all. A truncated source has a content
/// hash that matches nothing, and a citation into text the model never saw is
/// worse than a missing source.
pub fn fit_context_budget<'a>(
    sources: impl IntoIterator<Item = (&'a str, u64)>,
    budget_tokens: Option<u64>,
) -> Vec<BudgetOutcome> {
    let mut admitted_tokens = 0u64;
    sources
        .into_iter()
        .map(|(reference, tokens)| {
            let Some(budget) = budget_tokens else {
                admitted_tokens += tokens;
                return BudgetOutcome {
                    tokens,
                    admitted: true,
                    gap: None,
                };
            };
            if admitted_tokens + tokens <= budget {
                admitted_tokens += tokens;
                BudgetOutcome {
                    tokens,
                    admitted: true,
                    gap: None,
                }
            } else {
                BudgetOutcome {
                    tokens,
                    admitted: false,
                    gap: Some(Gap::new(
                        GapReason::BudgetExhausted,
                        format!(
                            "{reference} needs {tokens} {TOKEN_BASIS} and only {} of the job's \
                             {budget} remained, so it was left out whole rather than truncated.",
                            budget.saturating_sub(admitted_tokens)
                        ),
                    )),
                }
            }
        })
        .collect()
}

/// One source's fate against the context budget, in the order the sources were
/// offered.
#[derive(Debug)]
pub struct BudgetOutcome {
    pub tokens: u64,
    pub admitted: bool,
    pub gap: Option<Gap>,
}
