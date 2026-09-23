//! The instance registration client: create, renew, close and read against
//! the hub's lifecycle routes (`docs/contracts/instance-registration.md`;
//! `docs/contracts/session-evidence.md` §Instance registration).
//!
//! Every request is signed with the key minted at enrolment under the
//! registration signature profile
//! (`commonmeasure_harness::identity::EdgeKey::sign_registration`), whose
//! pinned interoperability vector is reproduced in this crate's tests. A
//! retry keeps its idempotency key and signs a fresh nonce.
//!
//! Each command records what it did in `<home>/instances/<instance>.json`
//! (`commonmeasure_harness::instance::Retained`), owner readable only: the
//! accepted binding's revision, validity window and next check, and each
//! operation's outcome with the hub's stable reason on a refusal. Each
//! mutation's exact body is kept under `<home>/instances/attempts/` by its
//! idempotency key before it is sent, with what the last send of it did, so
//! a retry resends the same bytes and a registration that produced no
//! instance still leaves its refusal.
//!
//! A registration needs an enrolled, managed edge: the hub binds the policy
//! envelope it served this edge, and only a managed edge holds one. An edge
//! that is not enrolled, not managed or has activated no revision is refused
//! here, before any request, with what is missing named.
//!
//! An accepted binding is retained only after
//! `commonmeasure_harness::instance::verify_accepted` passes. That check does
//! not establish that the registration signer is one the operator trusts:
//! the edge pins no registration signer yet. A binding that fails it leaves
//! the instance recorded with the standing `binding_rejected`: the hub holds
//! an active instance, and the record is what lets `close` end it.
//!
//! A record is read again and written under
//! `commonmeasure_harness::instance::locked` after the hub has answered,
//! because the mediating server appends its checks to the same record.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use commonmeasure_harness::EnrolmentRecord;
use commonmeasure_harness::identity::Identity;
use commonmeasure_harness::instance::{
    Answer, BINDING_REJECTED, Call, Expected, Operation, Retained, Work, directory, exchange,
    locked, verify_accepted, write_owner_only,
};
use commonmeasure_harness::managed::{Deployment, State};

const BUDGET: Duration = commonmeasure_http::CLIENT_TIMEOUT;

/// What the operator asks to be registered.
#[derive(Debug, Clone)]
pub struct Request {
    /// The owner's delegation for this edge key, by its hub identifier.
    pub delegation: String,
    /// The durable service's custody acceptance, by its hub identifier.
    pub acceptance: String,
    pub host_name: String,
    pub host_version: String,
    pub purpose: String,
    pub work: Vec<Work>,
    /// The session whose records carry the instance reference.
    pub session_id: Option<String>,
    /// Register although the session already points at an active instance,
    /// and point the session at the new one. The earlier instance is not
    /// ended by this. Not part of the body, so a retry may change it.
    pub replace_session: bool,
    pub parent: Option<String>,
    pub predecessor: Option<String>,
    /// Reporting duty references the instance is to carry, in the hub's
    /// form (`owner:<content owner id>`). The hub binds one only under a
    /// custody acceptance made with reporting, and refuses one it cannot
    /// resolve.
    pub duties: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

/// One closing evidence reference and its digest.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub reference: String,
    pub digest: String,
}

/// What one command did.
#[derive(Debug)]
pub struct Report {
    pub operation: Operation,
    /// The retained record after the command, where an instance exists.
    pub retained: Option<Retained>,
    /// Where the operation was recorded.
    pub record: PathBuf,
}

impl Report {
    /// Whether the hub accepted the operation, first time or as a replay.
    pub fn accepted(&self) -> bool {
        matches!(self.operation.outcome.as_str(), "accepted" | "replayed")
    }
}

struct Edge {
    identity: Identity,
    hub: String,
    /// The organisation the hub enrolled this edge in, by its identifier.
    organisation: String,
}

/// The enrolled identity and hub, or what is missing.
fn edge(home: &Path) -> Result<Edge, String> {
    let Some(record) = EnrolmentRecord::load(home)? else {
        return Err(format!(
            "this edge is not enrolled with a hub, so it holds no key a registration can be \
             signed with: no enrolment record at {}. Run `commonmeasure connect <hub-url> \
             --token <token> --managed`.",
            EnrolmentRecord::path(home).display()
        ));
    };
    let identity = Identity::load(home)?;
    if let Some(reason) = identity.presented().unsigned {
        return Err(format!("{reason}. Nothing was sent."));
    }
    Ok(Edge {
        identity,
        hub: record.hub.trim_end_matches('/').to_owned(),
        organisation: record.organization.id,
    })
}

/// The applied managed policy: the envelope the hub served and the digest
/// this edge activated.
fn applied_policy(home: &Path) -> Result<Value, String> {
    if matches!(Deployment::read(home)?, Deployment::Local) {
        return Err(format!(
            "this edge is not managed: {} says the deployment is local, so there is no \
             hub-issued source policy for a registration to bind. Run `commonmeasure connect \
             <hub-url> --token <token> --managed`.",
            Deployment::path(home).display()
        ));
    }
    let Some(applied) = State::read(home)?.applied else {
        return Err(
            "this managed edge has activated no source-policy revision, so a registration has \
             no applied policy to bind. Run `commonmeasure policy sync` once the owner has \
             published a policy."
                .to_owned(),
        );
    };
    let path = State::last_known_good_path(home);
    let envelope: Value = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("{} is not a policy envelope: {error}", path.display()))?;
    let payload = &envelope["payload"];
    if payload["digest"].as_str() != Some(applied.digest.as_str())
        || payload["revision"].as_u64() != Some(applied.revision)
    {
        return Err(format!(
            "the retained envelope in {} is not the applied revision {} ({}), so the applied \
             policy cannot be stated truthfully. Run `commonmeasure policy sync`.",
            path.display(),
            applied.revision,
            applied.digest
        ));
    }
    Ok(json!({"envelope": envelope, "applied_digest": applied.digest}))
}

/// An idempotency key for one new operation, in the characters the hub
/// accepts. A retry of that operation reuses the key it was given.
pub fn fresh_key(operation: &str) -> String {
    format!("{operation}-{}", uuid::Uuid::new_v4())
}

/// The hub's rule for an idempotency key
/// in instance registration: 1 to 128 of
/// `A-Z a-z 0-9 - _`. The hub answers any other key
/// `401 invalid_registration_signature`, which reads as a signing fault, so
/// the key is held to the rule here before anything is kept or sent.
pub fn idempotency_key_ok(key: &str) -> bool {
    (1..=128).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn operation(name: &str, idempotency_key: &str, now: DateTime<Utc>) -> Operation {
    Operation {
        at: rfc3339(now),
        operation: name.to_owned(),
        idempotency_key: idempotency_key.to_owned(),
        outcome: String::new(),
        http_status: None,
        reason: None,
        request: None,
        revision: None,
        hub_time: None,
    }
}

/// Fill `operation` from what the exchange did and say whether the hub
/// accepted it. `success` is the status a first acceptance answers with; a
/// replay answers 200 with `replayed`.
fn settle(operation: &mut Operation, sent: &Result<Answer, String>, success: u16) -> bool {
    match sent {
        Err(reason) => {
            operation.outcome = "unreachable".to_owned();
            operation.reason = Some(reason.clone());
            false
        }
        Ok(answer) => {
            operation.http_status = Some(answer.status);
            operation.request = answer.body["request"].as_str().map(str::to_owned);
            let replayed = answer.body["replayed"] == json!(true);
            if answer.status == success || (answer.status == 200 && replayed) {
                operation.outcome = if replayed { "replayed" } else { "accepted" }.to_owned();
                true
            } else {
                // A signature the hub did not accept is not a ruling on the
                // instance, and its usual cause is this machine's clock.
                operation.outcome = answer.refusal_outcome().to_owned();
                operation.reason = Some(answer.reason());
                operation.hub_time = answer.hub_time();
                false
            }
        }
    }
}

fn body_of(request: &Request, policy: Value) -> Value {
    json!({
        "version": 1,
        "delegation": request.delegation,
        "host": {"name": request.host_name, "version": request.host_version},
        "work": request.work,
        "purpose": request.purpose,
        "parent": request.parent,
        "predecessor": request.predecessor,
        "policy": policy,
        "expires_at": rfc3339(request.expires_at),
        "acceptance": request.acceptance,
        // This client names no entitlement or shared limit.
        "requires_entitlements": false,
        "entitlements": [],
        "limits": [],
        "duties": request.duties,
    })
}

/// One mutation as it was first built, kept at
/// `<home>/instances/attempts/<idempotency-key>.json`. The hub binds an
/// idempotency key to the exact body bytes, so a retry must send the bytes
/// the first attempt sent: a command run again with the same key resends
/// this body and ignores what it was given, and the requested expiry a
/// second run would compute is never sent under the first run's key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Attempt {
    version: u32,
    hub: String,
    /// `register`, `renew` or `close`.
    operation: String,
    /// The instance a renewal or closure names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance: Option<String>,
    /// The exact body bytes, as text.
    body: String,
    work: Vec<Work>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// The instance a registration produced, once the hub has answered one,
    /// so a retry is known to be for the instance its session points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registered: Option<String>,
    /// What the most recent send of this attempt did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<Operation>,
}

impl Attempt {
    fn path(home: &Path, idempotency_key: &str) -> Result<PathBuf, String> {
        if !idempotency_key_ok(idempotency_key) {
            return Err(format!(
                "idempotency key {idempotency_key:?} is not one the hub accepts: use 1 to 128 \
                 ASCII letters, digits, `-` and `_`. Nothing was sent."
            ));
        }
        Ok(directory(home)
            .join("attempts")
            .join(format!("{idempotency_key}.json")))
    }

    /// The attempt already kept under `idempotency_key`, if there is one.
    fn kept(home: &Path, idempotency_key: &str) -> Result<Option<Self>, String> {
        let path = Self::path(home, idempotency_key)?;
        match std::fs::read(&path) {
            Ok(encoded) => serde_json::from_slice(&encoded)
                .map(Some)
                .map_err(|error| format!("{} is not an attempt: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    /// The attempt already kept under `idempotency_key`, or the one `build`
    /// makes, written before anything is sent.
    fn open(
        home: &Path,
        hub: &str,
        idempotency_key: &str,
        operation: &str,
        instance: Option<&str>,
        build: impl FnOnce() -> Result<Self, String>,
    ) -> Result<Self, String> {
        match Self::kept(home, idempotency_key)? {
            Some(kept) => {
                if kept.operation != operation || kept.instance.as_deref() != instance {
                    return Err(format!(
                        "idempotency key {idempotency_key} was used for {} {}; choose another \
                         key for this operation",
                        kept.operation,
                        kept.instance.as_deref().unwrap_or("of a new instance")
                    ));
                }
                // The kept body names the first hub's delegation and carries
                // its policy envelope; another hub can only refuse it, for a
                // reason that says nothing about the key.
                if kept.hub != hub {
                    return Err(format!(
                        "idempotency key {idempotency_key} was used for a request to {}, and \
                         this edge is now enrolled with {hub}; choose another key. Nothing was \
                         sent.",
                        kept.hub
                    ));
                }
                Ok(kept)
            }
            None => {
                let attempt = build()?;
                attempt.write(home, idempotency_key)?;
                Ok(attempt)
            }
        }
    }

    fn write(&self, home: &Path, idempotency_key: &str) -> Result<(), String> {
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise the attempt: {error}"))?;
        write_owner_only(&Self::path(home, idempotency_key)?, &encoded)
    }

    fn send(
        &mut self,
        home: &Path,
        edge: &Edge,
        path: &str,
        success: u16,
        now: DateTime<Utc>,
        idempotency_key: &str,
    ) -> Result<(Operation, Result<Answer, String>), String> {
        let sent = exchange(
            &edge.identity,
            &edge.hub,
            &Call {
                method: "POST",
                path,
                body: self.body.as_bytes(),
                idempotency_key,
            },
            now,
            BUDGET,
        );
        let mut operation = operation(&self.operation, idempotency_key, now);
        if settle(&mut operation, &sent, success)
            && self.operation != "close"
            && let Ok(answer) = &sent
        {
            let (key_id, origin) = edge
                .identity
                .signer()
                .map(|signer| (signer.key_id().to_owned(), signer.origin().to_owned()))
                .unwrap_or_default();
            // A binding that fails the check confers nothing here, although
            // the hub holds the revision it made; `reject` records that.
            if let Err(reason) = verify_accepted(
                &answer.body["accepted"],
                &Expected {
                    key_id: &key_id,
                    origin: &origin,
                    organisation: &edge.organisation,
                    registration: &self.registration()?,
                },
            ) {
                operation.outcome = BINDING_REJECTED.to_owned();
                operation.reason = Some(reason);
            }
        }
        if self.operation == "register"
            && (accepted(&operation) || operation.outcome == BINDING_REJECTED)
            && let Ok(answer) = &sent
            && let Some(instance) = answer.body["instance"].as_str()
        {
            self.registered = Some(instance.to_owned());
        }
        self.last = Some(operation.clone());
        self.write(home, idempotency_key)?;
        Ok((operation, sent))
    }

    fn registration(&self) -> Result<Value, String> {
        let body: Value = serde_json::from_str(&self.body).map_err(|error| error.to_string())?;
        Ok(match self.operation.as_str() {
            "renew" => body["registration"].clone(),
            _ => body,
        })
    }
}

fn accepted(operation: &Operation) -> bool {
    matches!(operation.outcome.as_str(), "accepted" | "replayed")
}

fn report(home: &Path, retained: Retained, operation: Operation) -> Result<Report, String> {
    Ok(Report {
        operation,
        record: Retained::path(home, &retained.instance)?,
        retained: Some(retained),
    })
}

/// Take a create or renew answer into the retained record. The record this
/// edge already holds is read under the lock, after the hub has answered, so
/// a check the mediating server recorded during the send is kept.
fn retain(
    home: &Path,
    edge: &Edge,
    attempt: &Attempt,
    answer: &Answer,
    mut operation: Operation,
    taken_from: Option<String>,
) -> Result<Report, String> {
    let mut retained = Retained::from_accepted(
        &edge.hub,
        &attempt.registration()?,
        attempt.work.clone(),
        attempt.session_id.clone(),
        &answer.body,
    )?;
    retained.session_taken_from = taken_from;
    locked(home, || {
        // The history of an instance this edge already holds is kept, and a
        // replay of an earlier operation does not put its earlier revision
        // back.
        if let Some(mut prior) = Retained::read(home, &retained.instance)? {
            if prior.revision > retained.revision {
                operation.revision = Some(prior.revision);
                prior.operations.push(operation.clone());
                prior.write(home)?;
                return report(home, prior, operation);
            }
            retained.operations = prior.operations;
            retained.closure = prior.closure;
            if retained.session_taken_from.is_none() {
                retained.session_taken_from = prior.session_taken_from;
            }
        }
        operation.revision = Some(retained.revision);
        retained.operations.push(operation.clone());
        retained.write(home)?;
        report(home, retained, operation)
    })
}

/// Record a registration or renewal the hub made whose binding failed
/// `verify_accepted`. The hub holds an active instance at the answered
/// revision whatever this edge thinks of the binding, so the record carries
/// the instance and that revision with the standing `binding_rejected`: no
/// crossing proceeds under it, and `close` can end it. A rejected
/// registration names no session, so nothing is attributed to it; a rejected
/// renewal leaves the session pointed at the record, and its crossings are
/// refused `instance_binding_rejected` until the operator closes it or
/// registers again.
fn reject(
    home: &Path,
    edge: &Edge,
    attempt: &Attempt,
    answer: &Answer,
    mut operation: Operation,
    idempotency_key: &str,
) -> Result<Report, String> {
    let Ok(rejected) = Retained::rejected(
        &edge.hub,
        &attempt.registration()?,
        attempt.work.clone(),
        &answer.body,
    ) else {
        // The answer names no instance, so there is nothing to close.
        return Ok(Report {
            operation,
            retained: None,
            record: Attempt::path(home, idempotency_key)?,
        });
    };
    locked(home, || {
        let mut held = match Retained::read(home, &rejected.instance)? {
            // A replay of an operation older than the revision held changes
            // nothing but the history.
            Some(prior) if prior.revision > rejected.revision => prior,
            Some(mut prior) => {
                if prior.status != "closed" {
                    prior.status = BINDING_REJECTED.to_owned();
                }
                prior.revision = rejected.revision;
                // `accepted` stays the binding that verified; the hub holds
                // a closure to the one it signed last.
                prior.rejected = Some(rejected.accepted);
                prior
            }
            None => rejected,
        };
        operation.revision = Some(held.revision);
        held.operations.push(operation.clone());
        held.write(home)?;
        report(home, held, operation)
    })
}

/// The instance `session` points at, where it is not the one this attempt
/// registered.
fn occupant(
    home: &Path,
    session: Option<&str>,
    registered: Option<&str>,
) -> Result<Option<Retained>, String> {
    let Some(session) = session else {
        return Ok(None);
    };
    Ok(Retained::for_session(home, session)?
        .filter(|held| registered != Some(held.instance.as_str())))
}

/// Register one instance for the work `request` names. Run again with the
/// same `idempotency_key`, this resends the first attempt's bytes.
///
/// A session that already points at an active instance is not registered
/// again unless `request.replace_session` is set: the second registration
/// would attribute the session's later records to a new instance while the
/// first stayed active at the hub with nothing pointing at it.
pub fn register(
    home: &Path,
    request: &Request,
    idempotency_key: &str,
    now: DateTime<Utc>,
) -> Result<Report, String> {
    let edge = edge(home)?;
    // Decided before the attempt is kept, so a refusal leaves no attempt
    // file under a key that was never sent.
    let kept = Attempt::kept(home, idempotency_key)?;
    let session = kept.as_ref().map_or(request.session_id.as_deref(), |kept| {
        kept.session_id.as_deref()
    });
    let registered = kept.as_ref().and_then(|kept| kept.registered.as_deref());
    let occupant = occupant(home, session, registered)?;
    if let Some(held) = &occupant
        && held.status == "active"
        && DateTime::parse_from_rfc3339(&held.expires_at).is_ok_and(|expires_at| expires_at > now)
        && !request.replace_session
    {
        return Err(format!(
            "session {} already points at instance {}, which is active until {}. Another \
             registration would attribute the session's later records to a new instance and \
             leave that one active at the hub. Close it (`commonmeasure instance close {} \
             --outcome <outcome>`), or pass --replace-session to register anyway. Nothing was \
             sent.",
            session.unwrap_or_default(),
            held.instance,
            held.expires_at,
            held.instance
        ));
    }
    let mut attempt = Attempt::open(home, &edge.hub, idempotency_key, "register", None, || {
        Ok(Attempt {
            version: 1,
            hub: edge.hub.clone(),
            operation: "register".to_owned(),
            instance: None,
            body: body_of(request, applied_policy(home)?).to_string(),
            work: request.work.clone(),
            session_id: request.session_id.clone(),
            registered: None,
            last: None,
        })
    })?;
    let (operation, sent) =
        attempt.send(home, &edge, "/api/v1/instances", 201, now, idempotency_key)?;
    match &sent {
        Ok(answer) if accepted(&operation) => retain(
            home,
            &edge,
            &attempt,
            answer,
            operation,
            occupant.map(|held| held.instance),
        ),
        Ok(answer) if operation.outcome == BINDING_REJECTED => {
            reject(home, &edge, &attempt, answer, operation, idempotency_key)
        }
        _ => Ok(Report {
            operation,
            retained: None,
            record: Attempt::path(home, idempotency_key)?,
        }),
    }
}

fn held(home: &Path, instance: &str) -> Result<Retained, String> {
    Retained::read(home, instance)?.ok_or_else(|| {
        format!(
            "no instance {instance} is recorded under {}",
            directory(home).display()
        )
    })
}

/// Apply `change` and record `operation` on the instance's record as it is
/// on disk now, under the lock, and answer the report.
fn conclude(
    home: &Path,
    instance: &str,
    operation: Operation,
    change: impl FnOnce(&mut Retained),
) -> Result<Report, String> {
    let retained = Retained::update(home, instance, |held| {
        change(held);
        held.operations.push(operation.clone());
    })?;
    report(home, retained, operation)
}

/// Renew `instance` to `expires_at`, restating the registration it holds
/// with the policy currently applied. The hub refuses a renewal that widens
/// the work, so this client never offers one.
pub fn renew(
    home: &Path,
    instance: &str,
    expires_at: DateTime<Utc>,
    idempotency_key: &str,
    now: DateTime<Utc>,
) -> Result<Report, String> {
    let edge = edge(home)?;
    let retained = held(home, instance)?;
    let mut attempt = Attempt::open(
        home,
        &edge.hub,
        idempotency_key,
        "renew",
        Some(instance),
        || {
            let mut registration = retained.registration.clone();
            registration["policy"] = applied_policy(home)?;
            registration["expires_at"] = json!(rfc3339(expires_at));
            Ok(Attempt {
                version: 1,
                hub: edge.hub.clone(),
                operation: "renew".to_owned(),
                instance: Some(instance.to_owned()),
                body: json!({
                    "expected_revision": retained.revision,
                    "registration": registration,
                })
                .to_string(),
                work: retained.work.clone(),
                session_id: retained.session_id.clone(),
                registered: None,
                last: None,
            })
        },
    )?;
    let path = format!("/api/v1/instances/{}/renew", retained.instance);
    let (operation, sent) = attempt.send(home, &edge, &path, 200, now, idempotency_key)?;
    match &sent {
        Ok(answer) if accepted(&operation) => {
            retain(home, &edge, &attempt, answer, operation, None)
        }
        Ok(answer) if operation.outcome == BINDING_REJECTED => {
            reject(home, &edge, &attempt, answer, operation, idempotency_key)
        }
        _ => conclude(home, &retained.instance, operation, |_| {}),
    }
}

/// Close `instance` with its outcome and final evidence digests. When the
/// hub is not reached the local outcome is `closure_unacknowledged`: the
/// record says the closure was attempted and not received, and expiry still
/// ends the authority.
pub fn close(
    home: &Path,
    instance: &str,
    outcome: &str,
    evidence: &[Evidence],
    idempotency_key: &str,
    now: DateTime<Utc>,
) -> Result<Report, String> {
    let edge = edge(home)?;
    let retained = held(home, instance)?;
    let mut attempt = Attempt::open(
        home,
        &edge.hub,
        idempotency_key,
        "close",
        Some(instance),
        || {
            Ok(Attempt {
                version: 1,
                hub: edge.hub.clone(),
                operation: "close".to_owned(),
                instance: Some(instance.to_owned()),
                body: json!({
                    "expected_revision": retained.revision,
                    "outcome": outcome,
                    "evidence": evidence
                        .iter()
                        .map(|item| json!({"reference": item.reference, "digest": item.digest}))
                        .collect::<Vec<_>>(),
                    // The hub reads a duty `delivered` only once the
                    // instance has ended, so at closure this edge knows
                    // every duty it registered as outstanding.
                    "outstanding_duties": retained.duties_at_closure(),
                })
                .to_string(),
                work: retained.work.clone(),
                session_id: retained.session_id.clone(),
                registered: None,
                last: None,
            })
        },
    )?;
    let path = format!("/api/v1/instances/{}/close", retained.instance);
    let (mut operation, sent) = attempt.send(home, &edge, &path, 200, now, idempotency_key)?;
    let mut closure = None;
    let mut current = Value::Null;
    if accepted(&operation)
        && let Ok(answer) = &sent
    {
        current = answer.body["current"].clone();
        closure = Some(json!({
            "outcome": answer.body["outcome"],
            "closed_at": answer.body["closed_at"],
            "receipt": answer.body["receipt"],
        }));
        operation.revision = answer.body["revision"].as_i64().or(Some(retained.revision));
    } else if sent.is_err() {
        operation.outcome = "closure_unacknowledged".to_owned();
    }
    conclude(home, &retained.instance, operation, |held| {
        if closure.is_some() {
            held.status = "closed".to_owned();
            held.closure = closure;
        }
        held.apply_duties(&current);
    })
}

/// Read `instance`'s current standing from the hub and retain it.
pub fn status(home: &Path, instance: &str, now: DateTime<Utc>) -> Result<Report, String> {
    let edge = edge(home)?;
    let retained = held(home, instance)?;
    let idempotency_key = fresh_key("status");
    let path = format!("/api/v1/instances/{}", retained.instance);
    let sent = exchange(
        &edge.identity,
        &edge.hub,
        &Call {
            method: "GET",
            path: &path,
            body: &[],
            idempotency_key: &idempotency_key,
        },
        now,
        BUDGET,
    );
    let mut operation = operation("status", &idempotency_key, now);
    let mut standing = Value::Null;
    if settle(&mut operation, &sent, 200)
        && let Ok(answer) = &sent
    {
        standing = answer.body.clone();
        operation.revision = standing["revision"].as_i64();
    }
    conclude(home, &retained.instance, operation, |held| {
        held.apply_standing(&standing);
    })
}
