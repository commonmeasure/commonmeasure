//! The registered instance as this edge holds it: the retained binding, the
//! signed exchange with the hub's lifecycle routes, and the validity check a
//! mediated crossing makes first (`docs/contracts/session-evidence.md`
//! §Instance registration; `docs/contracts/instance-registration.md`).
//!
//! The lifecycle client that registers, renews and closes is
//! `commonmeasure_relay::instance_registration`. What is here is the part the
//! mediating server needs, because the relay depends on this crate and not
//! the reverse: the file a registration leaves, the lookup from a session to
//! its instance, and the one signed read that establishes current standing.
//!
//! `<home>/instances/<instance>.json` holds one [`Retained`] record, owner
//! readable only. `<home>/instances/by-session/<session-id>` names the
//! instance a session's records are attributed to. A session with no such
//! pointer has no registration and nothing here changes what it does.
//! Writers of a record hold `<home>/instances/.lock` from reading it to
//! renaming it into place ([`locked`]), because the mediating server and the
//! CLI both append to one record's `operations`.
//!
//! The first route validates online with no grace, because unavailability
//! must never widen rights (owner decision, 17 September 2026, on
//! `docs/contracts/instance-registration.md` §Owner choices before
//! implementation). A binding whose
//! `online_validation_required` is set, or whose `next_check` has come, is
//! checked with the hub before each crossing; when that check cannot be made
//! the crossing is refused as unavailable. The cached source policy is ruled
//! on regardless: a registration widens nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::identity::{
    Identity, LifecycleRequest, REGISTRATION_SKEW_ALLOWANCE_SECS, RegistrationSignature,
    write_private,
};

/// The product token a lifecycle request carries.
pub const USER_AGENT: &str = concat!("commonmeasure-instance/", env!("CARGO_PKG_VERSION"));

/// How long the check before a crossing waits for the hub. A host is waiting
/// on the tool call, and a hub that has not answered in this time is recorded
/// as not reached.
pub const CHECK_BUDGET: Duration = Duration::from_secs(5);

const RECORD_VERSION: u32 = 1;

/// The `instance` member of a session record: which registration service
/// issued the instance, its identifier, and the binding revision in force
/// when the record was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub issuer: String,
    pub id: String,
    pub revision: i64,
}

/// One typed work reference, as the hub's registration body takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    /// `job`, `run` or `session`.
    pub kind: String,
    pub namespace: String,
    pub id: String,
}

/// What one lifecycle command did, kept in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub at: String,
    /// `register`, `renew`, `close`, `status` or `check`.
    pub operation: String,
    pub idempotency_key: String,
    /// `accepted`, `replayed`, `refused`, `unavailable`, `unreachable`,
    /// `binding_rejected` for an accepted binding that failed
    /// [`verify_accepted`], or `closure_unacknowledged` for a closure the hub
    /// did not receive.
    pub outcome: String,
    /// The HTTP status, absent when the hub was not reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// The hub's stable reason on a refusal, or why it was not reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The hub's request reference, for its audit trail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    /// The hub's clock as it answered, kept where the hub did not accept the
    /// request's signature, to compare with `at`, which is this edge's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_time: Option<String>,
}

/// `<home>/instances/<instance>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Retained {
    pub version: u32,
    /// The hub base URL the lifecycle requests were sent to and signed for.
    pub hub: String,
    /// The registration service identifier the binding names.
    pub issuer: String,
    pub instance: String,
    pub revision: i64,
    /// `active`, `expired`, `revoked` or `closed`, as the hub last said, or
    /// `binding_rejected` while the hub's current revision is one whose
    /// binding failed [`verify_accepted`]: the instance exists at the hub,
    /// confers nothing here and can be closed.
    pub status: String,
    pub work: Vec<Work>,
    /// The session whose records carry this instance's reference, where the
    /// registration named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The instance the session pointed at before this registration took
    /// its place. Registering does not end that instance at the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_taken_from: Option<String>,
    pub valid_from: String,
    pub expires_at: String,
    pub next_check: String,
    pub online_validation_required: bool,
    /// The registration body as sent, without the policy envelope, so a
    /// renewal can restate it.
    pub registration: Value,
    /// The signed accepted binding exactly as the hub returned it: the
    /// binding, its signed bytes, digest, signer evidence and signature.
    pub accepted: Value,
    /// The accepted document of the hub's current revision where a renewal's
    /// binding failed [`verify_accepted`] and `accepted` keeps the earlier
    /// binding that verified. Unverified: read only for what the hub will
    /// hold a closure to ([`Retained::duties_at_closure`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<Value>,
    /// The hub's answer to closure: outcome, time and custody receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<Value>,
    /// Each reporting duty as the hub last answered it: its reference, its
    /// state and the receiver acceptances behind it. The hub's statement,
    /// not this edge's; absent until an answer carried a duty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duties: Option<Value>,
    pub operations: Vec<Operation>,
}

/// Why a crossing under a registered instance does not go ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    /// `refused` when the authority is known to have ended; `unavailable`
    /// when it could not be established.
    pub outcome: &'static str,
    pub reason: String,
}

impl Stopped {
    fn refused(reason: impl Into<String>) -> Self {
        Self {
            outcome: "refused",
            reason: reason.into(),
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            outcome: "unavailable",
            reason: reason.into(),
        }
    }
}

/// The hub's answer to one lifecycle request.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The stable reason a refusal carries, or a description of an answer
    /// that carried none.
    pub fn reason(&self) -> String {
        self.body["reason"]
            .as_str()
            .map_or_else(|| format!("http_{}", self.status), str::to_owned)
    }

    /// Whether the hub turned the request away for its signature alone: the
    /// signature or its times did not verify, a second credential came with
    /// it, or the nonce had been seen. The hub stops there, before it looks
    /// at the key's standing or the instance, so the answer says nothing
    /// about the authority and is recorded `unavailable`. A clock ahead of
    /// the hub's by more than [`REGISTRATION_SKEW_ALLOWANCE_SECS`] is the
    /// ordinary cause. `401 edge_inactive` and `401 unknown_edge` are rulings
    /// on the key and stay refusals.
    pub fn signature_not_accepted(&self) -> bool {
        self.status == 401
            && matches!(
                self.body["reason"].as_str(),
                Some(
                    "invalid_registration_signature"
                        | "registration_signature_required"
                        | "nonce_reused"
                )
            )
    }

    /// `unavailable` for an answer that establishes nothing about the
    /// instance (a `503`, or a signature the hub did not accept), `refused`
    /// for any other answer that is not an acceptance.
    pub fn refusal_outcome(&self) -> &'static str {
        if self.status == 503 || self.signature_not_accepted() {
            "unavailable"
        } else {
            "refused"
        }
    }

    /// The hub's clock as it answered, where it did not accept the
    /// signature and said what time it has, so the operator can compare it
    /// with this edge's without another request.
    pub fn hub_time(&self) -> Option<String> {
        self.signature_not_accepted()
            .then(|| self.body["server_time"].as_str().map(str::to_owned))
            .flatten()
    }
}

pub fn directory(home: &Path) -> PathBuf {
    home.join("instances")
}

fn session_pointer(home: &Path, session_id: &str) -> Option<PathBuf> {
    crate::session::safe_session(session_id)
        .map(|session| directory(home).join("by-session").join(session))
}

/// An instance identifier names a file, so only a plain name is accepted.
fn safe_instance(instance: &str) -> Result<&str, String> {
    crate::session::safe_session(instance)
        .ok_or_else(|| format!("instance identifier {instance:?} is not a plain name"))
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The `duties` member of a hub answer, where it names at least one duty.
fn answered_duties(answer: &Value) -> Option<Value> {
    answer["duties"]
        .as_array()
        .filter(|duties| !duties.is_empty())
        .map(|duties| Value::Array(duties.clone()))
}

impl Retained {
    pub fn path(home: &Path, instance: &str) -> Result<PathBuf, String> {
        Ok(directory(home).join(format!("{}.json", safe_instance(instance)?)))
    }

    /// A retained record from the hub's answer to a registration or renewal.
    pub fn from_accepted(
        hub: &str,
        registration: &Value,
        work: Vec<Work>,
        session_id: Option<String>,
        answer: &Value,
    ) -> Result<Self, String> {
        let accepted = &answer["accepted"];
        let binding = &accepted["binding"];
        let text = |field: &str| -> Result<String, String> {
            binding[field]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("the accepted binding has no {field}"))
        };
        let mut registration = registration.clone();
        if let Some(policy) = registration["policy"].as_object_mut() {
            policy.remove("envelope");
        }
        Ok(Self {
            version: RECORD_VERSION,
            hub: hub.to_owned(),
            issuer: text("issuer")?,
            instance: text("instance")?,
            revision: binding["revision"]
                .as_i64()
                .ok_or("the accepted binding has no revision")?,
            status: answer["current"]["status"]
                .as_str()
                .unwrap_or("active")
                .to_owned(),
            work,
            session_id,
            session_taken_from: None,
            valid_from: text("valid_from")?,
            expires_at: text("expires_at")?,
            next_check: text("next_check")?,
            online_validation_required: binding["online_validation_required"]
                .as_bool()
                .unwrap_or(true),
            registration,
            accepted: accepted.clone(),
            rejected: None,
            closure: None,
            duties: answered_duties(&answer["current"]),
            operations: Vec::new(),
        })
    }

    /// A record of an instance the hub registered whose binding failed
    /// [`verify_accepted`]. It names no session, so no record is attributed
    /// to it and no crossing is checked against it; it exists so the
    /// operator can see the instance and close it. The instance and revision
    /// are the hub's answer's own members, read without trusting the binding.
    pub fn rejected(
        hub: &str,
        registration: &Value,
        work: Vec<Work>,
        answer: &Value,
    ) -> Result<Self, String> {
        let binding = &answer["accepted"]["binding"];
        let member = |field: &str| {
            if answer[field].is_null() {
                &binding[field]
            } else {
                &answer[field]
            }
        };
        let text = |field: &str| binding[field].as_str().unwrap_or_default().to_owned();
        let mut registration = registration.clone();
        if let Some(policy) = registration["policy"].as_object_mut() {
            policy.remove("envelope");
        }
        Ok(Self {
            version: RECORD_VERSION,
            hub: hub.to_owned(),
            issuer: text("issuer"),
            instance: safe_instance(
                member("instance")
                    .as_str()
                    .ok_or("the hub's answer names no instance")?,
            )?
            .to_owned(),
            revision: member("revision")
                .as_i64()
                .ok_or("the hub's answer names no revision")?,
            status: BINDING_REJECTED.to_owned(),
            work,
            session_id: None,
            session_taken_from: None,
            valid_from: text("valid_from"),
            expires_at: text("expires_at"),
            next_check: text("next_check"),
            online_validation_required: true,
            registration,
            accepted: answer["accepted"].clone(),
            rejected: None,
            closure: None,
            duties: None,
            operations: Vec::new(),
        })
    }

    /// The duties a closure reports outstanding: those the registration
    /// named. Under the standing `binding_rejected` the hub's current binding
    /// may restate fewer, and the hub refuses a closure that names a duty its
    /// binding does not (`unknown_duty_reference`), so only the named duties
    /// that binding restates are reported.
    pub fn duties_at_closure(&self) -> Vec<Value> {
        let named = self.registration["duties"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if self.status != BINDING_REJECTED {
            return named;
        }
        let current = self.rejected.as_ref().unwrap_or(&self.accepted);
        let restated = current["binding"]["registration"]["duties"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        named
            .into_iter()
            .filter(|duty| restated.contains(duty))
            .collect()
    }

    pub fn read(home: &Path, instance: &str) -> Result<Option<Self>, String> {
        let path = Self::path(home, instance)?;
        match std::fs::read(&path) {
            Ok(encoded) => serde_json::from_slice(&encoded)
                .map(Some)
                .map_err(|error| format!("{} is not an instance record: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    /// Write the record, owner readable only, and point its session at it.
    /// A closed instance's pointer is removed: later records of that session
    /// are not attributed to it. A caller that read the record first holds
    /// [`locked`] across both, or goes through [`Retained::update`].
    pub fn write(&self, home: &Path) -> Result<(), String> {
        let path = Self::path(home, &self.instance)?;
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise the instance record: {error}"))?;
        write_owner_only(&path, &encoded)?;
        let Some(pointer) = self
            .session_id
            .as_deref()
            .and_then(|session| session_pointer(home, session))
        else {
            return Ok(());
        };
        if self.status == "closed" {
            let held = std::fs::read_to_string(&pointer).unwrap_or_default();
            if held.trim() == self.instance {
                let _ = std::fs::remove_file(&pointer);
            }
            return Ok(());
        }
        write_owner_only(&pointer, self.instance.as_bytes())
    }

    /// The instance this session's records are attributed to, if any.
    pub fn for_session(home: &Path, session_id: &str) -> Result<Option<Self>, String> {
        let Some(pointer) = session_pointer(home, session_id) else {
            return Ok(None);
        };
        match std::fs::read_to_string(&pointer) {
            Ok(instance) => Self::read(home, instance.trim()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", pointer.display())),
        }
    }

    pub fn reference(&self) -> Reference {
        Reference {
            issuer: self.issuer.clone(),
            id: self.instance.clone(),
            revision: self.revision,
        }
    }

    /// Read the record again under the instances lock, apply `change` and
    /// write it, so an operation another process appended since this one
    /// read the record is kept. Answers the record as written.
    pub fn update(
        home: &Path,
        instance: &str,
        change: impl FnOnce(&mut Self),
    ) -> Result<Self, String> {
        locked(home, || {
            let mut held = Self::read(home, instance)?
                .ok_or_else(|| format!("no instance {instance} is recorded"))?;
            change(&mut held);
            held.write(home)?;
            Ok(held)
        })
    }

    /// Take the standing the hub's read answered: status, revision, expiry
    /// and the next required check. A standing older than the revision held
    /// is ignored: it was read before a renewal this record already has. A
    /// record whose binding was rejected stays so while the hub says
    /// `active`, because the hub's standing is not this edge's acceptance.
    pub fn apply_standing(&mut self, standing: &Value) {
        if standing["revision"]
            .as_i64()
            .is_some_and(|revision| revision < self.revision)
        {
            return;
        }
        if let Some(status) = standing["status"].as_str()
            && !(self.status == BINDING_REJECTED && status == "active")
        {
            self.status = status.to_owned();
        }
        if let Some(revision) = standing["revision"].as_i64() {
            self.revision = revision;
        }
        for (field, held) in [
            ("expires_at", &mut self.expires_at),
            ("next_check", &mut self.next_check),
        ] {
            if let Some(value) = standing[field].as_str() {
                *held = value.to_owned();
            }
        }
        if let Some(required) = standing["online_validation_required"].as_bool() {
            self.online_validation_required = required;
        }
        self.apply_duties(standing);
    }

    /// Follow the hub's latest statement of this instance's duties. An answer
    /// whose `duties` names none clears what an earlier answer stated, so the
    /// record never shows a state the hub no longer states. An answer with no
    /// `duties` array says nothing about them and leaves the record as it is.
    pub fn apply_duties(&mut self, answer: &Value) {
        if answer["duties"].is_array() {
            self.duties = answered_duties(answer);
        }
    }

    /// Whether work under this instance may proceed at `now`, asking the hub
    /// where the binding requires it. The outcome of a check that was made is
    /// appended to `operations` and the record rewritten; a failure to
    /// rewrite it does not turn a refusal into permission. An answer that
    /// only says the hub did not accept the request's signature
    /// ([`Answer::signature_not_accepted`]) is `unavailable`: the instance
    /// was not ruled on.
    pub fn check(
        &mut self,
        home: &Path,
        identity: &Identity,
        now: DateTime<Utc>,
    ) -> Result<(), Stopped> {
        if self.status != "active" {
            return Err(Stopped::refused(format!("instance_{}", self.status)));
        }
        let Some(expires_at) = parse_time(&self.expires_at) else {
            return Err(Stopped::unavailable(
                "instance_validity_unknown: the retained binding's expires_at is not a time",
            ));
        };
        if expires_at <= now {
            return Err(Stopped::refused("instance_expired"));
        }
        let due = self.online_validation_required
            || parse_time(&self.next_check).is_none_or(|next_check| next_check <= now);
        if !due {
            return Ok(());
        }
        let idempotency_key = format!("check-{}", uuid::Uuid::new_v4());
        let path = format!("/api/v1/instances/{}", self.instance);
        let sent = exchange(
            identity,
            &self.hub,
            &Call {
                method: "GET",
                path: &path,
                body: &[],
                idempotency_key: &idempotency_key,
            },
            now,
            CHECK_BUDGET,
        );
        let mut operation = Operation {
            at: rfc3339(now),
            operation: "check".to_owned(),
            idempotency_key,
            outcome: String::new(),
            http_status: None,
            reason: None,
            request: None,
            revision: None,
            hub_time: None,
        };
        let mut standing = None;
        let verdict = match sent {
            Err(reason) => {
                operation.outcome = "unreachable".to_owned();
                operation.reason = Some(reason.clone());
                Err(Stopped::unavailable(format!(
                    "instance_check_unavailable: {reason}"
                )))
            }
            Ok(answer) => {
                operation.http_status = Some(answer.status);
                operation.request = answer.body["request"].as_str().map(str::to_owned);
                match answer.status {
                    200 => {
                        self.apply_standing(&answer.body);
                        standing = Some(answer.body.clone());
                        operation.revision = Some(self.revision);
                        if self.status == "active" {
                            operation.outcome = "accepted".to_owned();
                            Ok(())
                        } else {
                            operation.outcome = "refused".to_owned();
                            operation.reason = Some(format!("instance_{}", self.status));
                            Err(Stopped::refused(format!("instance_{}", self.status)))
                        }
                    }
                    401 | 403 | 404 | 409 if !answer.signature_not_accepted() => {
                        operation.outcome = "refused".to_owned();
                        operation.reason = Some(answer.reason());
                        Err(Stopped::refused(answer.reason()))
                    }
                    _ => {
                        operation.outcome = "unavailable".to_owned();
                        operation.reason = Some(answer.reason());
                        operation.hub_time = answer.hub_time();
                        let clocks = operation.hub_time.as_ref().map_or_else(String::new, |hub| {
                            format!(
                                "; the hub's clock read {hub} and this edge's {}",
                                rfc3339(now)
                            )
                        });
                        Err(Stopped::unavailable(format!(
                            "instance_check_unavailable: the hub answered {} ({}){clocks}",
                            answer.status,
                            answer.reason()
                        )))
                    }
                }
            }
        };
        // The session record carries the refusal either way; this file is
        // the operator's view of the same fact. The record is read again
        // under the lock, so a renewal or closure the CLI wrote while the
        // hub was being asked keeps its entry and its revision.
        let standing = standing.unwrap_or(Value::Null);
        match Self::update(home, &self.instance, |held| {
            held.apply_standing(&standing);
            held.operations.push(operation.clone());
        }) {
            Ok(held) => *self = held,
            Err(_) => self.operations.push(operation),
        }
        verdict
    }
}

/// The standing of a record whose current revision's binding this edge
/// rejected.
pub const BINDING_REJECTED: &str = "binding_rejected";

/// Run `work` holding the exclusive lock on `<home>/instances/.lock`. Every
/// read of a record that leads to a write of it happens inside one call, so
/// two processes cannot each write back a copy that lacks the other's entry.
/// The lock is held for file reads and one rename, never across a request
/// to the hub. Not re-entrant: `work` must not call `locked` or
/// [`Retained::update`].
pub fn locked<T>(home: &Path, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let path = directory(home).join(".lock");
    write_owner_only_directory(&path)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    lock.lock()
        .map_err(|error| format!("cannot lock {}: {error}", path.display()))?;
    let done = work();
    let _ = lock.unlock();
    done
}

/// Remove every session pointer and answer the sessions that had one. The
/// records stay. `disconnect` calls this: the instances were registered under
/// the key being given up, the hub answers `404` for them to any later key,
/// and a session still pointed at one would have every crossing refused for a
/// reason the operator cannot act on.
pub fn retire_session_pointers(home: &Path) -> Result<Vec<String>, String> {
    let pointers = directory(home).join("by-session");
    let entries = match std::fs::read_dir(&pointers) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", pointers.display())),
    };
    locked(home, || {
        let mut retired = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            std::fs::remove_file(&path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
            // A staging file a crashed writer left is removed and not named.
            if let Some(session) = entry.file_name().to_str()
                && !session.ends_with(STAGING_SUFFIX)
            {
                retired.push(session.to_owned());
            }
        }
        retired.sort();
        Ok(retired)
    })
}

fn write_owner_only_directory(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

const STAGING_SUFFIX: &str = ".tmp";

/// Where `path` is staged before the rename: the whole file name, then a
/// value no other writer has, then [`STAGING_SUFFIX`]. `Path::with_extension`
/// would replace what follows the last dot, and session identifiers and
/// idempotency keys may contain dots, so `by-session/a.b` and `by-session/a.c`
/// would both stage through `a.tmp` and one writer could rename the other's
/// bytes into place.
fn staging_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(
        ".{}{STAGING_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));
    path.with_file_name(name)
}

/// Write `bytes` to `path` readable by the owner only, creating its
/// directory with owner-only access, through a temporary file and a rename.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_owner_only_directory(path)?;
    let staged = staging_path(path);
    write_private(&staged, bytes)?;
    std::fs::rename(&staged, path).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot write {}: {error}", path.display())
    })
}

/// The purpose a registration signer's evidence and binding must name.
const PURPOSE: &str = "instance-registration";

/// What an accepted binding is held to: this edge, and the request the
/// binding answers.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    /// The enrolled key's identifier.
    pub key_id: &'a str,
    /// The origin the hub returned at enrolment.
    pub origin: &'a str,
    /// The organisation identifier in the enrolment record.
    pub organisation: &'a str,
    /// The registration as it was sent, policy envelope included.
    pub registration: &'a Value,
}

/// Check an accepted binding as far as this edge can: the signature under
/// the key the answer names, the signed bytes being the binding, the binding
/// naming this edge, its organisation, the registration purpose and, as
/// issuer, the origin the hub gave at enrolment, and the binding agreeing
/// with what was sent: the policy revision, digest and applied digest, the
/// work references, every duty named, and an expiry no later than the one
/// asked for. The
/// retained record takes its revision and window from the binding and its
/// work from the request, so a binding that differed would leave a record
/// that misdescribes the authority. Whether the signer is one the operator
/// trusts is not established here, because the edge pins no registration
/// signer yet.
pub fn verify_accepted(accepted: &Value, expected: &Expected<'_>) -> Result<(), String> {
    let Expected {
        key_id,
        origin,
        organisation,
        registration,
    } = *expected;
    let text = |value: &Value, what: &str| -> Result<String, String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("the accepted binding has no {what}"))
    };
    let signed_bytes = text(&accepted["signed_bytes"], "signed_bytes")?;
    let key: [u8; 32] = STANDARD
        .decode(text(
            &accepted["signer"]["public_key"],
            "signer public_key",
        )?)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("the registration signer's public key is not 32 bytes of base64")?;
    let signature: [u8; 64] = STANDARD
        .decode(text(&accepted["signature"], "signature")?)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("the binding's signature is not 64 bytes of base64")?;
    VerifyingKey::from_bytes(&key)
        .map_err(|error| format!("the registration signer's key is not usable: {error}"))?
        .verify_strict(signed_bytes.as_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| "the accepted binding does not verify under the signer it names")?;
    let binding = &accepted["binding"];
    if serde_json::from_str::<Value>(&signed_bytes).ok().as_ref() != Some(binding) {
        return Err("the signed bytes are not the binding the answer shows".to_owned());
    }
    if accepted["signer"]["purpose"] != json!(PURPOSE) || binding["purpose"] != json!(PURPOSE) {
        return Err(format!(
            "the binding or its signer does not name the purpose {PURPOSE}"
        ));
    }
    if binding["edge_key_id"].as_str() != Some(key_id) {
        return Err("the accepted binding names another edge key".to_owned());
    }
    if binding["issuer"]
        .as_str()
        .map(|issuer| issuer.trim_end_matches('/'))
        != Some(origin.trim_end_matches('/'))
    {
        return Err(format!(
            "the accepted binding names issuer {} but the registration was sent to {}",
            binding["issuer"], origin
        ));
    }
    if binding["organisation"].as_str() != Some(organisation) {
        return Err(format!(
            "the accepted binding names organisation {} but this edge is enrolled in \
             {organisation}",
            binding["organisation"]
        ));
    }
    let sent = &registration["policy"];
    for (member, stated) in [
        ("revision", &sent["envelope"]["payload"]["revision"]),
        ("digest", &sent["envelope"]["payload"]["digest"]),
        ("applied_digest", &sent["applied_digest"]),
    ] {
        let bound = &binding["policy"][member];
        if stated.is_null() || bound != stated {
            return Err(format!(
                "the accepted binding's policy {member} is {bound} but the registration stated \
                 {stated}"
            ));
        }
    }
    let work = |value: &Value| serde_json::from_value::<Vec<Work>>(value.clone()).ok();
    let same_work = match (
        work(&binding["registration"]["work"]),
        work(&registration["work"]),
    ) {
        (Some(bound), Some(stated)) => {
            bound.len() == stated.len() && stated.iter().all(|item| bound.contains(item))
        }
        _ => false,
    };
    if !same_work {
        return Err(format!(
            "the accepted binding's work is {} but the registration named {}",
            binding["registration"]["work"], registration["work"]
        ));
    }
    // Every duty the request named must be bound, both in the registration
    // the binding restates and in the terms it carries. More is allowed: a
    // predecessor or parent adds duties the request did not name. A dropped
    // duty would leave a record naming a duty the signed binding does not
    // carry, which closure reports and the hub refuses as unknown.
    let restated: Vec<&str> = binding["registration"]["duties"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let carried: Vec<&str> = binding["duties"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|duty| duty["terms"]["reference"].as_str())
        .collect();
    for duty in registration["duties"].as_array().into_iter().flatten() {
        let bound = duty
            .as_str()
            .is_some_and(|duty| restated.contains(&duty) && carried.contains(&duty));
        if !bound {
            return Err(format!(
                "the registration named duty {duty} but the accepted binding does not carry it"
            ));
        }
    }
    let time = |value: &Value| value.as_str().and_then(parse_time);
    match (
        time(&binding["expires_at"]),
        time(&registration["expires_at"]),
    ) {
        (Some(bound), Some(asked)) if bound <= asked => Ok(()),
        _ => Err(format!(
            "the accepted binding expires at {} but the registration asked for {}",
            binding["expires_at"], registration["expires_at"]
        )),
    }
}

/// One lifecycle request before it is signed.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    pub method: &'a str,
    /// The route path, e.g. `/api/v1/instances`.
    pub path: &'a str,
    /// The exact body bytes; empty for a read.
    pub body: &'a [u8],
    pub idempotency_key: &'a str,
}

/// Send one lifecycle request signed under the registration profile with the
/// enrolled key. `Err` is the hub not reached or the request not signable; an
/// answer of any status is `Ok`.
pub fn exchange(
    identity: &Identity,
    hub: &str,
    call: &Call<'_>,
    now: DateTime<Utc>,
    budget: Duration,
) -> Result<Answer, String> {
    let Call {
        method,
        path,
        body,
        idempotency_key,
    } = *call;
    let Some(signer) = identity.signer() else {
        return Err(identity
            .presented()
            .unsigned
            .unwrap_or_else(|| "this edge holds no key to sign with".to_owned()));
    };
    // The hub rebuilds the signed target from its own public origin, which
    // is the origin it returned at enrolment. The request is sent to the URL
    // `connect` was given, which may be another name for the same hub.
    let target_uri = format!("{}{path}", signer.origin().trim_end_matches('/'));
    let url = format!("{}{path}", hub.trim_end_matches('/'));
    let signature: RegistrationSignature = signer.sign_registration(
        &LifecycleRequest {
            method,
            target_uri: &target_uri,
            body,
            idempotency_key,
        },
        now.timestamp() - REGISTRATION_SKEW_ALLOWANCE_SECS,
    )?;
    let mut request = if body.is_empty() {
        commonmeasure_http::Request::get(path)
    } else {
        commonmeasure_http::Request::post(path, body.to_vec(), "application/json")
    };
    request.method = method.to_owned();
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("Accept", "application/json");
    signature.attach(&mut request);
    let response = commonmeasure_http::send_with_timeout(&url, request, budget)
        .map_err(|error| format!("{error:#}"))?;
    Ok(Answer {
        status: response.status,
        body: serde_json::from_slice(&response.body).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(instance: &str, session: Option<&str>) -> Retained {
        Retained {
            version: RECORD_VERSION,
            hub: "https://hub.example".to_owned(),
            issuer: "https://hub.example".to_owned(),
            instance: instance.to_owned(),
            revision: 1,
            status: "active".to_owned(),
            work: Vec::new(),
            session_id: session.map(str::to_owned),
            session_taken_from: None,
            valid_from: "2026-09-18T00:00:00.000Z".to_owned(),
            expires_at: "2026-09-19T00:00:00.000Z".to_owned(),
            next_check: "2026-09-18T00:00:00.000Z".to_owned(),
            online_validation_required: true,
            registration: Value::Null,
            accepted: Value::Null,
            rejected: None,
            closure: None,
            duties: None,
            operations: Vec::new(),
        }
    }

    fn entry(key: String) -> Operation {
        Operation {
            at: "2026-09-18T00:00:00.000Z".to_owned(),
            operation: "check".to_owned(),
            idempotency_key: key,
            outcome: "accepted".to_owned(),
            http_status: Some(200),
            reason: None,
            request: None,
            revision: Some(1),
            hub_time: None,
        }
    }

    // Catches: two files whose names differ only after the last dot staged
    // through one temporary name, so a writer renames the other's bytes into
    // place; a staging file left behind.
    #[test]
    fn dotted_names_are_staged_apart_and_written_whole() {
        let home = tempfile::tempdir().unwrap();
        let pointers = home.path().join("instances/by-session");
        let (first, second) = (pointers.join("a.b"), pointers.join("a.c"));
        // The name the two used to share.
        assert_eq!(first.with_extension("tmp"), second.with_extension("tmp"));
        for path in [&first, &second] {
            let staged = staging_path(path);
            let name = staged.file_name().unwrap().to_str().unwrap().to_owned();
            let whole = path.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with(&format!("{whole}.")), "{name}");
            assert!(name.ends_with(STAGING_SUFFIX), "{name}");
            assert_ne!(staged, staging_path(path), "one name per write");
        }

        let writers: Vec<_> = [(first.clone(), "one"), (second.clone(), "two")]
            .into_iter()
            .map(|(path, instance)| {
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        write_owner_only(&path, instance.as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "two");
        let mut left: Vec<_> = std::fs::read_dir(&pointers)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["a.b", "a.c"]);
    }

    // Catches: a writer that read the record before another appended to it
    // writing its own copy back and dropping the other's entry.
    #[test]
    fn concurrent_writers_keep_every_operation() {
        let home = tempfile::tempdir().unwrap();
        record("i-1", Some("s-1")).write(home.path()).unwrap();
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                let home = home.path().to_path_buf();
                std::thread::spawn(move || {
                    for turn in 0..10 {
                        Retained::update(&home, "i-1", |held| {
                            held.operations.push(entry(format!("{writer}-{turn}")));
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let held = Retained::read(home.path(), "i-1").unwrap().unwrap();
        let mut keys: Vec<_> = held
            .operations
            .iter()
            .map(|operation| operation.idempotency_key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 80);
    }

    // Catches: a standing read before a renewal putting the earlier revision
    // back; the hub's `active` turning a rejected binding into an accepted
    // one.
    #[test]
    fn a_stale_standing_and_a_rejected_binding_are_not_overwritten() {
        let mut held = record("i-1", None);
        held.revision = 2;
        held.apply_standing(&json!({"status": "expired", "revision": 1}));
        assert_eq!((held.revision, held.status.as_str()), (2, "active"));

        held.status = BINDING_REJECTED.to_owned();
        held.apply_standing(&json!({"status": "active", "revision": 2}));
        assert_eq!(held.status, BINDING_REJECTED);
        held.apply_standing(&json!({"status": "revoked", "revision": 2}));
        assert_eq!(held.status, "revoked");
    }

    // Catches: a duty state kept after the hub stopped stating it; an answer
    // that says nothing about duties clearing what the hub last stated.
    #[test]
    fn retained_duties_follow_the_hubs_latest_answer() {
        let mut held = record("i-1", None);
        let stated = json!([{"reference": "owner:1", "state": "outstanding"}]);
        held.apply_standing(&json!({"status": "active", "revision": 1, "duties": stated}));
        assert_eq!(held.duties, Some(stated.clone()));

        held.apply_standing(&json!({"status": "active", "revision": 1}));
        assert_eq!(held.duties, Some(stated), "no duties member says nothing");

        held.apply_standing(&json!({"status": "active", "revision": 1, "duties": []}));
        assert_eq!(held.duties, None, "the hub states no duty");
    }

    // Catches: a signature the hub did not accept recorded as the authority
    // having ended; a ruling on the key recorded as unavailable.
    #[test]
    fn only_a_signature_refusal_is_unavailable_among_the_401s() {
        let answer = |status: u16, reason: &str| Answer {
            status,
            body: json!({"reason": reason, "server_time": "2026-09-18T00:00:00Z"}),
        };
        for reason in [
            "invalid_registration_signature",
            "registration_signature_required",
            "nonce_reused",
        ] {
            let refused = answer(401, reason);
            assert_eq!(refused.refusal_outcome(), "unavailable", "{reason}");
            assert_eq!(refused.hub_time().as_deref(), Some("2026-09-18T00:00:00Z"));
        }
        for (status, reason) in [
            (401, "edge_inactive"),
            (401, "unknown_edge"),
            (403, "operator_inactive"),
            (404, "not_found"),
            (409, "instance_terminal"),
        ] {
            let refused = answer(status, reason);
            assert_eq!(refused.refusal_outcome(), "refused", "{reason}");
            assert_eq!(refused.hub_time(), None);
        }
        assert_eq!(
            answer(503, "operator_delegation").refusal_outcome(),
            "unavailable"
        );
    }

    // Catches: `disconnect` leaving a session pointed at an instance the
    // next key cannot read, or removing the record with the pointer.
    #[test]
    fn retiring_the_pointers_keeps_the_records() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            retire_session_pointers(home.path()).unwrap(),
            Vec::<String>::new()
        );
        record("i-1", Some("s.1")).write(home.path()).unwrap();
        record("i-2", Some("s-2")).write(home.path()).unwrap();
        assert_eq!(
            retire_session_pointers(home.path()).unwrap(),
            ["s-2", "s.1"]
        );
        assert!(Retained::for_session(home.path(), "s.1").unwrap().is_none());
        assert!(Retained::read(home.path(), "i-1").unwrap().is_some());
    }
}
