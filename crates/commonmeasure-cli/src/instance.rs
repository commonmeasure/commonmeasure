//! `commonmeasure instance`: register, renew, close and read a working
//! instance at the hub this edge is enrolled with
//! (`docs/contracts/session-evidence.md` §Instance registration).

use chrono::{Duration, Utc};
use commonmeasure_harness::instance::{Retained, Work};
use commonmeasure_harness::{home_dir, safe_session};
use commonmeasure_relay::instance_registration::{self as client, Evidence, Report};
use serde_json::{Value, json};

/// The namespace of a `job` work reference: a `ContextJob.id`.
const JOB_NAMESPACE: &str = "context-job";

#[derive(clap::Subcommand)]
pub enum InstanceCommand {
    /// Register a working instance for a session, a job or both. Needs an
    /// enrolled, managed edge with an applied source policy, and the owner's
    /// delegation and custody acceptance from the hub. Once accepted, every
    /// record the session appends carries the instance reference, and each
    /// mediated fetch or search first checks the binding with the hub: work
    /// is refused once the validity window has passed or when the check
    /// cannot be made. The source policy is enforced either way.
    Register {
        /// The session whose records are attributed to the instance, sent
        /// as a `session` work reference in the `--host` namespace.
        /// `AGENT_SESSION_ID` is used when this is omitted.
        #[arg(long)]
        session: Option<String>,
        /// A ContextJob id, sent as a `job` work reference in the
        /// `context-job` namespace.
        #[arg(long)]
        job: Option<String>,
        /// Attribute the session's records to the instance without naming
        /// the session as a work reference, for a delegation that covers
        /// the job alone. Requires --job.
        #[arg(long, requires = "job")]
        omit_session_work: bool,
        /// The owner's delegation for this edge key (hub identifier).
        #[arg(long)]
        delegation: String,
        /// The durable service's custody acceptance (hub identifier).
        #[arg(long)]
        acceptance: String,
        /// The host this instance runs in, as it is declared to the hub,
        /// e.g. `claude-code`. A declaration, not a credential.
        #[arg(long)]
        host: String,
        /// The host's version, as declared.
        #[arg(long)]
        host_version: String,
        /// The purpose category, which must be the delegation's.
        #[arg(long)]
        purpose: String,
        /// Requested lifetime in minutes. The hub refuses more than 24
        /// hours, or longer than the delegation or the acceptance lasts.
        #[arg(long, default_value_t = 60)]
        expires_in_minutes: i64,
        /// The parent instance, for a child registration.
        #[arg(long)]
        parent: Option<String>,
        /// An ended instance this one replaces. Limits and duties carry over.
        #[arg(long)]
        predecessor: Option<String>,
        /// A reporting duty the instance carries, as the hub names it:
        /// `owner:<content owner id>`. Repeatable. The hub binds a duty only
        /// under a custody acceptance made with reporting.
        #[arg(long, value_name = "REFERENCE")]
        duty: Vec<String>,
        /// Register although the session already points at an active
        /// instance. The session's later records are attributed to the new
        /// instance; the earlier one stays active at the hub until it is
        /// closed or expires.
        #[arg(long)]
        replace_session: bool,
        /// Reuse the key of an earlier attempt so the hub answers its
        /// retained result instead of registering again. 1 to 128 ASCII
        /// letters, digits, `-` and `_`.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Renew an instance before it expires, restating its registration with
    /// the source policy now applied. An expired, closed or revoked instance
    /// does not renew.
    Renew {
        instance: String,
        #[arg(long, default_value_t = 60)]
        expires_in_minutes: i64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Close an instance with its outcome and final evidence digests. If the
    /// hub is not reached the closure is recorded as unacknowledged; repeat
    /// the command with the same --idempotency-key.
    Close {
        instance: String,
        #[arg(long, value_parser = ["completed", "cancelled", "failed"])]
        outcome: String,
        /// A final evidence reference and its digest, as
        /// `<reference>=sha256:<64 hex>`. Repeatable.
        #[arg(long, value_name = "REFERENCE=DIGEST")]
        evidence: Vec<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Read an instance's standing from the hub and print the retained
    /// record. With no instance, list the instances recorded on this edge
    /// without contacting the hub.
    Status { instance: Option<String> },
}

fn key(given: Option<String>, operation: &str) -> String {
    given.unwrap_or_else(|| client::fresh_key(operation))
}

/// The summary a lifecycle command prints. `closure` is the hub's answer to
/// closure as this edge retained it (outcome, time and custody receipt) and
/// is null where the record holds none, which is also the case after a
/// closure the hub did not acknowledge.
fn summary(report: &Report) -> Value {
    let retained = report.retained.as_ref();
    json!({
        "operation": report.operation,
        "instance": retained.map(|held| &held.instance),
        "issuer": retained.map(|held| &held.issuer),
        "revision": retained.map(|held| held.revision),
        "status": retained.map(|held| &held.status),
        "valid_from": retained.map(|held| &held.valid_from),
        "expires_at": retained.map(|held| &held.expires_at),
        "next_check": retained.map(|held| &held.next_check),
        "online_validation_required": retained.map(|held| held.online_validation_required),
        "session_id": retained.and_then(|held| held.session_id.as_ref()),
        "session_taken_from": retained.and_then(|held| held.session_taken_from.as_ref()),
        "duties": retained.and_then(|held| held.duties.as_ref()),
        "closure": retained.and_then(|held| held.closure.as_ref()),
        "record": report.record,
    })
}

fn print(report: &Report) -> Result<(), String> {
    let retained = report.retained.as_ref();
    println!(
        "{}",
        serde_json::to_string_pretty(&summary(report)).map_err(|error| error.to_string())?
    );
    if report.accepted() {
        Ok(())
    } else {
        // The two remedies an operator cannot read off the outcome alone.
        let remedy = if let Some(hub_time) = &report.operation.hub_time {
            format!(
                " The hub did not accept the request's signature, which is not a ruling on the \
                 instance; its clock read {hub_time} and this machine's {}.",
                report.operation.at
            )
        } else if report.operation.outcome == "binding_rejected"
            && let Some(held) = retained
        {
            format!(
                " The hub holds instance {} at revision {}; `commonmeasure instance close {} \
                 --outcome failed` ends it.",
                held.instance, held.revision, held.instance
            )
        } else {
            String::new()
        };
        Err(format!(
            "{} was not accepted: {} ({}). Recorded in {}; idempotency key {}.{remedy}",
            report.operation.operation,
            report.operation.outcome,
            report
                .operation
                .reason
                .as_deref()
                .unwrap_or("no reason given"),
            report.record.display(),
            report.operation.idempotency_key
        ))
    }
}

fn parse_evidence(raw: &str) -> Result<Evidence, String> {
    let (reference, digest) = raw
        .rsplit_once('=')
        .filter(|(reference, digest)| !reference.is_empty() && !digest.is_empty())
        .ok_or_else(|| format!("--evidence {raw:?} is not <reference>=<digest>"))?;
    Ok(Evidence {
        reference: reference.to_owned(),
        digest: digest.to_owned(),
    })
}

fn list(home: &std::path::Path) -> Result<(), String> {
    let directory = commonmeasure_harness::instance::directory(home);
    let mut held = Vec::new();
    match std::fs::read_dir(&directory) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                    && let Some(instance) = path.file_stem().and_then(|stem| stem.to_str())
                    && let Some(record) = Retained::read(home, instance)?
                {
                    held.push(json!({
                        "instance": record.instance,
                        "revision": record.revision,
                        "status": record.status,
                        "expires_at": record.expires_at,
                        "session_id": record.session_id,
                    }));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"instances": held}))
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

pub fn run(command: InstanceCommand) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let now = Utc::now();
    match command {
        InstanceCommand::Register {
            session,
            job,
            omit_session_work,
            delegation,
            acceptance,
            host,
            host_version,
            purpose,
            expires_in_minutes,
            parent,
            predecessor,
            duty,
            replace_session,
            idempotency_key,
        } => {
            let session = session.or_else(|| {
                std::env::var("AGENT_SESSION_ID")
                    .ok()
                    .filter(|value| !value.is_empty())
            });
            if let Some(session) = &session
                && safe_session(session).is_none()
            {
                return Err(format!(
                    "session {session:?} is not a plain identifier, so no session log has that \
                     name"
                ));
            }
            let mut work = Vec::new();
            if let Some(session) = &session
                && !omit_session_work
            {
                work.push(Work {
                    kind: "session".to_owned(),
                    namespace: host.clone(),
                    id: session.clone(),
                });
            }
            if let Some(job) = job {
                work.push(Work {
                    kind: "job".to_owned(),
                    namespace: JOB_NAMESPACE.to_owned(),
                    id: job,
                });
            }
            if work.is_empty() {
                return Err(
                    "a registration names at least one work reference: give --session (or set \
                     AGENT_SESSION_ID), --job, or both"
                        .to_owned(),
                );
            }
            let request = client::Request {
                delegation,
                acceptance,
                host_name: host,
                host_version,
                purpose,
                work,
                session_id: session,
                replace_session,
                parent,
                predecessor,
                duties: duty,
                expires_at: now + Duration::minutes(expires_in_minutes),
            };
            print(&client::register(
                &home,
                &request,
                &key(idempotency_key, "register"),
                now,
            )?)
        }
        InstanceCommand::Renew {
            instance,
            expires_in_minutes,
            idempotency_key,
        } => print(&client::renew(
            &home,
            &instance,
            now + Duration::minutes(expires_in_minutes),
            &key(idempotency_key, "renew"),
            now,
        )?),
        InstanceCommand::Close {
            instance,
            outcome,
            evidence,
            idempotency_key,
        } => {
            let evidence = evidence
                .iter()
                .map(|raw| parse_evidence(raw))
                .collect::<Result<Vec<_>, _>>()?;
            print(&client::close(
                &home,
                &instance,
                &outcome,
                &evidence,
                &key(idempotency_key, "close"),
                now,
            )?)
        }
        InstanceCommand::Status {
            instance: Some(instance),
        } => print(&client::status(&home, &instance, now)?),
        InstanceCommand::Status { instance: None } => list(&home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_harness::instance::Operation;

    fn report(closure: Option<Value>) -> Report {
        let operation: Operation = serde_json::from_value(json!({
            "at": "2026-09-19T10:00:00Z",
            "operation": "status",
            "idempotency_key": "status-1",
            "outcome": "accepted",
        }))
        .expect("an operation");
        let mut retained: Retained = serde_json::from_value(json!({
            "version": 1,
            "hub": "http://127.0.0.1:1",
            "issuer": "registration.example",
            "instance": "inst_1",
            "revision": 2,
            "status": "closed",
            "work": [],
            "valid_from": "2026-09-19T09:00:00Z",
            "expires_at": "2026-09-19T11:00:00Z",
            "next_check": "2026-09-19T10:05:00Z",
            "online_validation_required": true,
            "registration": {},
            "accepted": {},
            "operations": [],
        }))
        .expect("a retained record");
        retained.closure = closure;
        Report {
            operation,
            retained: Some(retained),
            record: "instances/inst_1.json".into(),
        }
    }

    /// Establishes what the summary is built from, without a hub: the
    /// retained closure is printed whole, and a record without one prints
    /// null. It does not run the binary.
    #[test]
    fn the_summary_carries_the_retained_closure_or_null() {
        let closure = json!({
            "outcome": "completed",
            "closed_at": "2026-09-19T10:00:00Z",
            "receipt": {"binding": {"custody": "accepted"}},
        });
        assert_eq!(summary(&report(Some(closure.clone())))["closure"], closure);

        let open = summary(&report(None));
        assert!(open.as_object().expect("an object").contains_key("closure"));
        assert!(open["closure"].is_null());
    }
}
