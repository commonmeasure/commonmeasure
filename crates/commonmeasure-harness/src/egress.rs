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
    if !(is_witnessed(record) || is_refused(record) || is_turn_boundary(record)) {
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
