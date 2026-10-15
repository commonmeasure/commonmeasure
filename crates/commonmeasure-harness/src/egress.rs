//! Which records of a session may leave for the telemetry receiver, and
//! which crossings operator terms hold back.
//!
//! The relay projects under these predicates (`commonmeasure-relay`), and
//! the reporting ruling reads the same ones before it rules a licence's
//! reporting demand met (`mcp`), so that a crossing admitted on the promise
//! of reporting is one the relay can deliver. They live here, beside the
//! policy they read, because the relay depends on this crate and not the
//! other way round.

use crate::policy::{EgressWithheld, PolicyDocument};
use serde_json::{Value, json};

/// Whether an evidence record is a crossing this machine watched happen.
/// `crossing_refused` is a rejected source and `crossing_reconstructed` is a
/// transcript claim; neither is witnessed, and neither leaves. The one home for
/// the predicate, because the caller deciding what may be projected and the
/// projection deciding what to build must be asking the same question.
pub fn is_witnessed(record: &Value) -> bool {
    matches!(
        record["event"].as_str(),
        Some("crossing_observed" | "crossing_mediated")
    )
}

/// Whether a witnessed crossing was admitted on the promise that it would be
/// reported: its licence demanded reporting, the edge ruled the demand met,
/// and the ruling records the operator's reporting consent as agreed (owner
/// decision, 27 September 2026: consent before an obligated crossing). The
/// relay clears such a crossing whatever its scope clears. A record made
/// before the consent existed carries no consent and is not cleared by it.
pub fn reported_under_consent(record: &Value) -> bool {
    let reporting = &record["payload"]["declarations"]["reporting"];
    is_witnessed(record)
        && reporting["met"] == json!(true)
        && reporting["consent"]["state"] == "agreed"
}

/// Whether a record is a crossing made under the anonymous access profile
/// (product decision "Anonymous access profile"): its `anonymous` field
/// names the Anchor the host's operator terms name. Such a crossing stays
/// on the reader's own record. It is never cleared for egress, in any scope
/// and under any consent, so nothing of it is projected, and a refusal under
/// the profile is left out of the refused count.
pub fn under_anonymous_profile(record: &Value) -> bool {
    record["payload"]["anonymous"].is_object()
}

/// Whether the host recorded a turn boundary. Its clearance is resolved from
/// `payload.detail.cwd`, independently of any neighbouring crossing.
pub fn is_turn_boundary(record: &Value) -> bool {
    matches!(
        record["event"].as_str(),
        Some("turn_started" | "turn_completed")
    )
}

/// Whether an evidence record is a crossing policy refused. Nothing of it is
/// projected; the session's batches carry how many there were. The event
/// name alone decides: a refused file's `content_type` and `breach` are not
/// read.
pub fn is_refused(record: &Value) -> bool {
    record["event"] == json!("crossing_refused")
}

/// What lets one record leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleared {
    /// The scope matching the record's working directory clears telemetry
    /// egress under this governing engagement.
    Scope(String),
    /// The record is a crossing admitted under the operator's reporting
    /// consent ([`reported_under_consent`]); it clears that crossing alone.
    ReportingConsent,
}

/// What lets `record` leave under `policy` as it stands now, or `None`.
///
/// A scope's clearance comes first. Where the scope clears nothing, a
/// witnessed crossing met under reporting consent is cleared by that
/// consent: the edge admitted it on the promise that it would be reported.
/// The consent is read from the record, not from the home's consent file
/// now: a withdrawal applies to crossings after it and recalls nothing
/// admitted before it. Clearance implies a named engagement:
/// `allows_telemetry_egress` is false without one, and the loader refuses a
/// policy that clears egress under no name.
pub fn clearance(policy: &PolicyDocument, record: &Value) -> Option<Cleared> {
    if !(is_witnessed(record) || is_refused(record) || is_turn_boundary(record))
        || under_anonymous_profile(record)
    {
        return None;
    }
    let cwd = if is_turn_boundary(record) {
        record["payload"]["detail"]["cwd"].as_str()
    } else {
        record["payload"]["cwd"].as_str()
    };
    let resolved = policy.resolve(cwd);
    resolved
        .allows_telemetry_egress()
        .then(|| {
            resolved
                .governing_engagement()
                .map(|name| Cleared::Scope(name.to_owned()))
        })
        .flatten()
        .or_else(|| reported_under_consent(record).then_some(Cleared::ReportingConsent))
}

/// Why a crossing made in `cwd` may not leave under its scope, or `None`
/// where the scope clears it ([`PolicyDocument::resolve`]). A crossing the
/// reporting consent cleared is cleared whatever this says ([`clearance`]).
pub fn withheld_in(policy: &PolicyDocument, cwd: Option<&str>) -> Option<EgressWithheld> {
    match cwd {
        None => Some(EgressWithheld::NoDirectory),
        Some(cwd) => policy.resolve(Some(cwd)).egress_withheld().cloned(),
    }
}

/// The reference of the operator terms that name institution identifiers
/// for `host` in the scope `cwd` resolves to, or `None`. Such terms require
/// `access_context` on the session (`docs/contracts/session-evidence.md`
/// §Source declarations), which an event batch cannot carry.
pub fn access_context_terms(
    policy: &PolicyDocument,
    cwd: Option<&str>,
    host: &str,
) -> Option<String> {
    policy
        .resolve(cwd)
        .terms_for(host)
        .filter(|terms| !terms.access_context.is_empty())
        .map(|terms| terms.reference.clone())
}

/// The reference and Anchor origin of the operator terms that put `host`
/// under the anonymous access profile in the scope `cwd` resolves to, or
/// `None`. A crossing of such a host is never cleared for egress
/// ([`under_anonymous_profile`]), so a reporting demand on it cannot be met.
pub fn anchor_terms(
    policy: &PolicyDocument,
    cwd: Option<&str>,
    host: &str,
) -> Option<(String, String)> {
    let resolved = policy.resolve(cwd);
    let terms = resolved.terms_for(host)?;
    let anchor = terms.anchor.as_ref()?;
    Some((terms.reference.clone(), anchor.origin.clone()))
}

/// The reference of the operator terms that keep `record` home, or `None`:
/// a witnessed crossing whose host, in the scope its `cwd` resolves to,
/// falls under terms naming institution identifiers. Such terms require the
/// standard's session-level `access_context` on any report of that host's
/// content, and the event-batch delivery format has no session data to
/// carry it, so the relay sends nothing of the crossing. Only the crossing
/// is held: the other crossings of its session report no use of that host,
/// and they leave as their own clearance says. A refusal is not held: it
/// leaves only inside the session's refused count, which reports no use.
pub fn held_for_access_context(policy: &PolicyDocument, record: &Value) -> Option<String> {
    if !is_witnessed(record) {
        return None;
    }
    let payload = &record["payload"];
    access_context_terms(
        policy,
        payload["cwd"].as_str(),
        payload["host_name"].as_str().unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy whose one scope clears telemetry egress under a named
    /// engagement, for crossings made in `/work/acme`.
    fn clearing_policy() -> PolicyDocument {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            json!({"policy_mode": "observe", "scopes": [
                {"match": "/work/acme", "engagement": "acme", "allow_telemetry_egress": true}
            ]})
            .to_string(),
        )
        .unwrap();
        PolicyDocument::read(home.path()).expect("valid policy")
    }

    // Catches: clearing a display for relay egress. A display is the
    // consumer surface's own record of what a reader was shown; it never
    // leaves, under any scope, alongside the crossing that fetched the
    // release it was rendered against.
    #[test]
    fn a_rendered_display_is_never_cleared_for_egress() {
        let policy = clearing_policy();
        let crossing = json!({"event": "crossing_mediated", "payload": {"cwd": "/work/acme"}});
        assert_eq!(
            clearance(&policy, &crossing),
            Some(Cleared::Scope("acme".to_owned()))
        );
        let display = json!({"event": "display_rendered", "payload": {
            "cwd": "/work/acme",
            "tool": "context_show",
            "processor_record": 3,
            "outcomes": ["quote_shown"],
            "counts": {"vector": [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]},
        }});
        assert!(!is_witnessed(&display));
        assert!(!is_refused(&display));
        assert!(!is_turn_boundary(&display));
        assert_eq!(clearance(&policy, &display), None);
    }

    // Catches: clearing a crossing made under the anonymous access profile.
    // Neither the scope that clears every crossing in its directory nor the
    // reporting consent on the record lets it leave, and a refusal under the
    // profile is not counted either, since the count is taken over cleared
    // refusals.
    #[test]
    fn a_crossing_under_the_anonymous_profile_is_never_cleared() {
        let policy = clearing_policy();
        let anonymous = json!({"anchor": "https://anchor.example"});
        let carried = json!({"event": "crossing_mediated", "payload": {
            "cwd": "/work/acme",
            "host_name": "source.example",
            "anonymous": anonymous,
            "identity": {"user_agent": "CommonMeasureBot/0", "pass": {
                "anchor": "https://anchor.example", "truncated_token_key_id": 7}},
            "declarations": {"reporting": {"met": true, "consent": {"state": "agreed"}}},
        }});
        let refused = json!({"event": "crossing_refused", "payload": {
            "cwd": "/work/acme", "host_name": "source.example", "anonymous": anonymous,
            "refusal": "refused: no pass",
        }});
        for record in [&carried, &refused] {
            assert!(under_anonymous_profile(record));
            assert_eq!(clearance(&policy, record), None, "{record}");
        }
        let mut signed = carried.clone();
        signed["payload"]
            .as_object_mut()
            .unwrap()
            .remove("anonymous");
        assert!(reported_under_consent(&signed));
        assert_eq!(
            clearance(&policy, &signed),
            Some(Cleared::Scope("acme".to_owned()))
        );
    }

    // Catches: clearing a refill of anonymous passes for relay egress. A
    // refill names the Anchor, the host and the key the edge's passes are
    // under; it never leaves, under any scope, even one that clears every
    // crossing in its directory.
    #[test]
    fn a_pass_refill_is_never_cleared_for_egress() {
        let policy = clearing_policy();
        let refill = json!({"event": "passes_refilled", "payload": {
            "cwd": "/work/acme",
            "anchor": "https://anchor.example",
            "origin_info": ["source.example"],
            "requested": 5,
            "obtained": 5,
            "held": 5,
            "key_id": "ab".repeat(32),
            "stopped": null,
        }});
        assert!(!is_witnessed(&refill));
        assert!(!is_refused(&refill));
        assert!(!is_turn_boundary(&refill));
        assert_eq!(clearance(&policy, &refill), None);
    }
}
