use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Money;

/// How hard the operator wants policy enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    /// Record what happened; refuse nothing.
    Observe,
    /// Prefer eligible routes, but let an ineligible one through with the
    /// breach recorded.
    Prefer,
    /// Refuse an ineligible crossing before context reaches the model.
    Strict,
}

/// What the operator is optimising for. `Weighted` is the explicit utility of
/// `ARCHITECTURE.md`; its weights belong to the operator and are recorded in
/// the run manifest so a selection can be re-derived rather than trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Objective {
    MaximiseQuality,
    MinimiseCost,
    MinimiseLatency,
    Weighted {
        quality: f64,
        coverage: f64,
        freshness: f64,
        cost: f64,
        latency: f64,
        policy_risk: f64,
    },
}

/// A hard constraint. Hard constraints filter before anything is scored.
///
/// Every kind but one has set semantics: the list of denied hosts is a set,
/// and its order means nothing. `access_rule` is the exception. Access rules
/// are read in declaration order and the first whose pattern matches the
/// source's host decides; a later rule for the same host is never reached.
/// Order among access rules is therefore part of the policy, and an editor
/// that reorders them changes what is enforced.
///
/// An internally tagged enum cannot carry `deny_unknown_fields`, but every
/// variant's payload is mandatory, so a misspelled key already fails as the
/// mandatory one gone missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    AllowedProvider {
        provider: String,
    },
    DeniedProvider {
        provider: String,
    },
    /// Sources delivered by this adapter may pass the allowed-host list.
    /// Explicit host denials, ordered access rules and licences still apply.
    AllowedSourceProvider {
        provider: String,
    },
    AllowedSourceHost {
        host: String,
    },
    DeniedSourceHost {
        host: String,
    },
    RequiredLicence {
        licence: String,
    },
    MaximumAcquisitionCost {
        amount: Money,
    },
    MaximumContextTokens {
        tokens: u64,
    },
    MaximumLatencyMs {
        milliseconds: u64,
    },
    /// One ordered access rule: what happens to a source whose host matches
    /// `host`. Written as `{"kind": "access_rule", "host": "*.example.com",
    /// "action": "refuse"}`; a `require_licence` action also names the
    /// `licence` it requires.
    AccessRule {
        host: HostPattern,
        #[serde(flatten)]
        action: AccessAction,
    },
}

/// What an access rule does to the sources its pattern matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AccessAction {
    /// The host passes host policy: no later access rule and no allowed-host
    /// list is consulted for it. The denied-host list has already been
    /// applied by then, and a required licence still applies, because a
    /// licence is evidence about the content and not a fact about the host.
    Allow,
    /// The source is a breach: refused under `strict`, carried with the
    /// breach recorded under `observe` and `prefer`.
    Refuse,
    /// The source is admitted only when the supplier declared exactly this
    /// licence. An unknown licence is unknown, never permitted.
    RequireLicence { licence: String },
    /// Accepted by the loader and met at admission, because every crossing
    /// that reaches admission is a crossing Common Measure governed. No other
    /// path reads this rule: an observed crossing is not checked against it.
    RequireMediation,
}

impl AccessAction {
    /// The word an evidence record and a console use for this action.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Refuse => "refuse",
            Self::RequireLicence { .. } => "require_licence",
            Self::RequireMediation => "require_mediation",
        }
    }
}

/// The host part of an access rule: which hosts the rule is about.
///
/// Three forms, and nothing else parses. An exact host (`docs.example.com`)
/// matches that host alone. A wildcard host (`*.example.com`) matches
/// `example.com` and every host beneath it, because an operator refusing a
/// domain means the whole domain and a rule that spared the apex would
/// surprise them. `*` alone matches every source that has a host. A source
/// with no host, such as a document from the operator's own corpus, matches
/// no pattern: there is nothing for the rule to be about.
///
/// The host part is kept in the normalised form admission compares against
/// (lowercase, IDNA-encoded, no trailing dot), so a pattern the operator
/// wrote as `Docs.Example.COM.` names the host the record calls
/// `docs.example.com`. An empty or unparseable pattern is refused when it is
/// read, which is what makes the loader the place a bad rule is caught.
/// The schema form is the string an operator writes, because that is what
/// `try_from` parses; the two fields below are the parsed result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct HostPattern {
    /// The normalised host, or the normalised suffix for a wildcard; empty
    /// for the pattern that matches every host.
    host: String,
    wildcard: bool,
}

impl HostPattern {
    /// Parse a pattern as an operator wrote it.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        let written = pattern.trim();
        if written.is_empty() {
            return Err("an access rule's host pattern cannot be empty".to_owned());
        }
        if written == "*" {
            return Ok(Self {
                host: String::new(),
                wildcard: true,
            });
        }
        let (wildcard, host) = match written.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, written),
        };
        if host.contains('*') {
            return Err(format!(
                "access rule host pattern {written:?} is not a host, \"*.host\" or \"*\"; a wildcard \
                 stands only at the front"
            ));
        }
        let normalised = parsed_host(host)
            .ok_or_else(|| format!("access rule host pattern {written:?} does not name a host"))?;
        Ok(Self {
            host: normalised,
            wildcard,
        })
    }

    /// Whether this pattern is about `host`, which must already be in the
    /// normalised form the envelope carries.
    pub fn matches(&self, host: &str) -> bool {
        if host.is_empty() {
            return false;
        }
        if !self.wildcard {
            return host == self.host;
        }
        if self.host.is_empty() {
            return true;
        }
        host == self.host
            || host
                .strip_suffix(&self.host)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }

    /// The pattern as an operator would write it back.
    pub fn as_written(&self) -> String {
        match (self.wildcard, self.host.is_empty()) {
            (true, true) => "*".to_owned(),
            (true, false) => format!("*.{}", self.host),
            (false, _) => self.host.clone(),
        }
    }
}

/// The operator's spelling of a host, reduced to the form an envelope's host
/// arrives in: `Url::host_str` of `https://{entry}/`, lowercased,
/// IDNA-encoded, trailing dots removed. Both sides of every host
/// comparison, in admission and in a pattern, go through this one function,
/// so "the same host" has one definition. An entry the parser rejects is
/// compared as written, lowercased, because losing a denial to a typo is the
/// worse failure; a pattern, which must name a host, refuses such an entry
/// instead ([`HostPattern::parse`]).
pub fn normalised_host(entry: &str) -> String {
    parsed_host(entry).unwrap_or_else(|| entry.trim().to_lowercase())
}

/// `url` with a domain host in [`normalised_host`]'s form, for comparing
/// whole URLs or URL prefixes. For http and https the parser has already
/// lowered the host's case, IDNA-encoded it and dropped the scheme's default
/// port; this also removes a fully qualified name's trailing dots, because
/// `https://example.com./a` is the resource `https://example.com/a` is. It
/// clears a username and password: credentials authenticate a request and
/// do not name a different resource, so `https://u:p@example.com/a` is
/// `https://example.com/a` too. An address host is left as parsed. The
/// scheme, any other port, the path and the query are kept, since each names
/// a different resource.
pub fn canonical_url(url: &url::Url) -> url::Url {
    let mut canonical = url.clone();
    // Both setters refuse only a URL that cannot carry credentials, which
    // then has none to clear.
    let _ = canonical.set_username("");
    let _ = canonical.set_password(None);
    if let Some(url::Host::Domain(domain)) = url.host() {
        let host = normalised_host(domain);
        // A host the setter refuses leaves the URL as parsed.
        if host != domain && !host.is_empty() {
            let _ = canonical.set_host(Some(&host));
        }
    }
    canonical
}

/// The host `entry` names, when it names one.
fn parsed_host(entry: &str) -> Option<String> {
    let host = url::Url::parse(&format!("https://{}/", entry.trim()))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))?;
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

impl TryFrom<String> for HostPattern {
    type Error = String;

    fn try_from(pattern: String) -> Result<Self, Self::Error> {
        Self::parse(&pattern)
    }
}

impl From<HostPattern> for String {
    fn from(pattern: HostPattern) -> Self {
        pattern.as_written()
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_written())
    }
}

/// Evidence the job says it needs. Whether it was obtained is a run outcome,
/// never an assumption.
///
/// `required` is declared, not enforced: it is projected into the run record
/// and shown to the reader, and no refusal, filter or verdict turns on it. It
/// says what the operator asked for, not what the runtime guaranteed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequirement {
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// The unit of work. A job declares the task, the policy and the outcome
/// contract; it never names provider endpoints or wire details.
///
/// Unknown fields are load errors. Every list here defaults to empty, so a
/// misspelled `"contraints"` would otherwise deserialise into a job with no
/// policy at all and pass validation — enforcement silently absent because of
/// a typo nobody was told about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextJob {
    pub id: Uuid,
    pub kind: String,
    pub prompt: String,
    pub policy_mode: PolicyMode,
    pub objective: Objective,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub evidence_requirements: Vec<EvidenceRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    EmptyKind,
    EmptyPrompt,
    ConflictingProviderPolicy(String),
    ConflictingSourcePolicy(String),
    MixedBudgetCurrencies,
    DuplicateConstraint(&'static str),
    NegativeObjectiveWeight(&'static str),
    /// An access rule that could never be met as written: the rule's index
    /// among the constraints and the reason.
    InvalidAccessRule(usize, String),
}

impl ContextJob {
    /// Reject a job whose policy contradicts itself before it can be
    /// half-enforced. A provider that is both allowed and denied, or two
    /// budgets in different currencies, has no defensible reading, and
    /// choosing one at execution time would make the record unexplainable.
    ///
    /// A second cap, token ceiling or latency ceiling is refused for the same
    /// reason: each is read by first declaration, so a tighter one written
    /// later would be dropped from enforcement and from the record without
    /// anyone being told. The list constraints — providers, hosts, licences —
    /// are read whole and are meant to repeat.
    ///
    /// A negative objective weight is refused because nothing in the runtime
    /// gives one a meaning: the fraction arm's arithmetic would rank the
    /// least-covered window first while the single-term cost and latency
    /// arms defer to their minimising selectors whatever the sign — two
    /// readings of one field, neither of which a record could explain.
    pub fn validate(&self) -> Result<(), JobError> {
        if self.kind.trim().is_empty() {
            return Err(JobError::EmptyKind);
        }
        if self.prompt.trim().is_empty() {
            return Err(JobError::EmptyPrompt);
        }
        if let Objective::Weighted {
            quality,
            coverage,
            freshness,
            cost,
            latency,
            policy_risk,
        } = &self.objective
        {
            for (name, weight) in [
                ("quality", quality),
                ("coverage", coverage),
                ("freshness", freshness),
                ("cost", cost),
                ("latency", latency),
                ("policy_risk", policy_risk),
            ] {
                if *weight < 0.0 {
                    return Err(JobError::NegativeObjectiveWeight(name));
                }
            }
        }

        let mut budget_currency: Option<&str> = None;
        let mut declared: Vec<&'static str> = Vec::new();
        for (index, constraint) in self.constraints.iter().enumerate() {
            if let Constraint::AccessRule { action, .. } = constraint
                && let Err(reason) = action.validate()
            {
                return Err(JobError::InvalidAccessRule(index, reason));
            }
            if let Constraint::MaximumAcquisitionCost { amount } = constraint {
                match budget_currency {
                    Some(currency) if currency != amount.currency() => {
                        return Err(JobError::MixedBudgetCurrencies);
                    }
                    None => budget_currency = Some(amount.currency()),
                    _ => {}
                }
            }
            let scalar = match constraint {
                Constraint::MaximumAcquisitionCost { .. } => "maximum_acquisition_cost",
                Constraint::MaximumContextTokens { .. } => "maximum_context_tokens",
                Constraint::MaximumLatencyMs { .. } => "maximum_latency_ms",
                _ => continue,
            };
            if declared.contains(&scalar) {
                return Err(JobError::DuplicateConstraint(scalar));
            }
            declared.push(scalar);
        }

        if let Some(provider) = self
            .allowed_providers()
            .find(|candidate| self.denied_providers().any(|denied| denied == *candidate))
        {
            return Err(JobError::ConflictingProviderPolicy(provider.to_owned()));
        }
        if let Some(host) = self.allowed_source_hosts().find(|candidate| {
            self.denied_source_hosts()
                .any(|denied| denied == *candidate)
        }) {
            return Err(JobError::ConflictingSourcePolicy(host.to_owned()));
        }
        Ok(())
    }

    pub fn allowed_providers(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::AllowedProvider { provider } => Some(provider.as_str()),
            _ => None,
        })
    }

    pub fn denied_providers(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::DeniedProvider { provider } => Some(provider.as_str()),
            _ => None,
        })
    }

    /// Whether this trusted adapter's sources may pass the allowed-host list.
    /// This does not authorise dispatch to a provider otherwise excluded.
    pub fn allows_provider_sources(&self, provider: &str) -> bool {
        self.constraints.iter().any(|constraint| {
            matches!(constraint, Constraint::AllowedSourceProvider { provider: allowed } if allowed == provider)
        }) && !self.denied_providers().any(|denied| denied == provider)
            && (self.allowed_providers().next().is_none()
                || self.allowed_providers().any(|allowed| allowed == provider))
    }

    pub fn allowed_source_hosts(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::AllowedSourceHost { host } => Some(host.as_str()),
            _ => None,
        })
    }

    pub fn denied_source_hosts(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::DeniedSourceHost { host } => Some(host.as_str()),
            _ => None,
        })
    }

    pub fn maximum_acquisition_cost(&self) -> Option<&Money> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumAcquisitionCost { amount } => Some(amount),
            _ => None,
        })
    }

    pub fn maximum_context_tokens(&self) -> Option<u64> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumContextTokens { tokens } => Some(*tokens),
            _ => None,
        })
    }

    pub fn maximum_latency_ms(&self) -> Option<u64> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumLatencyMs { milliseconds } => Some(*milliseconds),
            _ => None,
        })
    }

    pub fn required_licences(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::RequiredLicence { licence } => Some(licence.as_str()),
            _ => None,
        })
    }

    /// The access rules in declaration order, each with its position among
    /// the constraints so a ruling can name the rule it applied.
    pub fn access_rules(&self) -> impl Iterator<Item = (usize, &HostPattern, &AccessAction)> {
        self.constraints
            .iter()
            .enumerate()
            .filter_map(|(index, c)| match c {
                Constraint::AccessRule { host, action } => Some((index, host, action)),
                _ => None,
            })
    }

    /// The first access rule whose pattern matches `host`, which must be in
    /// the normalised form the envelope carries. First match wins; nothing
    /// after it is read.
    pub fn access_rule_for(&self, host: &str) -> Option<(usize, &HostPattern, &AccessAction)> {
        self.access_rules()
            .find(|(_, pattern, _)| pattern.matches(host))
    }
}

impl AccessAction {
    /// Whether the action can be met as written. A `require_licence` naming
    /// no licence could match no declaration and would refuse every source
    /// under it for a reason the operator never wrote.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::RequireLicence { licence } if licence.trim().is_empty() => {
                Err("a require_licence access rule must name the licence it requires".to_owned())
            }
            _ => Ok(()),
        }
    }
}
