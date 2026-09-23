//! The suite-declared governance block: the support-status rule set and the
//! entitlement grant a governed run admits sources under.
//!
//! Rules are data the admission path evaluates, never prose a model
//! interprets — and they are *suite* input, not corpus data, for one load-
//! bearing reason: the run manifest seals the suite whole but names nothing
//! inside the corpus, so a rule written into `corpus.json` could change
//! without moving any manifest hash. Declared here and sealed beside
//! `coverage_rubric`, flipping one rule's status or effective date is a
//! different experiment by hash — which is exactly the traceability the
//! revocation demonstration rests on (`demo/specialist/README.md`).
//!
//! What the rules mean at admission — matching, precedence, what refuses —
//! is the support processor's business (`crate::processor::support`); this
//! module owns the declaration and its validation.

use serde::{Deserialize, Serialize};

use crate::freshness::parse_date;

/// A versioned rule set plus the entitlement the run is granted. Optional on
/// a suite; a suite that declares one is a governed experiment, and one that
/// does not records no governance invocation at all.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Governance {
    pub name: String,
    pub version: String,
    /// Entitlement tiers in ascending order of privilege. The order is
    /// declared rather than assumed, because "exceeds the grant" needs an
    /// ordering and tier names are the operator's vocabulary, not ours.
    pub entitlement_tiers: Vec<String>,
    /// The tier this run holds. Sources declaring a higher tier are refused
    /// at admission.
    pub granted_entitlement: String,
    pub rules: Vec<SupportRule>,
}

/// One deterministic support-status rule: for this edition, version range
/// and integration path, this status holds from this date.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupportRule {
    pub edition: String,
    pub version_range: String,
    pub integration_path: String,
    pub support_status: String,
    /// ISO calendar date the status takes effect, compared against the
    /// suite's `as_of` — suite input, never a clock.
    pub effective_date: String,
}

/// The statuses a rule may declare. Closed, because a status the admission
/// path does not understand would silently govern nothing.
const SUPPORT_STATUSES: [&str; 3] = ["supported", "deprecated", "experimental"];

impl Governance {
    /// Reject a declaration the admission path could not evaluate before
    /// anything runs — the same discipline as `CoverageRubric::validate`.
    /// Every refusal record names the rule set's identity, so a nameless or
    /// unversioned one could never be traced; a grant outside the declared
    /// tiers could never be compared against any source; and two rules under
    /// one (edition, version range, integration path) key would leave which
    /// one decided unknowable.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(
                "a governance block needs a name and a version; every refusal \
                        must name the rule set that decided it"
                    .to_owned(),
            );
        }
        if self.entitlement_tiers.is_empty() {
            return Err(format!(
                "governance {} declares no entitlement tiers, so no grant could ever be \
                 compared against a source's declared tier",
                self.identity()
            ));
        }
        let mut tiers: Vec<&str> = Vec::new();
        for tier in &self.entitlement_tiers {
            if tier.trim().is_empty() {
                return Err(format!(
                    "governance {} declares an empty entitlement tier",
                    self.identity()
                ));
            }
            if tiers.contains(&tier.as_str()) {
                return Err(format!(
                    "governance {} declares entitlement tier {tier} twice; a tier named \
                     twice has no single rank",
                    self.identity()
                ));
            }
            tiers.push(tier);
        }
        if !tiers.contains(&self.granted_entitlement.as_str()) {
            return Err(format!(
                "governance {} grants entitlement {:?}, which is not one of its declared \
                 tiers, so no source's tier could be compared against it",
                self.identity(),
                self.granted_entitlement
            ));
        }
        if self.rules.is_empty() {
            return Err(format!(
                "governance {} declares no rules, so there would be nothing to govern with",
                self.identity()
            ));
        }
        let mut keys: Vec<(&str, &str, &str)> = Vec::new();
        for rule in &self.rules {
            if rule.edition.trim().is_empty()
                || rule.version_range.trim().is_empty()
                || rule.integration_path.trim().is_empty()
            {
                return Err(format!(
                    "governance {} has a rule with an empty edition, version range or \
                     integration path; a rule that names nothing can match nothing",
                    self.identity()
                ));
            }
            let key = (
                rule.edition.as_str(),
                rule.version_range.as_str(),
                rule.integration_path.as_str(),
            );
            if keys.contains(&key) {
                return Err(format!(
                    "governance {} declares two rules for ({}, {}, {}); which one decided \
                     an admission could never be known",
                    self.identity(),
                    rule.edition,
                    rule.version_range,
                    rule.integration_path
                ));
            }
            keys.push(key);
            if !SUPPORT_STATUSES.contains(&rule.support_status.as_str()) {
                return Err(format!(
                    "governance {} rule ({}, {}, {}) declares support status {:?}, which \
                     the admission path does not evaluate; it must be one of {}",
                    self.identity(),
                    rule.edition,
                    rule.version_range,
                    rule.integration_path,
                    rule.support_status,
                    SUPPORT_STATUSES.join(", ")
                ));
            }
            if parse_date(&rule.effective_date).is_none() {
                return Err(format!(
                    "governance {} rule ({}, {}, {}) declares effective date {:?}, which \
                     does not parse as an ISO calendar date, so when it takes effect \
                     could never be measured",
                    self.identity(),
                    rule.edition,
                    rule.version_range,
                    rule.integration_path,
                    rule.effective_date
                ));
            }
        }
        Ok(())
    }

    /// The identity every refusal and invocation record names: whose rules
    /// decided the admission.
    pub fn identity(&self) -> String {
        format!("{}/{}", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn governed() -> Governance {
        serde_json::from_value(serde_json::json!({
            "name": "fictive-support-rules",
            "version": "1",
            "entitlement_tiers": ["standard", "premier"],
            "granted_entitlement": "standard",
            "rules": [{
                "edition": "orchestrator",
                "version_range": "4.x",
                "integration_path": "flux-connector",
                "support_status": "deprecated",
                "effective_date": "2026-03-01"
            }]
        }))
        .expect("a well-formed governance block deserialises")
    }

    #[test]
    fn a_well_formed_declaration_validates() {
        assert_eq!(governed().validate(), Ok(()));
    }

    #[test]
    fn a_grant_outside_the_declared_tiers_is_rejected() {
        let mut governance = governed();
        governance.granted_entitlement = "platinum".to_owned();
        let error = governance.validate().expect_err("must reject");
        assert!(error.contains("platinum"), "{error}");
    }

    #[test]
    fn duplicate_rule_keys_are_rejected() {
        let mut governance = governed();
        governance.rules.push(governance.rules[0].clone());
        let error = governance.validate().expect_err("must reject");
        assert!(error.contains("two rules"), "{error}");
    }

    #[test]
    fn an_unevaluable_support_status_is_rejected() {
        let mut governance = governed();
        governance.rules[0].support_status = "retired".to_owned();
        let error = governance.validate().expect_err("must reject");
        assert!(error.contains("retired"), "{error}");
    }

    #[test]
    fn an_unparseable_effective_date_is_rejected() {
        let mut governance = governed();
        governance.rules[0].effective_date = "March 2026".to_owned();
        let error = governance.validate().expect_err("must reject");
        assert!(error.contains("March 2026"), "{error}");
    }

    #[test]
    fn an_empty_rule_list_is_rejected() {
        let mut governance = governed();
        governance.rules.clear();
        assert!(governance.validate().is_err());
    }

    #[test]
    fn a_misspelled_field_is_a_load_error() {
        let result: Result<Governance, _> = serde_json::from_value(serde_json::json!({
            "name": "r", "version": "1",
            "entitlement_tiers": ["standard"],
            "granted_entitlement": "standard",
            "ruels": []
        }));
        assert!(result.is_err(), "unknown fields must not be ignored");
    }
}
