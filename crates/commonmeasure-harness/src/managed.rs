//! Signed policy distribution: the deployment mode an edge chooses for
//! itself, the signed desired-policy envelope it accepts in `managed` mode,
//! and the last-known-good rule (`docs/contracts/policy-envelope.md`).
//!
//! The edge decides. The hub manages desired state, and the edge takes it
//! only under a mode chosen in a local file and from a signer pinned in
//! that file; nothing an envelope carries can change either. A desired
//! policy that fails any check leaves the policy already on disk in force,
//! records why, and never becomes a weaker policy the operator did not
//! write. A hub that is unreachable changes nothing: the policy last
//! accepted keeps governing every crossing, offline, for as long as it
//! takes.
//!
//! There is one policy engine. An accepted policy is activated by the same
//! loader and the same atomic save the console's editor uses, so a
//! distributed policy the runtime would refuse to load is refused before it
//! is written.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::declaration;
use crate::fleet::{EdgeIdentity, Management};
use crate::policy::{PolicyDocument, PolicyFile};
use commonmeasure_types::canonical::{canonical_digest, canonical_json};

/// The envelope format this edge accepts. It moves when a payload field is
/// added, removed or given a new meaning.
pub const ENVELOPE: &str = "contextops-policy-envelope/v1";

/// The `User-Agent` a management request carries. Distinct from the relay's,
/// so a hub can tell a policy fetch from a telemetry delivery in its logs.
pub const USER_AGENT: &str = concat!("commonmeasure-managed/", env!("CARGO_PKG_VERSION"));

/// The outcome recorded when this edge's key does not authenticate it: the
/// edge holds no key to sign with, so the hub is not asked at all, or the
/// hub answered 401 to the signed request, which is what a revoked key or a
/// closed organisation answers. Either way the fact is about this edge's
/// standing, not about the hub being out of reach.
const UNAUTHENTICATED: &str = "unauthenticated";

/// The one signing algorithm accepted.
const ED25519: &str = "ed25519";

/// The outcome recorded when the deployment or state file could not be
/// read, so no synchronisation was attempted. Distinct from `unreachable`:
/// the hub was never asked.
const UNAVAILABLE: &str = "unavailable";

/// How long a synchronisation run at session start may wait for the hub,
/// name resolution included. The host is waiting on the hook or the
/// server, and a host that gives a server ten seconds to answer its first
/// request would drop the mediated tools for the whole session if the hub
/// took that long, so the budget is well inside it; a hub that does not
/// answer in time is recorded as unreachable and the policy in force keeps
/// governing.
pub const SESSION_START_BUDGET: Duration = Duration::from_secs(3);

/// How long a synchronisation may wait for the hub where nothing else is
/// waiting on it: `policy sync` on demand and the refresh before a relay
/// run. The HTTP client's own budget.
pub const DEFAULT_BUDGET: Duration = commonmeasure_http::CLIENT_TIMEOUT;

/// `<home>/deployment.json`: the mode this edge runs in. Absent means
/// `local`. Unknown fields are load errors, as in every declaration this
/// runtime reads, because a misspelled `"signer"` must not load as a
/// managed edge pinned to nobody. The file is read through
/// [`DeploymentFile`], one flat record, because a tagged enum does not
/// refuse an unknown field beside a unit variant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "DeploymentFile", into = "DeploymentFile")]
pub enum Deployment {
    /// No remote policy is accepted and no management request is made.
    Local,
    /// One signing identity is pinned; a desired-policy envelope from it,
    /// for this organisation, is accepted after every check passes.
    Managed {
        signer: Signer,
        /// Where the desired envelope is fetched from. A management path of
        /// its own: never the telemetry receiver in `relay.json`, and no
        /// telemetry key is sent with the request.
        policy_url: String,
        /// The organisation the envelope must name.
        organisation: String,
    },
}

/// The file's own shape: `mode`, and the managed fields that must all be
/// present under `managed` and all absent under `local`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentFile {
    mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signer: Option<Signer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    organisation: Option<String>,
}

impl TryFrom<DeploymentFile> for Deployment {
    type Error = String;

    fn try_from(file: DeploymentFile) -> Result<Self, String> {
        match file.mode.as_str() {
            "local" => {
                if file.signer.is_some() || file.policy_url.is_some() || file.organisation.is_some()
                {
                    return Err(
                        "mode local names a signer, policy_url or organisation, which only \
                         managed mode reads"
                            .to_owned(),
                    );
                }
                Ok(Self::Local)
            }
            "managed" => Ok(Self::Managed {
                signer: file.signer.ok_or("managed mode needs a signer")?,
                policy_url: file.policy_url.ok_or("managed mode needs a policy_url")?,
                organisation: file
                    .organisation
                    .ok_or("managed mode needs an organisation")?,
            }),
            other => Err(format!("mode {other:?} is not local or managed")),
        }
    }
}

impl From<Deployment> for DeploymentFile {
    fn from(deployment: Deployment) -> Self {
        match deployment {
            Deployment::Local => Self {
                mode: "local".to_owned(),
                signer: None,
                policy_url: None,
                organisation: None,
            },
            Deployment::Managed {
                signer,
                policy_url,
                organisation,
            } => Self {
                mode: "managed".to_owned(),
                signer: Some(signer),
                policy_url: Some(policy_url),
                organisation: Some(organisation),
            },
        }
    }
}

/// The pinned signer: the hub's policy-signing key, named by its key id.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Signer {
    pub key_id: String,
    pub algorithm: String,
    /// The raw 32-byte Ed25519 public key, hex encoded.
    pub public_key: String,
}

/// Whether a `policy_url` is one this runtime fetches policy from. The
/// policy document crosses in the envelope: scope matchers, engagement
/// names, hosts. It travels under TLS, except to a loopback origin, which
/// stays on the machine. The rule is applied when a deployment is read and
/// when `connect --managed` pins one, so a URL the runtime would refuse at
/// synchronisation time is never written.
pub fn policy_url_accepted(policy_url: &str) -> Result<(), String> {
    match url::Url::parse(policy_url) {
        Ok(url) if url.scheme() == "https" => Ok(()),
        Ok(url)
            if url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("127.0.0.1") | Some("localhost") | Some("[::1]")
                ) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "policy_url {policy_url:?} must be an https URL, or http to a loopback origin"
        )),
    }
}

impl Deployment {
    pub fn path(home: &Path) -> PathBuf {
        home.join("deployment.json")
    }

    /// Read `<home>/deployment.json`. Absent is `local`; malformed is an
    /// error, never `local`, because an edge whose operator wrote
    /// `managed` and mistyped it must not quietly stop taking policy.
    pub fn read(home: &Path) -> Result<Self, String> {
        let source = Self::path(home);
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Local),
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let deployment: Self = serde_json::from_slice(&encoded)
            .map_err(|error| format!("{} is not a valid deployment: {error}", source.display()))?;
        if let Self::Managed {
            signer,
            policy_url,
            organisation,
        } = &deployment
        {
            if signer.key_id.is_empty() {
                return Err(format!("{}: the signer has no key_id", source.display()));
            }
            if signer.algorithm != ED25519 {
                return Err(format!(
                    "{}: signer algorithm {:?} is not supported; only {ED25519} is",
                    source.display(),
                    signer.algorithm
                ));
            }
            if decode_hex(&signer.public_key).is_none_or(|key| key.len() != 32) {
                return Err(format!(
                    "{}: the signer public_key must be the 32-byte Ed25519 key, hex encoded",
                    source.display()
                ));
            }
            if let Err(reason) = policy_url_accepted(policy_url) {
                return Err(format!("{}: {reason}", source.display()));
            }
            if organisation.is_empty() {
                return Err(format!("{}: organisation is empty", source.display()));
            }
        }
        Ok(deployment)
    }

    pub fn mode(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Managed { .. } => "managed",
        }
    }
}

/// Why an envelope was not accepted. `kind` is the stable word a status
/// reader groups on; `reason` is the sentence for the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub kind: &'static str,
    pub reason: String,
}

impl Rejection {
    fn new(kind: &'static str, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.reason, self.kind)
    }
}

/// An envelope every check accepted: what it carries, ready to activate.
#[derive(Debug, Clone)]
pub struct Verified {
    pub revision: u64,
    /// The digest the envelope names the revision by: over the policy as
    /// the envelope carries it (`docs/contracts/policy-envelope.md` §The
    /// envelope).
    pub digest: String,
    /// The digest of the same policy as the loader serialises it, which is
    /// what the file on disk digests to once the policy is activated. Equal
    /// to `digest` when the hub carried the loader's form; used to tell a
    /// policy still on disk unchanged from one edited locally.
    pub loader_digest: String,
    pub issued_at: String,
    pub expires_at: String,
    pub signer_key_id: String,
    pub policy: PolicyFile,
}

/// Verify `encoded` against the pinned signer, this organisation and this
/// edge, at `now`. The checks run in the order a forger meets them: the
/// format, then the signer and the signature over the payload, then what
/// the signed payload binds, then what it carries.
pub fn verify(
    encoded: &[u8],
    signer: &Signer,
    organisation: &str,
    edge: &EdgeIdentity,
    now: DateTime<Utc>,
) -> Result<Verified, Rejection> {
    let envelope: Value = serde_json::from_slice(encoded)
        .map_err(|error| Rejection::new("invalid_schema", format!("not JSON: {error}")))?;
    if envelope["envelope"] != json!(ENVELOPE) {
        return Err(Rejection::new(
            "invalid_schema",
            format!("envelope format {} is not {ENVELOPE}", envelope["envelope"]),
        ));
    }
    let payload = &envelope["payload"];
    if !payload.is_object() {
        return Err(Rejection::new(
            "invalid_schema",
            "the envelope has no payload object",
        ));
    }
    let signature = &envelope["signature"];
    if signature["algorithm"] != json!(ED25519) {
        return Err(Rejection::new(
            "wrong_signer",
            format!(
                "signature algorithm {} is not {ED25519}",
                signature["algorithm"]
            ),
        ));
    }
    let key_id = signature["key_id"].as_str().unwrap_or_default();
    if key_id != signer.key_id {
        return Err(Rejection::new(
            "wrong_signer",
            format!(
                "the envelope is signed by key {key_id:?}; this edge pins {:?}",
                signer.key_id
            ),
        ));
    }
    let public_key = decode_hex(&signer.public_key).unwrap_or_default();
    let value = signature["value"]
        .as_str()
        .and_then(decode_hex)
        .ok_or_else(|| Rejection::new("bad_signature", "the signature value is not hex"))?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &public_key)
        .verify(canonical_json(payload).as_bytes(), &value)
        .map_err(|_| {
            Rejection::new(
                "bad_signature",
                format!("the signature does not verify under key {key_id:?}"),
            )
        })?;

    // From here every field was signed by the pinned signer, and the
    // question is whether it was signed for this edge.
    if payload["organisation"].as_str() != Some(organisation) {
        return Err(Rejection::new(
            "wrong_organisation",
            format!(
                "the envelope names organisation {}; this edge belongs to {organisation:?}",
                payload["organisation"]
            ),
        ));
    }
    match (&payload["edge_key_id"], edge) {
        (Value::Null, _) => {}
        (Value::String(bound), EdgeIdentity::KeyId(own)) if bound == own => {}
        (Value::String(bound), EdgeIdentity::KeyId(own)) => {
            return Err(Rejection::new(
                "wrong_edge",
                format!("the envelope is bound to edge {bound:?}; this edge is {own:?}"),
            ));
        }
        (Value::String(bound), EdgeIdentity::Unknown { reason }) => {
            return Err(Rejection::new(
                "wrong_edge",
                format!(
                    "the envelope is bound to edge {bound:?} and this edge has no key id ({reason})"
                ),
            ));
        }
        (other, _) => {
            return Err(Rejection::new(
                "invalid_schema",
                format!("edge_key_id must be a key id or null, not {other}"),
            ));
        }
    }
    let revision = payload["revision"]
        .as_u64()
        .ok_or_else(|| Rejection::new("invalid_schema", "revision must be an unsigned integer"))?;
    let issued_at = parse_time(&payload["issued_at"], "issued_at")?;
    let expires_at = parse_time(&payload["expires_at"], "expires_at")?;
    if expires_at <= issued_at {
        return Err(Rejection::new(
            "invalid_schema",
            "expires_at is not after issued_at",
        ));
    }
    if issued_at > now {
        return Err(Rejection::new(
            "not_yet_valid",
            format!(
                "the envelope is issued at {}, which is after now",
                issued_at.to_rfc3339_opts(SecondsFormat::Secs, true)
            ),
        ));
    }
    if expires_at <= now {
        return Err(Rejection::new(
            "expired",
            format!(
                "the envelope expired at {}",
                expires_at.to_rfc3339_opts(SecondsFormat::Secs, true)
            ),
        ));
    }
    // The digest is checked over the policy exactly as the envelope carries
    // it, before the policy is parsed. A hub then names a revision by the
    // document it published and never has to reproduce this loader's
    // serialisation, which would drift whenever the policy schema changed.
    // A policy the loader refuses is still refused, one check later.
    let digest = canonical_digest(&payload["policy"]);
    if payload["digest"].as_str() != Some(digest.as_str()) {
        return Err(Rejection::new(
            "digest_mismatch",
            format!(
                "the policy as carried digests to {digest} and the envelope claims {}",
                payload["digest"]
            ),
        ));
    }
    let policy: PolicyFile =
        serde_json::from_value(payload["policy"].clone()).map_err(|error| {
            Rejection::new(
                "invalid_policy",
                format!("the policy does not load: {error}"),
            )
        })?;
    let loader_digest = canonical_digest(&serde_json::to_value(&policy).unwrap_or(Value::Null));
    Ok(Verified {
        revision,
        digest,
        loader_digest,
        issued_at: issued_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        signer_key_id: key_id.to_owned(),
        policy,
    })
}

fn parse_time(value: &Value, field: &str) -> Result<DateTime<Utc>, Rejection> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
        .ok_or_else(|| Rejection::new("invalid_schema", format!("{field} is not an RFC 3339 time")))
}

/// `<home>/managed/state.json`: what this edge activated and what its last
/// synchronisation did. The last accepted envelope itself is kept beside it
/// as `last-known-good.json`, verbatim, so a reviewer can re-verify it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<Applied>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<Sync>,
}

/// The desired revision whose policy is the one on disk.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub revision: u64,
    pub digest: String,
    pub issued_at: String,
    pub expires_at: String,
    pub activated_at: String,
    pub signer_key_id: String,
}

impl Applied {
    /// When the applied envelope expired, if it has: `expires_at` once it
    /// is not after `now`. An expired envelope keeps enforcing the policy it
    /// carried; expiry is a condition for accepting a new envelope, never a
    /// reason to relax the one in force, so what expiry changes is only what
    /// the record says (`docs/contracts/policy-envelope.md` §Cadence and
    /// staleness).
    pub fn stale_since(&self, now: DateTime<Utc>) -> Option<String> {
        let expires_at = DateTime::parse_from_rfc3339(&self.expires_at)
            .ok()?
            .with_timezone(&Utc);
        (expires_at <= now).then(|| self.expires_at.clone())
    }
}

/// One synchronisation's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sync {
    pub at: String,
    /// `accepted`, `reapplied`, `already_applied`, `rejected`,
    /// `unreachable` or `unauthenticated`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl State {
    fn directory(home: &Path) -> PathBuf {
        home.join("managed")
    }

    fn path(home: &Path) -> PathBuf {
        Self::directory(home).join("state.json")
    }

    pub fn last_known_good_path(home: &Path) -> PathBuf {
        Self::directory(home).join("last-known-good.json")
    }

    pub fn read(home: &Path) -> Result<Self, String> {
        let source = Self::path(home);
        match std::fs::read(&source) {
            Ok(encoded) => serde_json::from_slice(&encoded).map_err(|error| {
                format!("{} is not a valid managed state: {error}", source.display())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("cannot read {}: {error}", source.display())),
        }
    }

    fn write(&self, home: &Path) -> Result<(), String> {
        let directory = Self::directory(home);
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise managed state: {error}"))?;
        declaration::replace(&Self::path(home), &encoded)
    }

    /// The fields the fleet-status document reports (`desired`).
    fn desired(&self) -> Value {
        match &self.last_sync {
            Some(sync) => json!({
                "revision": sync.revision,
                "digest": sync.digest,
                "learned_at": sync.at,
                "outcome": sync.outcome,
                "reason": sync.reason,
            }),
            None => Value::Null,
        }
    }
}

/// What one synchronisation did, for the command that ran it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub policy_url: String,
    pub sync: Sync,
    /// The revision in force after this synchronisation, whatever it did.
    pub applied: Option<Applied>,
    /// When the applied envelope expired, if it has by the end of this
    /// synchronisation. A successful synchronisation clears it by
    /// activating or refreshing an envelope that has not expired.
    pub stale_since: Option<String>,
}

impl SyncReport {
    /// Whether the desired policy is the one in force after this run.
    pub fn converged(&self) -> bool {
        matches!(
            self.sync.outcome.as_str(),
            "accepted" | "reapplied" | "already_applied"
        )
    }

    /// Whether there is nothing to tell an operator: the desired policy was
    /// already in force and its envelope has not expired.
    pub fn fresh(&self) -> bool {
        self.sync.outcome == "already_applied" && self.stale_since.is_none()
    }

    /// The fields a session record of this synchronisation carries
    /// (`docs/contracts/session-evidence.md` §Policy synchronisation).
    pub fn to_record(&self) -> Value {
        json!({
            "policy_url": self.policy_url,
            "outcome": self.sync.outcome,
            "revision": self.sync.revision,
            "digest": self.sync.digest,
            "reason": self.sync.reason,
            "applied": self.applied.as_ref().map(|applied| json!({
                "revision": applied.revision,
                "digest": applied.digest,
                "expires_at": applied.expires_at,
            })),
            "stale_since": self.stale_since,
        })
    }

    /// The record for a synchronisation that could not be attempted: the
    /// deployment or state file did not load. Nothing was asked of the hub
    /// and the policy on disk keeps governing.
    pub fn unavailable(reason: &str) -> Value {
        json!({
            "policy_url": null,
            "outcome": UNAVAILABLE,
            "revision": null,
            "digest": null,
            "reason": reason,
            "applied": null,
            "stale_since": null,
        })
    }
}

/// Whether `<home>/deployment.json` puts this edge under managed policy.
/// `Err` is a deployment file that does not load, which is reported as
/// that wherever the mode is reported and is never read as `local`.
pub fn is_managed(home: &Path) -> Result<bool, String> {
    Ok(matches!(
        Deployment::read(home)?,
        Deployment::Managed { .. }
    ))
}

/// Fetch the desired envelope, verify it, and activate it or keep the
/// policy already in force.
///
/// `Err` is a refusal to try: a local edge, or a deployment or state file
/// this runtime cannot read. Everything the hub does or fails to do is an
/// `Ok` report with its outcome, recorded in the state file so the next
/// status document carries it.
pub fn sync(home: &Path, edge: &EdgeIdentity, now: DateTime<Utc>) -> Result<SyncReport, String> {
    sync_within(home, edge, now, DEFAULT_BUDGET)
}

/// [`sync`] with the time the request may take bounded by `budget`: the
/// session-start form, where a host is waiting.
pub fn sync_within(
    home: &Path,
    edge: &EdgeIdentity,
    now: DateTime<Utc>,
    budget: Duration,
) -> Result<SyncReport, String> {
    let Deployment::Managed {
        signer,
        policy_url,
        organisation,
    } = Deployment::read(home)?
    else {
        return Err(format!(
            "deployment mode is local: no management request is made. Write {} with \
             \"mode\": \"managed\", a pinned signer and a policy_url to accept remote policy.",
            Deployment::path(home).display()
        ));
    };
    let at = now.to_rfc3339_opts(SecondsFormat::Millis, true);

    // A management request on its own path, carrying no telemetry key and no
    // credential of its own: the hub authenticates this edge by the signature
    // it makes with its enrolled key, the same key and the same construction
    // the mediated fetch signs with (`docs/contracts/policy-envelope.md`).
    let identity = crate::identity::Identity::load(home)?;
    let Some(edge_key) = identity.signer() else {
        let reason = identity
            .presented()
            .unsigned
            .unwrap_or_else(|| "this edge holds no key to sign with".to_owned());
        // No request is made. The hub would refuse an unsigned one, and an
        // edge reporting "unreachable" for its own missing key would send the
        // operator to look at the hub.
        return conclude(
            home,
            &policy_url,
            State::read(home)?,
            Sync {
                at,
                outcome: UNAUTHENTICATED.to_owned(),
                revision: None,
                digest: None,
                reason: Some(format!(
                    "{reason}. The policy endpoint authenticates the edge by its enrolled key, \
                     so no request was made; run `commonmeasure connect` to enrol this edge."
                )),
            },
            now,
        );
    };
    let mut request = commonmeasure_http::Request::get("/");
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("Accept", "application/json");
    if let Err(reason) = edge_key.sign_request(&policy_url, &mut request, &[], now.timestamp()) {
        return conclude(
            home,
            &policy_url,
            State::read(home)?,
            Sync {
                at,
                outcome: UNAUTHENTICATED.to_owned(),
                revision: None,
                digest: None,
                reason: Some(format!("the request could not be signed: {reason}")),
            },
            now,
        );
    }
    let sent = send_within(&policy_url, request, budget);
    // The policy file is held exclusively from here to the save, against
    // the console's editor and another synchronisation, so a read, a
    // comparison and a replacement are one step.
    let _lock = PolicyDocument::lock(home)
        .map_err(|refused| format!("cannot hold the policy file: {refused}"))?;
    let mut state = State::read(home)?;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            return conclude(
                home,
                &policy_url,
                state,
                Sync {
                    at,
                    outcome: "unreachable".to_owned(),
                    revision: None,
                    digest: None,
                    reason: Some(format!("{error:#}")),
                },
                now,
            );
        }
    };
    // A 401 is the hub refusing this edge's key: what a revoked key or a
    // closed organisation answers. That is this edge's standing, not a hub
    // out of reach, and an operator sent to look at the hub for an
    // "unreachable" endpoint would find it answering.
    if response.status == 401 {
        return conclude(
            home,
            &policy_url,
            state,
            Sync {
                at,
                outcome: UNAUTHENTICATED.to_owned(),
                revision: None,
                digest: None,
                reason: Some(format!(
                    "the policy URL answered 401 {}: the hub does not accept this edge's key, \
                     which is what a revoked key or a closed organisation answers; the next \
                     relay run reports the key's standing",
                    response.reason
                )),
            },
            now,
        );
    }
    // An answer that is not an envelope, including the hub's 404 before any
    // revision is published, is the hub not reached rather than a desired
    // policy refused.
    if response.status != 200 {
        return conclude(
            home,
            &policy_url,
            state,
            Sync {
                at,
                outcome: "unreachable".to_owned(),
                revision: None,
                digest: None,
                reason: Some(format!(
                    "the policy URL answered {} {}",
                    response.status, response.reason
                )),
            },
            now,
        );
    }
    let verified = match verify(&response.body, &signer, &organisation, edge, now) {
        Ok(verified) => verified,
        Err(rejection) => {
            // A revision the envelope may name is not trusted enough to
            // record as desired: nothing before the signature check is
            // known to come from the signer, and after it the envelope was
            // not for this edge.
            return conclude(
                home,
                &policy_url,
                state,
                Sync {
                    at,
                    outcome: "rejected".to_owned(),
                    revision: None,
                    digest: None,
                    reason: Some(rejection.to_string()),
                },
                now,
            );
        }
    };

    // Rollback and reuse. An earlier revision is refused however valid its
    // signature, so a captured old envelope cannot move an edge backwards;
    // the same revision with other content is refused for the same reason.
    if let Some(applied) = &state.applied {
        if verified.revision < applied.revision {
            return conclude(
                home,
                &policy_url,
                state.clone(),
                Sync {
                    at,
                    outcome: "rejected".to_owned(),
                    revision: Some(verified.revision),
                    digest: Some(verified.digest.clone()),
                    reason: Some(format!(
                        "rollback refused: revision {} is earlier than the applied revision {}",
                        verified.revision, applied.revision
                    )),
                },
                now,
            );
        }
        if verified.revision == applied.revision && verified.digest != applied.digest {
            return conclude(
                home,
                &policy_url,
                state.clone(),
                Sync {
                    at,
                    outcome: "rejected".to_owned(),
                    revision: Some(verified.revision),
                    digest: Some(verified.digest.clone()),
                    reason: Some(format!(
                        "revision {} was applied with digest {} and is now offered with {}; a \
                         revision names one policy",
                        applied.revision, applied.digest, verified.digest
                    )),
                },
                now,
            );
        }
        if verified.revision == applied.revision {
            let on_disk = PolicyDocument::read(home)
                .ok()
                .and_then(|document| document.digest());
            if on_disk.as_deref() == Some(verified.loader_digest.as_str()) {
                // The same revision reissued with a later expiry is the hub
                // renewing it. The window and the envelope are refreshed,
                // which is what clears a stale record; the policy file is
                // not touched, because it is already the policy.
                let mut state = state.clone();
                if let Some(applied) = state.applied.as_mut()
                    && (applied.expires_at != verified.expires_at
                        || applied.issued_at != verified.issued_at)
                {
                    declaration::replace(&State::last_known_good_path(home), &response.body)?;
                    applied.issued_at.clone_from(&verified.issued_at);
                    applied.expires_at.clone_from(&verified.expires_at);
                }
                return conclude(
                    home,
                    &policy_url,
                    state,
                    Sync {
                        at,
                        outcome: "already_applied".to_owned(),
                        revision: Some(verified.revision),
                        digest: Some(verified.digest.clone()),
                        reason: None,
                    },
                    now,
                );
            }
        }
    }
    let reapplied = state
        .applied
        .as_ref()
        .is_some_and(|applied| applied.revision == verified.revision);

    // The loader's own validity rule, before anything is written, so a
    // policy the runtime would refuse is refused here.
    if let Err(error) = PolicyDocument::validate(&verified.policy) {
        return conclude(
            home,
            &policy_url,
            state,
            Sync {
                at,
                outcome: "rejected".to_owned(),
                revision: Some(verified.revision),
                digest: Some(verified.digest.clone()),
                reason: Some(format!(
                    "the policy does not load (invalid_policy): {error}"
                )),
            },
            now,
        );
    }
    // The envelope and the state are written before the policy file is
    // replaced. A failure here leaves the policy in force unchanged; a
    // failure of the replacement itself leaves the state naming a revision
    // whose policy is not on disk, which the next synchronisation reapplies
    // and the status document reports as divergent until then.
    std::fs::create_dir_all(State::directory(home))
        .map_err(|error| format!("create {}: {error}", State::directory(home).display()))?;
    declaration::replace(&State::last_known_good_path(home), &response.body)?;
    state.applied = Some(Applied {
        revision: verified.revision,
        digest: verified.digest.clone(),
        issued_at: verified.issued_at.clone(),
        expires_at: verified.expires_at.clone(),
        activated_at: at.clone(),
        signer_key_id: verified.signer_key_id.clone(),
    });
    state.last_sync = Some(Sync {
        at,
        outcome: if reapplied { "reapplied" } else { "accepted" }.to_owned(),
        revision: Some(verified.revision),
        digest: Some(verified.digest.clone()),
        reason: None,
    });
    state.write(home)?;
    PolicyDocument::save(home, &verified.policy).map_err(|error| {
        format!(
            "revision {} is recorded as applied but {} could not be replaced: {error}; the next \
             synchronisation reapplies it",
            verified.revision,
            home.join("policy.json").display()
        )
    })?;
    Ok(SyncReport {
        policy_url,
        sync: state.last_sync.clone().expect("recorded above"),
        stale_since: state
            .applied
            .as_ref()
            .and_then(|applied| applied.stale_since(now)),
        applied: state.applied,
    })
}

/// Send one request with name resolution inside the budget as well as the
/// exchange. The standard resolver has no timeout of its own, so it runs on
/// a thread that is left behind if it has not answered in time; a resolver
/// that does not answer within the budget is the hub not reached, recorded
/// like any other unreachable outcome.
fn send_within(
    url: &str,
    request: commonmeasure_http::Request,
    budget: Duration,
) -> Result<commonmeasure_http::Response, String> {
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let target = url.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(commonmeasure_http::resolve(&target));
    });
    let addresses = match rx.recv_timeout(budget) {
        Ok(resolved) => resolved.map_err(|error| format!("{error:#}"))?,
        Err(_) => {
            return Err(format!(
                "name resolution for {url} did not complete within {} s: timed out",
                budget.as_secs_f32()
            ));
        }
    };
    let remaining = budget.saturating_sub(started.elapsed());
    commonmeasure_http::send_to(url, &addresses, request, remaining)
        .map_err(|error| format!("{error:#}"))
}

/// Record the outcome and report it. The state file is the one place the
/// next status document reads, so a synchronisation that is not recorded
/// did not, for the fleet, happen.
fn conclude(
    home: &Path,
    policy_url: &str,
    mut state: State,
    sync: Sync,
    now: DateTime<Utc>,
) -> Result<SyncReport, String> {
    state.last_sync = Some(sync.clone());
    state.write(home)?;
    Ok(SyncReport {
        policy_url: policy_url.to_owned(),
        sync,
        stale_since: state
            .applied
            .as_ref()
            .and_then(|applied| applied.stale_since(now)),
        applied: state.applied,
    })
}

/// What the fleet-status document reports about this edge's management:
/// the mode, the last desired revision learned of, the revision applied
/// and, at `now`, whether its envelope has expired. A deployment or state
/// file this runtime cannot read is reported as that, never as a local
/// edge.
pub fn management(home: &Path, now: DateTime<Utc>) -> Management {
    let deployment = match Deployment::read(home) {
        Ok(deployment) => deployment,
        Err(error) => {
            return Management {
                mode: UNAVAILABLE.to_owned(),
                desired: json!({"unavailable": error}),
                applied_revision: None,
                applied_digest: None,
                applied_edited: None,
                applied_expires_at: None,
                stale_since: None,
            };
        }
    };
    match deployment {
        Deployment::Local => Management::local(),
        Deployment::Managed { .. } => match State::read(home) {
            Ok(state) => Management {
                mode: "managed".to_owned(),
                desired: state.desired(),
                applied_revision: state.applied.as_ref().map(|applied| applied.revision),
                applied_digest: state.applied.as_ref().map(|applied| applied.digest.clone()),
                applied_edited: state
                    .applied
                    .as_ref()
                    .and_then(|_| edited_since_activation(home)),
                applied_expires_at: state
                    .applied
                    .as_ref()
                    .map(|applied| applied.expires_at.clone()),
                stale_since: state
                    .applied
                    .as_ref()
                    .and_then(|applied| applied.stale_since(now)),
            },
            Err(error) => Management {
                mode: "managed".to_owned(),
                desired: json!({"unavailable": error}),
                applied_revision: None,
                applied_digest: None,
                applied_edited: None,
                applied_expires_at: None,
                stale_since: None,
            },
        },
    }
}

/// Whether the policy on disk has moved away from the applied revision's
/// policy: the loader's form of the policy in the kept envelope digests
/// differently from the file. `None` where the kept envelope or the file
/// cannot be read, which the status document reports as not known rather
/// than as either answer.
fn edited_since_activation(home: &Path) -> Option<bool> {
    let kept: Value =
        serde_json::from_slice(&std::fs::read(State::last_known_good_path(home)).ok()?).ok()?;
    let policy: PolicyFile = serde_json::from_value(kept["payload"]["policy"].clone()).ok()?;
    let activated = canonical_digest(&serde_json::to_value(&policy).ok()?);
    let on_disk = PolicyDocument::read(home).ok()?.digest()?;
    Some(on_disk != activated)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;

    struct Hub {
        pair: ring::signature::Ed25519KeyPair,
        signer: Signer,
    }

    fn hub(key_id: &str) -> Hub {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let signer = Signer {
            key_id: key_id.to_owned(),
            algorithm: ED25519.to_owned(),
            public_key: encode_hex(pair.public_key().as_ref()),
        };
        Hub { pair, signer }
    }

    impl Hub {
        fn envelope(&self, mut payload: Value) -> Vec<u8> {
            if payload.get("digest").is_none() {
                // The hub digests the policy as it publishes it, in
                // whatever form the owner wrote it.
                payload["digest"] = json!(canonical_digest(&payload["policy"]));
            }
            let signature = self.pair.sign(canonical_json(&payload).as_bytes());
            serde_json::to_vec(&json!({
                "envelope": ENVELOPE,
                "payload": payload,
                "signature": {
                    "algorithm": ED25519,
                    "key_id": self.signer.key_id,
                    "value": encode_hex(signature.as_ref()),
                },
            }))
            .unwrap()
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-06T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn payload(revision: u64) -> Value {
        json!({
            "organisation": "org-1",
            "edge_key_id": null,
            "revision": revision,
            "issued_at": "2026-09-06T00:00:00Z",
            "expires_at": "2026-09-07T00:00:00Z",
            "policy": {"policy_mode": "strict", "constraints": [
                {"kind": "allowed_source_host", "host": "www.gov.uk"}]},
        })
    }

    fn unknown_edge() -> EdgeIdentity {
        EdgeIdentity::Unknown {
            reason: "not enrolled".to_owned(),
        }
    }

    /// Enrol `home` so a synchronisation has a key to sign the request with.
    /// The endpoint authenticates the edge by that signature, so a test of
    /// what the hub answers has to start from an edge that can ask.
    fn enrol(home: &Path) -> String {
        use crate::enrolment::{EnrolledIdentity, EnrolledOrganization, EnrolmentRecord};
        let key = crate::identity::EdgeKey::generate().unwrap();
        key.store(home).unwrap();
        let key_id = key.thumbprint();
        EnrolmentRecord {
            hub: "https://hub.example".to_owned(),
            organization: EnrolledOrganization {
                id: "org-1".to_owned(),
                name: "Org".to_owned(),
            },
            name: "laptop".to_owned(),
            key_id: key_id.clone(),
            identity: EnrolledIdentity {
                origin: "https://hub.example".to_owned(),
                bot_page: "https://hub.example/bot".to_owned(),
                contact: None,
            },
            enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
            revoked_at: None,
            revocation: None,
            revocation_learnt_at: None,
        }
        .store(home)
        .unwrap();
        key_id
    }

    #[test]
    fn a_well_formed_envelope_from_the_pinned_signer_verifies() {
        let hub = hub("hub-1");
        let verified = verify(
            &hub.envelope(payload(3)),
            &hub.signer,
            "org-1",
            &unknown_edge(),
            now(),
        )
        .expect("verifies");
        assert_eq!(verified.revision, 3);
        assert_eq!(verified.signer_key_id, "hub-1");
        assert!(verified.digest.starts_with("sha256:"));
        assert_eq!(
            verified.policy.policy_mode,
            crate::policy::PolicyMode::Strict
        );
    }

    /// Each rejection names its kind, and nothing after the signature is
    /// reached by an envelope the signer did not sign.
    #[test]
    fn each_rejection_names_its_kind() {
        let hub = hub("hub-1");
        let kind = |encoded: &[u8], signer: &Signer, org: &str, edge: &EdgeIdentity| {
            verify(encoded, signer, org, edge, now())
                .err()
                .map(|rejection| rejection.kind)
        };

        let other = self::hub("hub-1");
        assert_eq!(
            kind(
                &other.envelope(payload(1)),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("bad_signature"),
            "the same key id under another key is a forgery"
        );
        let renamed = self::hub("hub-2");
        assert_eq!(
            kind(
                &renamed.envelope(payload(1)),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("wrong_signer")
        );
        let mut tampered: Value = serde_json::from_slice(&hub.envelope(payload(1))).unwrap();
        tampered["payload"]["policy"]["policy_mode"] = json!("observe");
        assert_eq!(
            kind(
                &serde_json::to_vec(&tampered).unwrap(),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("bad_signature"),
            "a payload edited after signing does not verify"
        );
        assert_eq!(
            kind(
                &hub.envelope(payload(1)),
                &hub.signer,
                "org-2",
                &unknown_edge()
            ),
            Some("wrong_organisation")
        );
        let mut bound = payload(1);
        bound["edge_key_id"] = json!("edge-a");
        assert_eq!(
            kind(
                &hub.envelope(bound.clone()),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("wrong_edge"),
            "an edge with no key id cannot be the bound edge"
        );
        assert_eq!(
            kind(
                &hub.envelope(bound.clone()),
                &hub.signer,
                "org-1",
                &EdgeIdentity::KeyId("edge-b".to_owned())
            ),
            Some("wrong_edge")
        );
        assert!(
            verify(
                &hub.envelope(bound),
                &hub.signer,
                "org-1",
                &EdgeIdentity::KeyId("edge-a".to_owned()),
                now()
            )
            .is_ok(),
            "the bound edge itself accepts it"
        );
        let mut expired = payload(1);
        expired["expires_at"] = json!("2026-09-06T11:59:59Z");
        assert_eq!(
            kind(
                &hub.envelope(expired),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("expired")
        );
        let mut invalid = payload(1);
        invalid["policy"]["contraints"] = json!([]);
        assert_eq!(
            kind(
                &hub.envelope(invalid),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("invalid_policy"),
            "a misspelled field is refused by the loader's own rule"
        );
        let mut mismatched = payload(1);
        mismatched["digest"] = json!("sha256:0000");
        assert_eq!(
            kind(
                &hub.envelope(mismatched),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("digest_mismatch")
        );
        let mut format: Value = serde_json::from_slice(&hub.envelope(payload(1))).unwrap();
        format["envelope"] = json!("contextops-policy-envelope/v0");
        assert_eq!(
            kind(
                &serde_json::to_vec(&format).unwrap(),
                &hub.signer,
                "org-1",
                &unknown_edge()
            ),
            Some("invalid_schema")
        );
        assert_eq!(
            kind(b"{ not json", &hub.signer, "org-1", &unknown_edge()),
            Some("invalid_schema")
        );
    }

    #[test]
    fn the_deployment_file_is_local_when_absent_and_an_error_when_malformed() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Deployment::read(home.path()).unwrap(), Deployment::Local);
        std::fs::write(Deployment::path(home.path()), r#"{"mode":"local"}"#).unwrap();
        assert_eq!(Deployment::read(home.path()).unwrap(), Deployment::Local);

        std::fs::write(Deployment::path(home.path()), r#"{"mode":"managed"}"#).unwrap();
        assert!(
            Deployment::read(home.path()).is_err(),
            "managed needs a signer"
        );
        std::fs::write(
            Deployment::path(home.path()),
            r#"{"mode":"managed","signer":{"key_id":"k","algorithm":"ed25519","public_key":"abcd"},
                "policy_url":"http://127.0.0.1:1/policy","organisation":"org"}"#,
        )
        .unwrap();
        let error = Deployment::read(home.path()).unwrap_err();
        assert!(error.contains("32-byte"), "{error}");
        std::fs::write(
            Deployment::path(home.path()),
            format!(
                r#"{{"mode":"managed","signer":{{"key_id":"k","algorithm":"ed25519","public_key":"{}"}},
                "policy_url":"ftp://x/policy","organisation":"org"}}"#,
                encode_hex(&[7u8; 32])
            ),
        )
        .unwrap();
        let error = Deployment::read(home.path()).unwrap_err();
        assert!(error.contains("https URL"), "{error}");
        std::fs::write(
            Deployment::path(home.path()),
            r#"{"mode":"managed","signre":{}}"#,
        )
        .unwrap();
        assert!(
            Deployment::read(home.path()).is_err(),
            "unknown fields are errors"
        );
        assert_eq!(management(home.path(), now()).mode, "unavailable");

        // Unknown fields beside a complete declaration, in both modes.
        std::fs::write(
            Deployment::path(home.path()),
            r#"{"mode":"local","extra":1}"#,
        )
        .unwrap();
        let error = Deployment::read(home.path()).unwrap_err();
        assert!(error.contains("extra"), "{error}");
        let complete = |extra: &str, url: &str| {
            format!(
                r#"{{"mode":"managed","signer":{{"key_id":"k","algorithm":"ed25519","public_key":"{}"}},
                "policy_url":"{url}","organisation":"org"{extra}}}"#,
                encode_hex(&[7u8; 32])
            )
        };
        std::fs::write(
            Deployment::path(home.path()),
            complete("", "https://hub.example/api/v1/policy/desired"),
        )
        .unwrap();
        assert!(
            Deployment::read(home.path()).is_ok(),
            "the complete file loads"
        );
        std::fs::write(
            Deployment::path(home.path()),
            complete(
                r#","credential":"x""#,
                "https://hub.example/api/v1/policy/desired",
            ),
        )
        .unwrap();
        let error = Deployment::read(home.path()).unwrap_err();
        assert!(error.contains("credential"), "{error}");

        // Plain http reaches only a loopback origin.
        std::fs::write(
            Deployment::path(home.path()),
            complete("", "http://hub.example/api/v1/policy/desired"),
        )
        .unwrap();
        let error = Deployment::read(home.path()).unwrap_err();
        assert!(error.contains("https"), "{error}");
        std::fs::write(
            Deployment::path(home.path()),
            complete("", "http://127.0.0.1:9/policy"),
        )
        .unwrap();
        assert!(Deployment::read(home.path()).is_ok());
    }

    #[test]
    fn an_envelope_issued_in_the_future_is_not_yet_valid() {
        let hub = hub("hub-1");
        let mut premature = payload(1);
        premature["issued_at"] = json!("2026-09-06T12:00:01Z");
        let rejection = verify(
            &hub.envelope(premature),
            &hub.signer,
            "org-1",
            &unknown_edge(),
            now(),
        )
        .unwrap_err();
        assert_eq!(rejection.kind, "not_yet_valid");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("zz"), None);
        assert_eq!(encode_hex(&[0, 255, 16]), "00ff10");
    }

    /// The sync path against a loopback hub: accept, keep across an
    /// unreachable hub, refuse rollback and forgery, reapply after a local
    /// edit. The CLI test drives the same path through the binary.
    #[test]
    fn sync_accepts_keeps_the_last_known_good_and_refuses_rollback() {
        use std::sync::{Arc, Mutex};
        let hub = hub("hub-1");
        let served: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(hub.envelope(payload(2))));
        let handler_served = Arc::clone(&served);
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                assert!(
                    request.headers.get("X-API-Key").is_none(),
                    "a policy fetch carries no telemetry key"
                );
                let body = handler_served.lock().unwrap().clone();
                commonmeasure_http::Response::new(200, body)
            })
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{}/policy", origin.url()),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .unwrap();

        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "accepted");
        assert!(report.converged());
        let applied = report.applied.clone().expect("applied");
        assert_eq!(applied.revision, 2);
        let document = PolicyDocument::read(home.path()).unwrap();
        assert_eq!(
            applied.digest,
            canonical_digest(&payload(2)["policy"]),
            "the revision is named by the digest of the policy as carried"
        );
        let loader_form = serde_json::to_value(
            serde_json::from_value::<PolicyFile>(payload(2)["policy"].clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            document.digest().as_deref(),
            Some(canonical_digest(&loader_form).as_str()),
            "the file on disk is the loader's form of the same policy"
        );
        assert_eq!(
            document.resolve(None).mode(),
            crate::policy::PolicyMode::Strict,
            "the distributed policy is the one in force"
        );
        assert!(State::last_known_good_path(home.path()).exists());
        let reported = management(home.path(), now());
        assert_eq!(reported.mode, "managed");
        assert_eq!(reported.applied_revision, Some(2));
        assert_eq!(reported.desired["outcome"], "accepted");

        // Rollback: an earlier revision, validly signed, is refused and the
        // applied policy stays.
        *served.lock().unwrap() = hub.envelope(payload(1));
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(report.sync.reason.as_deref().unwrap().contains("rollback"));
        assert_eq!(report.applied.as_ref().unwrap().revision, 2);
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Strict
        );

        // Forgery under another key, carrying a looser policy: refused,
        // and strict stays strict.
        let forger = self::hub("hub-1");
        let mut loose = payload(3);
        loose["policy"] = json!({"policy_mode": "observe"});
        *served.lock().unwrap() = forger.envelope(loose);
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("bad_signature")
        );
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Strict
        );

        // The same revision again is a no-op; after a local edit it is
        // reapplied.
        *served.lock().unwrap() = hub.envelope(payload(2));
        assert_eq!(
            sync(home.path(), &unknown_edge(), now())
                .unwrap()
                .sync
                .outcome,
            "already_applied"
        );
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .unwrap();
        assert_eq!(
            sync(home.path(), &unknown_edge(), now())
                .unwrap()
                .sync
                .outcome,
            "reapplied"
        );
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Strict
        );

        // The same revision with other content is a reuse, refused.
        let mut reused = payload(2);
        reused["policy"] = json!({"policy_mode": "prefer"});
        *served.lock().unwrap() = hub.envelope(reused);
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("names one policy")
        );

        // Unreachable: the hub is gone and the last-known-good stays.
        let address = origin.url();
        drop(origin);
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{address}/policy"),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "unreachable");
        assert_eq!(report.applied.as_ref().unwrap().revision, 2);
        let reported = management(home.path(), now());
        assert_eq!(reported.applied_revision, Some(2));
        assert_eq!(reported.desired["outcome"], "unreachable");
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Strict,
            "an unreachable hub relaxes nothing"
        );
    }

    /// The envelope and the state are written before the policy file is
    /// replaced, so a managed directory that cannot be written leaves the
    /// policy in force exactly as it was.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_managed_directory_leaves_the_policy_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let hub = hub("hub-1");
        let served = hub.envelope(payload(1));
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| commonmeasure_http::Response::new(200, served.clone()))
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{}/policy", origin.url()),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .unwrap();
        let managed = State::directory(home.path());
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = sync(home.path(), &unknown_edge(), now()).unwrap_err();
        std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(error.contains("last-known-good"), "{error}");
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Observe,
            "nothing was activated"
        );
        assert!(State::read(home.path()).unwrap().applied.is_none());
    }

    /// A hub answer that is not an envelope is the hub not reached.
    #[test]
    fn a_non_envelope_answer_is_recorded_as_unreachable() {
        let hub = hub("hub-1");
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(|_| commonmeasure_http::Response::text(404, "no revision published"))
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{}/policy", origin.url()),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "unreachable");
        assert!(report.sync.reason.as_deref().unwrap().contains("404"));
    }

    /// A 401 from the policy endpoint is the hub refusing this edge's key,
    /// recorded as the edge's own standing rather than a hub out of reach.
    #[test]
    fn a_401_from_the_policy_endpoint_is_recorded_as_unauthenticated() {
        let hub = hub("hub-1");
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(|_| commonmeasure_http::Response::text(401, "a valid credential is required"))
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{}/policy", origin.url()),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "unauthenticated");
        let reason = report.sync.reason.as_deref().unwrap();
        assert!(
            reason.contains("401") && reason.contains("revoked"),
            "{reason}"
        );
        assert!(State::read(home.path()).unwrap().applied.is_none());
    }

    #[test]
    fn a_local_edge_makes_no_request() {
        let home = tempfile::tempdir().unwrap();
        let error = sync(home.path(), &unknown_edge(), now()).unwrap_err();
        assert!(error.contains("deployment mode is local"), "{error}");
        assert!(!State::path(home.path()).exists(), "nothing was recorded");
    }

    /// The digest names the policy as the envelope carries it, computed
    /// before the policy is parsed, so a hub never has to reproduce this
    /// loader's serialisation. The loader's own digest is kept beside it
    /// for the comparison with the file on disk, and the two are equal
    /// exactly when the hub carried the loader's form.
    #[test]
    fn the_digest_is_over_the_policy_as_carried_and_the_loader_form_is_kept_beside_it() {
        let hub = hub("hub-1");
        // As an owner writes it: defaulted fields left out.
        let mut written = payload(4);
        written["policy"] = json!({"policy_mode": "strict"});
        let verified = verify(
            &hub.envelope(written.clone()),
            &hub.signer,
            "org-1",
            &unknown_edge(),
            now(),
        )
        .expect("a policy in the owner's form verifies");
        assert_eq!(
            verified.digest,
            canonical_digest(&json!({"policy_mode": "strict"}))
        );
        let loader_form = serde_json::to_value(&verified.policy).unwrap();
        assert_eq!(verified.loader_digest, canonical_digest(&loader_form));
        assert_ne!(
            verified.digest, verified.loader_digest,
            "the loader adds the defaulted fields, so its form digests differently"
        );

        // As the hub publishes it: the loader's form, digested as carried,
        // which is the same document under both rules.
        let mut published = payload(4);
        published["policy"] = loader_form.clone();
        let verified = verify(
            &hub.envelope(published),
            &hub.signer,
            "org-1",
            &unknown_edge(),
            now(),
        )
        .expect("the hub's form verifies");
        assert_eq!(verified.digest, verified.loader_digest);

        // The loader's digest over a policy carried in another form is no
        // longer accepted: the digest must be over what is carried.
        let mut stale_rule = written;
        stale_rule["digest"] = json!(canonical_digest(&loader_form));
        let rejection = verify(
            &hub.envelope(stale_rule),
            &hub.signer,
            "org-1",
            &unknown_edge(),
            now(),
        )
        .unwrap_err();
        assert_eq!(rejection.kind, "digest_mismatch");
    }

    /// An expired envelope keeps enforcing the policy it carried, the record
    /// says since when it has been stale, and the hub renewing the same
    /// revision with a later expiry clears it without touching the policy
    /// file.
    #[test]
    fn an_expired_envelope_keeps_enforcing_and_is_stale_until_the_hub_renews_it() {
        use std::sync::{Arc, Mutex};
        let hub = hub("hub-1");
        let served: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(hub.envelope(payload(2))));
        let handler_served = Arc::clone(&served);
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| {
                commonmeasure_http::Response::new(200, handler_served.lock().unwrap().clone())
            })
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            Deployment::path(home.path()),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: format!("{}/policy", origin.url()),
                organisation: "org-1".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .unwrap();

        let report = sync(home.path(), &unknown_edge(), now()).unwrap();
        assert_eq!(report.sync.outcome, "accepted");
        assert_eq!(report.stale_since, None);
        assert!(!report.fresh(), "an activation is worth a line");

        // Two days on: the envelope has expired, the hub still serves it,
        // and nothing relaxes.
        let later = now() + chrono::Duration::days(2);
        let report = sync(home.path(), &unknown_edge(), later).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(report.sync.reason.as_deref().unwrap().contains("expired"));
        assert_eq!(report.applied.as_ref().unwrap().revision, 2);
        assert_eq!(
            report.stale_since.as_deref(),
            Some("2026-09-07T00:00:00Z"),
            "stale since the envelope's own expiry"
        );
        assert_eq!(
            PolicyDocument::read(home.path())
                .unwrap()
                .resolve(None)
                .mode(),
            crate::policy::PolicyMode::Strict,
            "the expired envelope's policy stays in force"
        );
        let reported = management(home.path(), later);
        assert_eq!(
            reported.stale_since.as_deref(),
            Some("2026-09-07T00:00:00Z")
        );
        assert_eq!(
            reported.applied_expires_at.as_deref(),
            Some("2026-09-07T00:00:00Z")
        );
        assert_eq!(
            management(home.path(), now()).stale_since,
            None,
            "staleness is a fact about now, not a stored flag"
        );
        let record = report.to_record();
        assert_eq!(record["outcome"], "rejected");
        assert_eq!(record["stale_since"], "2026-09-07T00:00:00Z");
        assert_eq!(record["applied"]["revision"], 2);

        // The hub renews revision 2: same policy, later expiry. Already
        // applied, no longer stale, the window refreshed, the file untouched.
        let mut renewed = payload(2);
        renewed["issued_at"] = json!("2026-09-08T00:00:00Z");
        renewed["expires_at"] = json!("2026-09-15T00:00:00Z");
        *served.lock().unwrap() = hub.envelope(renewed);
        let before = std::fs::metadata(home.path().join("policy.json"))
            .unwrap()
            .modified()
            .unwrap();
        let report = sync(home.path(), &unknown_edge(), later).unwrap();
        assert_eq!(report.sync.outcome, "already_applied");
        assert_eq!(report.stale_since, None);
        assert!(report.fresh());
        assert_eq!(
            report.applied.as_ref().unwrap().expires_at,
            "2026-09-15T00:00:00Z"
        );
        assert_eq!(
            std::fs::metadata(home.path().join("policy.json"))
                .unwrap()
                .modified()
                .unwrap(),
            before,
            "the policy file is not rewritten for a renewal"
        );
        let kept: Value = serde_json::from_slice(
            &std::fs::read(State::last_known_good_path(home.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            kept["payload"]["expires_at"], "2026-09-15T00:00:00Z",
            "the renewed envelope is the one kept"
        );
        assert_eq!(management(home.path(), later).stale_since, None);
    }

    /// A local edge has no synchronisation to run and says so in one
    /// question; a deployment file that does not load is an outcome of its
    /// own, never read as local.
    #[test]
    fn is_managed_answers_local_managed_and_unavailable() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(is_managed(home.path()), Ok(false));
        std::fs::write(Deployment::path(home.path()), r#"{"mode":"managed"}"#).unwrap();
        assert!(is_managed(home.path()).is_err());
        let record = SyncReport::unavailable("managed mode needs a signer");
        assert_eq!(record["outcome"], "unavailable");
        assert_eq!(record["reason"], "managed mode needs a signer");
    }
}
