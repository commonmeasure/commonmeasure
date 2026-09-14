//! The fleet-status document: what one edge reports about the policy it
//! applies, on a contract of its own (`docs/contracts/fleet-status.md`).
//!
//! The document exists so an organisation running many edges can tell
//! whether each one applies the policy it is meant to, without any edge
//! exporting its policy document. It carries digests, versions, a last
//! enforcement time and a bounded allowance summary; it carries no prompt,
//! answer, policy text, per-crossing spend or engagement name. It travels
//! separately from Content Telemetry, which is purpose-limited to what an
//! agent retrieved and grounded on, and it never extends that wire.
//!
//! Everything here is the edge's report about itself. A digest it reports
//! is drift evidence: two edges reporting one digest resolved one effective
//! policy. It is not attestation that an uncompromised edge enforced it.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_runtime::allowance::{Account, Ledger};
use serde_json::{Value, json};

use crate::policy::{IDENTITY_SCHEMA, PolicyDocument, RESOLVER_VERSION};
use crate::session::SessionLog;

/// The version of the document [`status`] builds. It moves when a field is
/// added, removed or given a new meaning.
pub const CONTRACT: &str = "contextops-fleet-status/v1";

/// The pseudonymous identity of this edge: the key id minted at enrolment
/// (`docs/GLOSSARY.md` §Key id). An edge with no key has no identity to
/// report, and the document says so rather than inventing one from a
/// hostname or a user name, neither of which is pseudonymous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeIdentity {
    KeyId(String),
    Unknown { reason: String },
}

impl EdgeIdentity {
    fn to_value(&self) -> Value {
        match self {
            Self::KeyId(key_id) => json!({"key_id": key_id}),
            Self::Unknown { reason } => json!({"key_id": null, "unknown": reason}),
        }
    }
}

/// What the edge knows about desired state: the deployment mode it chose,
/// the last desired revision it learned of and what it did with it, and the
/// revision it activated. A local edge learns of no desired state and
/// reports none, which a receiver reads as unknown, never as current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Management {
    /// `local` or `managed` (`docs/contracts/fleet-status.md` §Deployment
    /// mode), or `unavailable` where the deployment file does not load.
    pub mode: String,
    /// The last desired revision this edge learned of, with the outcome:
    /// `null` when none was ever learned.
    pub desired: Value,
    /// The desired revision whose policy is the one on disk, when the edge
    /// activated one. `None` for a local edge and for a managed edge that
    /// has activated nothing yet.
    pub applied_revision: Option<u64>,
    /// The digest the applied revision's envelope named it by, over the
    /// policy as the envelope carried it; `None` where `applied_revision`
    /// is. A receiver compares this with the digest it published.
    pub applied_digest: Option<String>,
    /// Whether the policy on disk has been changed since the revision was
    /// activated: the file no longer digests to the loader's form of the
    /// revision's policy. `None` where that cannot be determined.
    pub applied_edited: Option<bool>,
    /// When the applied envelope expires, as it states; `None` where
    /// `applied_revision` is.
    pub applied_expires_at: Option<String>,
    /// When the applied envelope expired, when it has. The policy it
    /// carried stays in force; the record says it is stale
    /// (`docs/contracts/policy-envelope.md` §Cadence and staleness).
    pub stale_since: Option<String>,
}

impl Management {
    /// A local edge: no remote policy is accepted and none is reported.
    pub fn local() -> Self {
        Self {
            mode: "local".to_owned(),
            desired: Value::Null,
            applied_revision: None,
            applied_digest: None,
            applied_edited: None,
            applied_expires_at: None,
            stale_since: None,
        }
    }
}

/// Build this edge's status document at `now`.
///
/// `cwd` is the working directory the applied identity is resolved for,
/// because an identity is a resolution and a resolution has a directory;
/// the document reports the identity and not the directory. The principal
/// is the one this process runs as, resolved by the policy loader from the
/// same basis a mediated crossing would use.
pub fn status(
    home: &Path,
    cwd: Option<&str>,
    edge: &EdgeIdentity,
    management: &Management,
    now: DateTime<Utc>,
) -> Value {
    // One read of the declaration for every figure in the document, so an
    // edit while the document is built cannot put two declarations in it.
    let document = PolicyDocument::read(home);
    let applied = match &document {
        Ok(document) => {
            let resolved = document.resolve(cwd);
            json!({
                "revision": management.applied_revision,
                "digest": management.applied_digest,
                "edited_locally": management.applied_edited,
                "expires_at": management.applied_expires_at,
                "stale_since": management.stale_since,
                "policy_declared": document.declared(),
                "policy_digest": document.digest(),
                "policy_identity": resolved.identity().to_value(),
                "principal": {
                    "name": resolved.principal(),
                    "basis": resolved.authentication_basis().as_str(),
                },
            })
        }
        // A policy that does not load governs nothing a mediating server
        // would start under; the document reports the load error and no
        // digest, so a receiver reads it as unknown rather than as a
        // permissive default that was never declared.
        Err(error) => json!({
            "revision": management.applied_revision,
            "digest": management.applied_digest,
            "edited_locally": management.applied_edited,
            "expires_at": management.applied_expires_at,
            "stale_since": management.stale_since,
            "policy_declared": true,
            "policy_digest": null,
            "policy_identity": null,
            "principal": null,
            "unavailable": error,
        }),
    };
    json!({
        "contract": CONTRACT,
        "generated_at": now.to_rfc3339_opts(SecondsFormat::Millis, true),
        "edge": edge.to_value(),
        "deployment_mode": management.mode,
        "desired": management.desired,
        "applied": applied,
        "versions": {
            "commonmeasure": env!("CARGO_PKG_VERSION"),
            "resolver": RESOLVER_VERSION,
            "identity_schema": IDENTITY_SCHEMA,
        },
        "last_enforcement": last_enforcement(home),
        "allowances": allowance_summary(home, &document, now),
    })
}

/// The most recent moment this edge ruled on a crossing: the latest
/// timestamp on a mediated or refused crossing across the session logs.
/// An edge that has never mediated has no heartbeat, and says so.
fn last_enforcement(home: &Path) -> Value {
    let logs = match SessionLog::list(home) {
        Ok(logs) => logs,
        Err(error) => {
            return json!({"at": null, "unknown": format!("session logs unreadable: {error}")});
        }
    };
    let mut latest: Option<String> = None;
    for path in logs {
        let Ok(records) = SessionLog::read(&path) else {
            continue;
        };
        for record in records {
            if !matches!(
                record["event"].as_str(),
                Some("crossing_mediated") | Some("crossing_refused")
            ) {
                continue;
            }
            if let Some(timestamp) = record["payload"]["timestamp"].as_str()
                && latest.as_deref().is_none_or(|seen| timestamp > seen)
            {
                latest = Some(timestamp.to_owned());
            }
        }
    }
    match latest {
        Some(at) => json!({
            "at": at,
            "basis": "the latest mediated or refused crossing in this edge's session logs",
        }),
        None => json!({"at": null, "unknown": "no mediated crossing has been recorded"}),
    }
}

/// The bounded allowance summary: for each principal with a declared
/// allowance, the period it is in and what remains, read from the ledger
/// enforcement reads. Never a provider, a quote, a receipt or a crossing
/// (`DECISIONS.md` §Delegated authority and fleet management).
fn allowance_summary(
    home: &Path,
    document: &Result<PolicyDocument, String>,
    now: DateTime<Utc>,
) -> Value {
    let document = match document {
        Ok(document) => document,
        Err(error) => return json!({"unavailable": error}),
    };
    let ledger = Ledger::in_home(home);
    let mut periods: Vec<Value> = Vec::new();
    for binding in document.principals() {
        if binding.allowances.is_empty() {
            continue;
        }
        let account = Account {
            principal: &binding.principal,
            declarations: &binding.allowances,
        };
        match ledger.state(&account, now) {
            Ok(states) => periods.extend(states.into_iter().map(|state| {
                json!({
                    "principal": binding.principal,
                    "period": state.period.as_str(),
                    "period_key": state.period_key,
                    "remaining": state.remaining,
                    "exceeded": state.exceeded,
                })
            })),
            Err(error) => periods.push(json!({
                "principal": binding.principal,
                "unavailable": error,
            })),
        }
    }
    json!(periods)
}

/// What a receiver concludes about one edge from its status document and
/// the revision and digest the receiver itself holds as desired. The
/// receiver needs no policy document for this: the digests are compared,
/// never the policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convergence {
    /// The edge activated the desired revision and its policy still
    /// digests to what was distributed.
    Current,
    /// The edge activated an earlier revision than the desired one.
    Stale,
    /// The edge reports the desired revision, or a later one, but its
    /// policy digests to something else: edited locally after activation,
    /// or ahead of what the receiver believes is desired.
    Divergent,
    /// The document says nothing a receiver can compare: a local edge, a
    /// managed edge that has activated nothing, or a policy that did not
    /// load.
    Unknown,
}

impl Convergence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Divergent => "divergent",
            Self::Unknown => "unknown",
        }
    }
}

/// Classify one status document against the desired revision and digest
/// the receiver holds. Returns the conclusion and the one fact it rests on.
pub fn classify(
    status: &Value,
    desired_revision: u64,
    desired_digest: &str,
) -> (Convergence, String) {
    let applied = &status["applied"];
    let Some(digest) = applied["policy_digest"].as_str() else {
        return (
            Convergence::Unknown,
            match applied["unavailable"].as_str() {
                Some(reason) => format!("the edge's policy did not load: {reason}"),
                None => "the edge declares no policy".to_owned(),
            },
        );
    };
    let Some(revision) = applied["revision"].as_u64() else {
        return (
            Convergence::Unknown,
            format!(
                "the edge reports no applied revision (deployment mode {})",
                status["deployment_mode"].as_str().unwrap_or("unstated")
            ),
        );
    };
    if revision < desired_revision {
        return (
            Convergence::Stale,
            format!("the edge applied revision {revision}; revision {desired_revision} is desired"),
        );
    }
    // An edge that reports the digest its envelope named the revision by is
    // compared on that, whatever form the receiver published the policy in,
    // and says itself whether the file has moved since; an older edge is
    // compared on the loader's digest of the file, which agrees only where
    // the receiver published the loader's form.
    let (compared, edited) = match applied["digest"].as_str() {
        Some(named) => (named, applied["edited_locally"] == json!(true)),
        None => (digest, false),
    };
    if revision == desired_revision && compared == desired_digest && !edited {
        return (
            Convergence::Current,
            format!(
                "the edge applied revision {revision} and its policy digests to what was distributed"
            ),
        );
    }
    (
        Convergence::Divergent,
        if revision == desired_revision {
            format!(
                "the edge applied revision {revision} but its policy digests to {digest}, not {desired_digest}"
            )
        } else {
            format!(
                "the edge applied revision {revision}, later than the desired {desired_revision}"
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(policy: Option<&str>) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        if let Some(policy) = policy {
            std::fs::write(home.path().join("policy.json"), policy).expect("policy");
        }
        home
    }

    fn local_status(home: &Path) -> Value {
        status(
            home,
            None,
            &EdgeIdentity::Unknown {
                reason: "not enrolled".to_owned(),
            },
            &Management::local(),
            Utc::now(),
        )
    }

    /// The document carries what the contract lists and nothing the
    /// contract excludes.
    #[test]
    fn the_document_carries_the_contract_fields_and_no_policy_text() {
        let home = home_with(Some(
            r#"{"policy_mode":"strict","scopes":[{"match":"/secret/client","engagement":"acme","allow_telemetry_egress":true}],
                "principals":[{"principal":"alice","os_user":1001,"allowances":[
                    {"period":"day","amount":{"currency":"USD","micros":5000000},"timezone":"UTC"}]}]}"#,
        ));
        let document = local_status(home.path());
        let mut keys: Vec<&str> = document
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "allowances",
                "applied",
                "contract",
                "deployment_mode",
                "desired",
                "edge",
                "generated_at",
                "last_enforcement",
                "versions",
            ]
        );
        assert_eq!(document["contract"], CONTRACT);
        assert_eq!(document["deployment_mode"], "local");
        assert!(document["desired"].is_null());
        assert!(document["edge"]["key_id"].is_null());
        assert_eq!(document["edge"]["unknown"], "not enrolled");
        assert!(document["applied"]["revision"].is_null());
        assert_eq!(document["applied"]["policy_declared"], true);
        assert!(
            document["applied"]["policy_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert_eq!(
            document["applied"]["policy_identity"]["schema"],
            IDENTITY_SCHEMA
        );
        assert_eq!(document["versions"]["resolver"], RESOLVER_VERSION);
        assert!(document["last_enforcement"]["at"].is_null());
        assert_eq!(document["allowances"][0]["principal"], "alice");
        assert_eq!(document["allowances"][0]["period"], "day");
        assert_eq!(document["allowances"][0]["remaining"]["micros"], 5000000);
        assert_eq!(document["allowances"][0]["exceeded"], false);

        let text = document.to_string();
        for excluded in [
            "acme",
            "/secret/client",
            "allow_telemetry_egress",
            "constraints",
        ] {
            assert!(
                !text.contains(excluded),
                "{excluded} is policy detail or an engagement name and must not leave: {text}"
            );
        }
        assert!(document["allowances"][0].get("declared").is_none());
    }

    #[test]
    fn an_absent_or_unreadable_policy_reports_no_digest_and_classifies_unknown() {
        let absent = local_status(home_with(None).path());
        assert_eq!(absent["applied"]["policy_declared"], false);
        assert!(absent["applied"]["policy_digest"].is_null());
        assert!(
            absent["applied"]["policy_identity"]["digest"].is_string(),
            "an absent policy still resolves to the observing default, and that is an identity"
        );
        assert_eq!(classify(&absent, 1, "sha256:x").0, Convergence::Unknown);

        let broken = local_status(home_with(Some("{ not json")).path());
        assert_eq!(broken["applied"]["policy_declared"], true);
        assert!(broken["applied"]["policy_digest"].is_null());
        assert!(broken["applied"]["policy_identity"].is_null());
        assert!(broken["applied"]["unavailable"].is_string());
        assert!(broken["allowances"]["unavailable"].is_string());
        let (verdict, reason) = classify(&broken, 1, "sha256:x");
        assert_eq!(verdict, Convergence::Unknown);
        assert!(reason.contains("did not load"), "{reason}");
    }

    /// A receiver's four answers, from the document and its own desired
    /// revision and digest alone.
    #[test]
    fn a_receiver_distinguishes_current_stale_divergent_and_unknown_without_the_policy() {
        let home = home_with(Some(r#"{"policy_mode":"strict"}"#));
        let digest = PolicyDocument::read(home.path()).unwrap().digest().unwrap();
        let managed = |applied_revision: Option<u64>| {
            status(
                home.path(),
                None,
                &EdgeIdentity::KeyId("edge-key".to_owned()),
                &Management {
                    mode: "managed".to_owned(),
                    desired: json!({"revision": applied_revision, "outcome": "accepted"}),
                    applied_revision,
                    applied_digest: None,
                    applied_edited: None,
                    applied_expires_at: None,
                    stale_since: None,
                },
                Utc::now(),
            )
        };
        assert_eq!(
            classify(&managed(Some(7)), 7, &digest).0,
            Convergence::Current
        );
        assert_eq!(
            classify(&managed(Some(6)), 7, &digest).0,
            Convergence::Stale
        );
        let (verdict, reason) = classify(&managed(Some(7)), 7, "sha256:other");
        assert_eq!(verdict, Convergence::Divergent);
        assert!(reason.contains("digests to"), "{reason}");
        assert_eq!(
            classify(&managed(Some(8)), 7, &digest).0,
            Convergence::Divergent
        );
        let (verdict, reason) = classify(&managed(None), 7, &digest);
        assert_eq!(verdict, Convergence::Unknown);
        assert!(reason.contains("no applied revision"), "{reason}");
        assert_eq!(
            classify(&local_status(home.path()), 7, &digest).0,
            Convergence::Unknown
        );
    }

    /// The heartbeat is the latest ruling on record, and only a ruling:
    /// an observed crossing is not enforcement.
    #[test]
    fn the_last_enforcement_is_the_latest_mediated_or_refused_crossing() {
        let home = home_with(None);
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("a.ndjson"),
            concat!(
                r#"{"seq":1,"event":"crossing_observed","payload":{"timestamp":"2026-09-06T12:00:00.000Z"}}"#,
                "\n",
                r#"{"seq":2,"event":"crossing_refused","payload":{"timestamp":"2026-09-05T09:00:00.000Z"}}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            sessions.join("b.ndjson"),
            concat!(
                r#"{"seq":1,"event":"crossing_mediated","payload":{"timestamp":"2026-09-05T10:00:00.000Z"}}"#,
                "\n",
            ),
        )
        .unwrap();
        let document = local_status(home.path());
        assert_eq!(
            document["last_enforcement"]["at"],
            "2026-09-05T10:00:00.000Z"
        );
        assert!(document["last_enforcement"]["basis"].is_string());
    }

    /// A revision published in the owner's form is named by that form's
    /// digest, and an edge reporting it is current on that digest while its
    /// file still holds the revision's policy, whatever the loader's form
    /// digests to; a local edit makes it divergent.
    #[test]
    fn an_owner_form_revision_is_current_on_the_digest_its_envelope_named() {
        use commonmeasure_types::canonical::canonical_digest;
        let home = home_with(Some(r#"{"policy_mode":"strict"}"#));
        let owner_form = canonical_digest(&json!({"policy_mode": "strict"}));
        let loader_form = PolicyDocument::read(home.path()).unwrap().digest().unwrap();
        assert_ne!(owner_form, loader_form);
        let managed = |edited: bool| {
            status(
                home.path(),
                None,
                &EdgeIdentity::KeyId("edge-key".to_owned()),
                &Management {
                    mode: "managed".to_owned(),
                    desired: json!({"revision": 7, "outcome": "accepted"}),
                    applied_revision: Some(7),
                    applied_digest: Some(owner_form.clone()),
                    applied_edited: Some(edited),
                    applied_expires_at: Some("2026-09-21T00:00:00Z".to_owned()),
                    stale_since: None,
                },
                Utc::now(),
            )
        };
        let document = managed(false);
        assert_eq!(document["applied"]["digest"], owner_form);
        assert_eq!(document["applied"]["policy_digest"], loader_form);
        assert_eq!(classify(&document, 7, &owner_form).0, Convergence::Current);
        assert_eq!(
            classify(&managed(true), 7, &owner_form).0,
            Convergence::Divergent
        );
        assert_eq!(classify(&document, 8, &owner_form).0, Convergence::Stale);
    }
}
