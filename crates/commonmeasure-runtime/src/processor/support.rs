//! The support-status governor: a deterministic `admit`-stage processor.
//!
//! The suite's declared rule set (`crate::governance`) evaluated against
//! each source's operator-declared metadata, before that source can reach a
//! model. The verdict is evidence, not deletion — the same mode discipline
//! as the PII detector: in `strict` a breach refuses the crossing; in
//! `observe` and `prefer` the breach is recorded identically and the
//! crossing carries on (`docs/FAIL-POLICY.md`, clause 6).
//!
//! This processor runs only when the suite declares `governance`. An
//! ungoverned suite records no invocation at all, on the grounding
//! evaluator's opt-in precedent: absence of a record and a record of
//! absence are different facts.

use chrono::Utc;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{Decision, Gap, GapReason, PolicyMode};
use serde_json::{Value, json};

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use crate::freshness::parse_date;
use crate::governance::{Governance, SupportRule};
use crate::policy::Ruling;

pub const NAME: &str = "support-governor";
pub const VERSION: &str = "1";

/// The evaluation semantics, stated canonically. The manifest's configuration
/// digest covers exactly this text: the governor's behaviour cannot change
/// without the digest changing. The rule set itself is deliberately not part
/// of this digest — it is suite input, sealed by the run manifest, so a rule
/// change is a different experiment by hash while the processor identity
/// stands still (the coverage evaluator's convention).
const RULES: &str = "\
support-governor/1: evaluated only when the suite declares governance, over each source \
the prior admission checks would let cross.\n\
metadata: a source's operator-declared governance metadata (edition, version_range, \
integration_path, support_status, entitlement — all plain strings) is read from the \
envelope's native metadata. A source with no declared metadata is refused: under a \
governed suite, unknown is not supported. Metadata missing any of the five fields is \
refused as unreadable rather than part-evaluated.\n\
entitlement: the suite declares its tiers in ascending order of privilege and the tier \
the run holds. A source declaring a tier absent from that order is refused (an unknown \
tier has no rank); a source declaring a tier ranked above the grant is refused.\n\
rules: a rule matches a source when edition, version_range and integration_path are \
each equal, compared exactly and case-sensitively. Rule keys are unique by validation, \
so at most one rule matches. A source no rule matches is refused: the rule set does \
not speak for that path, and the governed run does not exceed its rule set's \
authority.\n\
status: a matching rule whose support_status is deprecated refuses the source when the \
rule's effective_date is on or before the suite's as_of date; a deprecation with an \
effective_date after as_of is not yet in force and the source is admitted with the \
pending date recorded. supported and experimental admit. The rule's status decides; \
the document's declared support_status is corroboration, and where the two disagree \
the disagreement is recorded as a note on the invocation, never a verdict.\n\
verdicts pass through the shared mode discipline: strict refuses a breach, observe and \
prefer record it and carry on.\n";

static MANIFEST: std::sync::LazyLock<ProcessorManifest> =
    std::sync::LazyLock::new(|| ProcessorManifest {
        name: NAME,
        version: VERSION,
        stage: Stage::Admit,
        capability: "support-status-governance",
        implementation: IN_PROCESS,
        configuration_digest: sha256_digest(RULES.as_bytes()),
        permissions: NO_AMBIENT_AUTHORITY,
        determinism: Determinism::Deterministic,
        limits: "in-process and synchronous; one pass over the declared metadata, no \
                 supervisor timeout",
        failure_by_mode: FailureByMode {
            strict: FailBehaviour::FailClosed,
            prefer: FailBehaviour::FailOpen,
            observe: FailBehaviour::FailOpen,
        },
        evidence_format: INVOCATION_VERSION,
    });

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "Rules match the operator's declared metadata; nothing verifies that a document's \
     prose agrees with its declaration, so a document may describe the very path its \
     metadata disowns, or disown the one it describes.",
    "Effective dates are measured against the suite's declared as_of, never a clock; \
     a stale as_of governs by a stale calendar and this processor cannot tell.",
    "Only sources this runtime carries are governed; an observed crossing has already \
     happened and is recorded, not governed.",
];

/// The five declared fields, read from the envelope's native metadata.
struct Declared {
    edition: String,
    version_range: String,
    integration_path: String,
    support_status: String,
    entitlement: String,
}

fn declared_from(metadata: &Value) -> Option<Declared> {
    let field = |name: &str| metadata.get(name)?.as_str().map(str::to_owned);
    Some(Declared {
        edition: field("edition")?,
        version_range: field("version_range")?,
        integration_path: field("integration_path")?,
        support_status: field("support_status")?,
        entitlement: field("entitlement")?,
    })
}

/// The verdict the rule evaluation reaches, before mode discipline is
/// applied. `refusal` explains a breach; `note` carries a fact worth
/// recording about an admitted source.
struct Evaluation {
    refusal: Option<String>,
    matched_rule: Option<Value>,
    note: Option<String>,
}

fn evaluate(
    declared: Option<&Declared>,
    governance: &Governance,
    as_of_date: &str,
    source_ref: &str,
) -> Evaluation {
    let Some(declared) = declared else {
        return Evaluation {
            refusal: Some(format!(
                "{source_ref} declares no governance metadata, and under governed rule set \
                 {} unknown is not supported.",
                governance.identity()
            )),
            matched_rule: None,
            note: None,
        };
    };
    let rank = |tier: &str| {
        governance
            .entitlement_tiers
            .iter()
            .position(|declared| declared == tier)
    };
    let Some(declared_rank) = rank(&declared.entitlement) else {
        return Evaluation {
            refusal: Some(format!(
                "{source_ref} declares entitlement tier {:?}, which rule set {} does not \
                 rank, so the grant cannot be compared against it.",
                declared.entitlement,
                governance.identity()
            )),
            matched_rule: None,
            note: None,
        };
    };
    let granted_rank = rank(&governance.granted_entitlement)
        .expect("validation requires the grant to be a declared tier");
    if declared_rank > granted_rank {
        return Evaluation {
            refusal: Some(format!(
                "{source_ref} requires entitlement tier {:?} and this run is granted {:?} \
                 under rule set {}.",
                declared.entitlement,
                governance.granted_entitlement,
                governance.identity()
            )),
            matched_rule: None,
            note: None,
        };
    }
    let matched: Option<&SupportRule> = governance.rules.iter().find(|rule| {
        rule.edition == declared.edition
            && rule.version_range == declared.version_range
            && rule.integration_path == declared.integration_path
    });
    let Some(rule) = matched else {
        return Evaluation {
            refusal: Some(format!(
                "No rule in {} speaks for ({}, {}, {}), which {source_ref} declares; a \
                 governed run does not exceed its rule set's authority.",
                governance.identity(),
                declared.edition,
                declared.version_range,
                declared.integration_path
            )),
            matched_rule: None,
            note: None,
        };
    };
    let matched_rule = Some(json!(rule));
    // The rule decides; the declaration corroborates. Where the two
    // disagree, the disagreement is a fact worth a note on the record —
    // the blind spot made visible where it can be — never a verdict of its
    // own.
    let mut notes: Vec<String> = Vec::new();
    if declared.support_status != rule.support_status {
        notes.push(format!(
            "the document declares support_status {:?} but the matched rule says {:?}; \
             the rule decided",
            declared.support_status, rule.support_status
        ));
    }
    let note = |notes: Vec<String>| {
        if notes.is_empty() {
            None
        } else {
            Some(notes.join("; "))
        }
    };
    // Both dates parse: the suite validation refuses an unparseable as_of or
    // effective date before anything runs. Refusing here anyway keeps the
    // processor honest if it is ever reached another way — an unmeasurable
    // date must not admit by accident.
    let (Some(reference), Some(effective)) =
        (parse_date(as_of_date), parse_date(&rule.effective_date))
    else {
        return Evaluation {
            refusal: Some(format!(
                "The dates governing {source_ref} could not be read (as_of {as_of_date:?}, \
                 effective {:?}), so whether rule set {} admits it cannot be known.",
                rule.effective_date,
                governance.identity()
            )),
            matched_rule,
            note: note(notes),
        };
    };
    if rule.support_status == "deprecated" {
        if effective <= reference {
            return Evaluation {
                refusal: Some(format!(
                    "{source_ref} is on a deprecated path: rule set {} deprecates ({}, {}, \
                     {}) effective {}, on or before the suite's as_of {as_of_date}.",
                    governance.identity(),
                    rule.edition,
                    rule.version_range,
                    rule.integration_path,
                    rule.effective_date
                )),
                matched_rule,
                note: note(notes),
            };
        }
        notes.push(format!(
            "deprecation effective {} is after as_of {as_of_date} and not yet in force",
            rule.effective_date
        ));
        return Evaluation {
            refusal: None,
            matched_rule,
            note: note(notes),
        };
    }
    Evaluation {
        refusal: None,
        matched_rule,
        note: note(notes),
    }
}

/// Evaluate one source's declared metadata against the suite's rule set and
/// turn the verdict into the shared `Ruling` shape, so the mode discipline
/// is the same match every other policy uses.
///
/// `metadata` is the envelope's native metadata; the declaration is read
/// from its `governance` key. In-process direct call, like the PII detector:
/// the signature is this function's, not a trait's.
pub fn invoke(
    mode: PolicyMode,
    source_ref: &str,
    content_hash: Option<&str>,
    metadata: &Value,
    governance: &Governance,
    as_of_date: &str,
) -> (Invocation, Ruling) {
    let started_at = Utc::now();
    let declared_value = metadata.get("governance").filter(|value| !value.is_null());
    let declared = declared_value.and_then(declared_from);
    // Metadata that is present but unreadable must not be evaluated as
    // absent: the refusal has to name the right fact.
    let evaluation = if declared_value.is_some() && declared.is_none() {
        Evaluation {
            refusal: Some(format!(
                "{source_ref} declares governance metadata that rule set {} cannot read \
                 (the five declared fields must each be a string), so it cannot be \
                 evaluated.",
                governance.identity()
            )),
            matched_rule: None,
            note: None,
        }
    } else {
        evaluate(declared.as_ref(), governance, as_of_date, source_ref)
    };

    let ruling = match &evaluation.refusal {
        None => Ruling::Allowed,
        Some(reason) => Ruling::breach(
            mode,
            reason.clone(),
            Gap::new(GapReason::PolicyRefused, reason.clone()),
        ),
    };
    let decision = if ruling.is_refusal() {
        Decision::Refuse
    } else {
        Decision::Admit
    };
    let invocation = Invocation::new(
        manifest(),
        started_at,
        decision,
        "one deterministic evaluation of the suite's sealed rule set against the \
         source's declared metadata",
        vec![ArtefactRef {
            reference: source_ref.to_owned(),
            content_hash: content_hash.map(str::to_owned),
            // The governor reads declarations, not text; there is no token
            // count to report and none is invented.
            tokens: None,
        }],
        // The admit stage transforms nothing: the record is the output.
        Vec::new(),
        json!({
            "rule_set": governance.identity(),
            "granted_entitlement": governance.granted_entitlement,
            "as_of": as_of_date,
            "declared": declared.as_ref().map(|declared| json!({
                "edition": declared.edition,
                "version_range": declared.version_range,
                "integration_path": declared.integration_path,
                "support_status": declared.support_status,
                "entitlement": declared.entitlement,
            })),
            "matched_rule": evaluation.matched_rule,
            "note": evaluation.note,
        }),
        ruling.gap().cloned().into_iter().collect(),
        BLIND_SPOTS.to_vec(),
    );
    (invocation, ruling)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn governance() -> Governance {
        serde_json::from_value(json!({
            "name": "fictive-support-rules",
            "version": "1",
            "entitlement_tiers": ["standard", "premier"],
            "granted_entitlement": "standard",
            "rules": [
                {"edition": "orchestrator", "version_range": "4.x",
                 "integration_path": "flux-connector",
                 "support_status": "deprecated", "effective_date": "2026-03-01"},
                {"edition": "orchestrator", "version_range": "4.x",
                 "integration_path": "stream-gateway",
                 "support_status": "supported", "effective_date": "2026-03-01"},
                {"edition": "orchestrator", "version_range": "4.x",
                 "integration_path": "bulk-export",
                 "support_status": "supported", "effective_date": "2026-04-10"}
            ]
        }))
        .expect("a well-formed governance block")
    }

    fn metadata(path: &str, status: &str, entitlement: &str) -> Value {
        json!({"governance": {
            "edition": "orchestrator", "version_range": "4.x",
            "integration_path": path, "support_status": status,
            "entitlement": entitlement,
        }})
    }

    fn strict(metadata: &Value) -> (Invocation, Ruling) {
        invoke(
            PolicyMode::Strict,
            "file:///corpus/doc.md",
            None,
            metadata,
            &governance(),
            "2026-08-06",
        )
    }

    #[test]
    fn a_supported_declared_source_is_admitted_and_the_record_names_the_rule() {
        let (invocation, ruling) = strict(&metadata("stream-gateway", "supported", "standard"));
        assert_eq!(ruling, Ruling::Allowed);
        let record = invocation.to_value();
        assert_eq!(record["decision"], "admit");
        assert_eq!(
            record["detail"]["matched_rule"]["integration_path"],
            "stream-gateway"
        );
        assert_eq!(record["detail"]["rule_set"], "fictive-support-rules/1");
    }

    #[test]
    fn a_deprecated_path_in_force_is_refused_naming_rule_and_dates() {
        let (invocation, ruling) = strict(&metadata("flux-connector", "deprecated", "standard"));
        assert!(ruling.is_refusal());
        let reason = ruling.reason().expect("a refusal has a reason");
        assert!(reason.contains("deprecated"), "{reason}");
        assert!(reason.contains("2026-03-01"), "{reason}");
        assert!(reason.contains("fictive-support-rules/1"), "{reason}");
        assert_eq!(invocation.to_value()["decision"], "refuse");
    }

    #[test]
    fn a_deprecation_not_yet_in_force_admits_with_the_pending_date_recorded() {
        let (invocation, ruling) = invoke(
            PolicyMode::Strict,
            "file:///corpus/doc.md",
            None,
            &metadata("flux-connector", "deprecated", "standard"),
            &governance(),
            "2026-02-01",
        );
        assert_eq!(ruling, Ruling::Allowed);
        let note = invocation.to_value()["detail"]["note"]
            .as_str()
            .expect("the pending deprecation is recorded")
            .to_owned();
        assert!(note.contains("not yet in force"), "{note}");
    }

    /// The rule decides and the declaration corroborates; where they
    /// disagree the record says so, without the disagreement becoming a
    /// verdict of its own.
    #[test]
    fn a_declaration_disagreeing_with_the_matched_rule_is_noted_not_judged() {
        let (invocation, ruling) = strict(&metadata("stream-gateway", "deprecated", "standard"));
        assert_eq!(
            ruling,
            Ruling::Allowed,
            "the matched rule says supported, and the rule decides"
        );
        let note = invocation.to_value()["detail"]["note"]
            .as_str()
            .expect("the disagreement is on the record")
            .to_owned();
        assert!(
            note.contains("\"deprecated\"") && note.contains("\"supported\""),
            "{note}"
        );
        assert!(note.contains("the rule decided"), "{note}");

        // Agreement records no such note.
        let (invocation, _) = strict(&metadata("stream-gateway", "supported", "standard"));
        assert!(invocation.to_value()["detail"]["note"].is_null());
    }

    #[test]
    fn a_source_above_the_granted_tier_is_refused() {
        let (_, ruling) = strict(&metadata("bulk-export", "supported", "premier"));
        assert!(ruling.is_refusal());
        let reason = ruling.reason().unwrap();
        assert!(
            reason.contains("premier") && reason.contains("standard"),
            "{reason}"
        );
    }

    #[test]
    fn a_tier_the_rule_set_does_not_rank_is_refused() {
        let (_, ruling) = strict(&metadata("stream-gateway", "supported", "platinum"));
        assert!(ruling.is_refusal());
        assert!(ruling.reason().unwrap().contains("platinum"));
    }

    #[test]
    fn a_path_no_rule_speaks_for_is_refused() {
        let (_, ruling) = strict(&metadata("pulse-metrics", "experimental", "standard"));
        assert!(ruling.is_refusal());
        let reason = ruling.reason().unwrap();
        assert!(reason.contains("No rule"), "{reason}");
        assert!(reason.contains("pulse-metrics"), "{reason}");
    }

    #[test]
    fn a_source_with_no_declared_metadata_is_refused_naming_the_absence() {
        let (invocation, ruling) = strict(&json!({"path": "overview.md"}));
        assert!(ruling.is_refusal());
        let reason = ruling.reason().unwrap();
        assert!(reason.contains("no governance metadata"), "{reason}");
        assert!(
            invocation.to_value()["detail"]["declared"].is_null(),
            "nothing declared, nothing recorded as declared"
        );
    }

    #[test]
    fn unreadable_metadata_is_refused_as_unreadable_not_as_absent() {
        let (_, ruling) = strict(&json!({"governance": {"edition": 4}}));
        assert!(ruling.is_refusal());
        assert!(ruling.reason().unwrap().contains("cannot read"));
    }

    #[test]
    fn observe_records_the_same_breach_and_carries_on() {
        let refused = strict(&metadata("flux-connector", "deprecated", "standard")).1;
        let (invocation, observed) = invoke(
            PolicyMode::Observe,
            "file:///corpus/doc.md",
            None,
            &metadata("flux-connector", "deprecated", "standard"),
            &governance(),
            "2026-08-06",
        );
        assert!(!observed.is_refusal());
        assert_eq!(
            refused.gap(),
            observed.gap(),
            "the mode decides what happens to a breach, never how it is recorded"
        );
        assert_eq!(invocation.to_value()["decision"], "admit");
    }
}
