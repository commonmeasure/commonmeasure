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

/// The outcome reported when the enrolment a request was made under was
/// replaced before its response could be acted on: `deployment.json` or the
/// enrolled edge key changed in the meantime. Nothing is verified or
/// written, the state file included, because the response answers an
/// enrolment other than the one this edge holds now.
pub const SUPERSEDED: &str = "superseded";

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

/// Accept modest clock skew between the signer and edge without accepting
/// arbitrarily future-dated policy. Only issue time receives this allowance;
/// expiry remains strict, and network delay is handled by reading the clock
/// after the complete response arrives.
pub const ISSUED_AT_TOLERANCE: chrono::Duration = chrono::Duration::minutes(5);

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
/// synchronisation time is never written. The refusal names the URL by its
/// origin: a hand-written `policy_url` can carry credentials, and the
/// refusal is recorded as a `policy_sync` reason. A value that does not
/// parse has no origin and is refused with the parser's reason, which
/// quotes no part of it.
pub fn policy_url_accepted(policy_url: &str) -> Result<(), String> {
    match url::Url::parse(policy_url) {
        Err(parse_error) => Err(format!("policy_url is not a URL: {parse_error}")),
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
            "policy_url{} must be an https URL, or http to a loopback origin",
            crate::enrolment::at_hub_origin(policy_url)
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
                return Err(format!(
                    "{}: {reason}; correct it by hand, or run `commonmeasure disconnect`, then \
                     `commonmeasure connect --managed` to pin the hub's",
                    source.display()
                ));
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
    if issued_at.signed_duration_since(now) > ISSUED_AT_TOLERANCE {
        return Err(Rejection::new(
            "not_yet_valid",
            format!(
                "the envelope is issued at {}, which is more than the {} second clock-skew tolerance after now",
                issued_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                ISSUED_AT_TOLERANCE.num_seconds()
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
    /// The organisation the revision was verified for. Absent from a state
    /// written by 0.4.6 or earlier; [`Applied::pin`] then reads it from the
    /// kept envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organisation: Option<String>,
}

/// The organisation and signer a revision was verified under: the pair
/// `deployment.json` pinned when it was accepted. The rollback counter
/// belongs to one pair, so a re-enrolled edge judges the new pair's
/// revisions only against revisions from that pair
/// (`docs/contracts/policy-envelope.md` §Re-enrolment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// `None` for a state written by 0.4.6 or earlier whose kept envelope
    /// cannot be read, is not the applied revision's or names no
    /// organisation, and for an empty organisation in either file.
    pub organisation: Option<String>,
    pub signer_key_id: String,
}

impl Pin {
    /// Whether a revision verified under this pair is judged against the
    /// pinned `organisation` and `signer_key_id`. An organisation that
    /// cannot be recovered counts as the pinned one when the signer is: the
    /// counter is kept, not dropped on a guess, and the edge refuses as it
    /// did before rather than accepting an earlier revision.
    fn governs(&self, organisation: &str, signer_key_id: &str) -> bool {
        self.signer_key_id == signer_key_id
            && self
                .organisation
                .as_deref()
                .is_none_or(|own| own == organisation)
    }
}

/// The applied revision, verified under a pair other than the one
/// `deployment.json` now pins. Its policy stays in force as the
/// last-known-good until the pinned pair's first revision is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Previous {
    pub applied: Pin,
    pub pinned: Pin,
}

impl std::fmt::Display for Previous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pinned_organisation = self.pinned.organisation.as_deref().unwrap_or_default();
        match self.applied.organisation.as_deref() {
            Some(organisation) if organisation != pinned_organisation => write!(
                f,
                "from previous organisation {organisation} (signer {}); this edge is pinned to \
                 organisation {pinned_organisation}",
                self.applied.signer_key_id
            ),
            _ => write!(
                f,
                "signed by previous signer {}; this edge pins {}",
                self.applied.signer_key_id, self.pinned.signer_key_id
            ),
        }
    }
}

impl Applied {
    /// The pair this revision was verified under. A state written by 0.4.6
    /// or earlier records the signer and not the organisation; the
    /// organisation is then the one the kept envelope names, which the edge
    /// wrote verbatim only after verifying it, provided the kept envelope is
    /// this revision's by revision and digest. That metadata is trusted as
    /// the state file is: both sit in the same operator-controlled home, and
    /// the kept envelope's signature is not checked again here.
    ///
    /// An empty organisation, in the state or in the kept envelope, is
    /// unknown: no deployment names one, so it cannot have been verified,
    /// and reading it as another organisation would drop the counter.
    pub fn pin(&self, home: &Path) -> Pin {
        let organisation = match &self.organisation {
            Some(organisation) => Some(organisation.clone()),
            None => self.kept_organisation(home),
        }
        .filter(|organisation| !organisation.is_empty());
        Pin {
            organisation,
            signer_key_id: self.signer_key_id.clone(),
        }
    }

    /// The organisation the kept envelope names, when it is this revision's.
    fn kept_organisation(&self, home: &Path) -> Option<String> {
        let kept: Value =
            serde_json::from_slice(&std::fs::read(State::last_known_good_path(home)).ok()?).ok()?;
        let payload = &kept["payload"];
        (payload["revision"].as_u64() == Some(self.revision)
            && payload["digest"].as_str() == Some(self.digest.as_str()))
        .then(|| payload["organisation"].as_str().map(str::to_owned))
        .flatten()
    }

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
    /// Where the revision in force was verified under a pair other than the
    /// pinned one: the pair it came from and the pair pinned now.
    pub previous: Option<Previous>,
}

impl SyncReport {
    /// Whether the desired policy is the one in force after this run.
    pub fn converged(&self) -> bool {
        matches!(
            self.sync.outcome.as_str(),
            "accepted" | "reapplied" | "already_applied"
        )
    }

    /// Enrolment may finish while the organisation has not published its first
    /// revision: none has been applied under the pinned pair, whether nothing
    /// is applied or the revision in force came from a previous enrolment.
    /// This is not convergence and cannot clear an applied policy.
    pub fn awaiting_first_revision(&self) -> bool {
        self.sync.outcome == "no_revision" && (self.applied.is_none() || self.previous.is_some())
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

/// What a synchronisation asks under: the deployment `deployment.json`
/// pins and the edge key `enrolment.json` names. Read once before the
/// request and again under the policy file's lock before its response is
/// acted on. The writers of both files (`connect`, `disconnect`) hold the
/// same lock, so a basis that still matches under it is the one in force
/// until the lock is released.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Basis {
    deployment: Deployment,
    edge_key_id: Option<String>,
}

impl Basis {
    fn read(home: &Path) -> Result<Self, String> {
        Ok(Self {
            deployment: Deployment::read(home)?,
            edge_key_id: crate::enrolment::enrolled_key_id(home)?,
        })
    }

    /// Why the basis a request was made under is not current now, or
    /// `None` when it still is. Called with the policy file's lock held.
    /// A file that does not load now cannot confirm the basis, so it
    /// supersedes it too. The reason names no policy URL, which can carry
    /// credentials.
    fn superseded_in(&self, home: &Path) -> Option<String> {
        let current = match Self::read(home) {
            Ok(current) if current == *self => return None,
            Ok(current) => current,
            Err(error) => return Some(format!("the enrolment could not be read again ({error})")),
        };
        let mut changes = Vec::new();
        match (&self.deployment, &current.deployment) {
            (
                Deployment::Managed {
                    signer,
                    policy_url,
                    organisation,
                },
                Deployment::Managed {
                    signer: now_signer,
                    policy_url: now_policy_url,
                    organisation: now_organisation,
                },
            ) => {
                if organisation != now_organisation {
                    changes.push(format!(
                        "the pinned organisation changed from {organisation} to {now_organisation}"
                    ));
                }
                if signer != now_signer {
                    changes.push(format!(
                        "the pinned signer changed from {} to {}",
                        signer.key_id, now_signer.key_id
                    ));
                }
                if policy_url != now_policy_url {
                    changes.push("the policy URL changed".to_owned());
                }
            }
            (_, Deployment::Local) => changes.push("the deployment is now local".to_owned()),
            (Deployment::Local, _) => changes.push("the deployment is now managed".to_owned()),
        }
        if self.edge_key_id != current.edge_key_id {
            changes.push(match (&self.edge_key_id, &current.edge_key_id) {
                (Some(before), Some(now)) => {
                    format!("the enrolled edge key changed from {before} to {now}")
                }
                (Some(before), None) => format!("edge key {before} is not enrolled now"),
                (None, _) => "an edge key was enrolled".to_owned(),
            });
        }
        Some(changes.join("; "))
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
/// policy already in force. The caller supplies a clock, read once for
/// request signing and again after the complete response arrives for
/// envelope validity and staleness. A failed request uses the second reading
/// too; an edge that cannot sign uses only the first.
///
/// `Err` is a refusal to try: a local edge, or a deployment or state file
/// this runtime cannot read. Everything the hub does or fails to do is an
/// `Ok` report with its outcome, recorded in the state file so the next
/// status document carries it. The exception is [`SUPERSEDED`]: the
/// enrolment the request was made under was replaced before the response
/// was acted on, and the state file belongs to the current enrolment.
pub fn sync(
    home: &Path,
    edge: &EdgeIdentity,
    clock: impl FnMut() -> DateTime<Utc>,
) -> Result<SyncReport, String> {
    sync_within(home, edge, clock, DEFAULT_BUDGET)
}

/// [`sync`] with the time the request may take bounded by `budget`: the
/// session-start form, where a host is waiting.
pub fn sync_within(
    home: &Path,
    edge: &EdgeIdentity,
    mut clock: impl FnMut() -> DateTime<Utc>,
    budget: Duration,
) -> Result<SyncReport, String> {
    let basis = Basis::read(home)?;
    let Deployment::Managed {
        signer,
        policy_url,
        organisation,
    } = basis.deployment.clone()
    else {
        return Err(format!(
            "deployment mode is local: no management request is made. Write {} with \
             \"mode\": \"managed\", a pinned signer and a policy_url to accept remote policy.",
            Deployment::path(home).display()
        ));
    };
    let pinned = Pin {
        organisation: Some(organisation.clone()),
        signer_key_id: signer.key_id.clone(),
    };
    if let Some(record) = crate::EnrolmentRecord::load(home)? {
        crate::enrolment::hub_url_accepted(&record.hub)?;
    }
    let now = clock();
    let at = now.to_rfc3339_opts(SecondsFormat::Millis, true);

    // A management request on its own path, carrying no telemetry key and no
    // credential of its own: the hub authenticates this edge by the signature
    // it makes with its enrolled key, the same key and the same construction
    // the mediated fetch signs with (`docs/contracts/policy-envelope.md`).
    let identity = crate::identity::Identity::load(home)?;
    let (sent, now) = match identity.signer() {
        None => {
            let reason = identity
                .presented()
                .unsigned
                .unwrap_or_else(|| "this edge holds no key to sign with".to_owned());
            // No request is made. The hub would refuse an unsigned one, and an
            // edge reporting "unreachable" for its own missing key would send
            // the operator to look at the hub.
            (
                Err(Sync {
                    at: at.clone(),
                    outcome: UNAUTHENTICATED.to_owned(),
                    revision: None,
                    digest: None,
                    reason: Some(format!(
                        "{reason}. The policy endpoint authenticates the edge by its enrolled \
                         key, so no request was made; run `commonmeasure connect` to enrol this \
                         edge."
                    )),
                }),
                now,
            )
        }
        Some(edge_key) => {
            let mut request = commonmeasure_http::Request::get("/");
            request.headers.set("User-Agent", USER_AGENT);
            request.headers.set("Accept", "application/json");
            match edge_key.sign_request(&policy_url, &mut request, &[], now.timestamp()) {
                Err(reason) => (
                    Err(Sync {
                        at: at.clone(),
                        outcome: UNAUTHENTICATED.to_owned(),
                        revision: None,
                        digest: None,
                        reason: Some(format!("the request could not be signed: {reason}")),
                    }),
                    now,
                ),
                Ok(()) => {
                    let sent = send_within(&policy_url, request, budget);
                    (Ok(sent), clock())
                }
            }
        }
    };
    // The policy file is held exclusively from here to the save, against
    // the console's editor, another synchronisation and the enrolment
    // writers, so a read, a comparison and a replacement are one step.
    let _lock = PolicyDocument::lock(home)
        .map_err(|refused| format!("cannot hold the policy file: {refused}"))?;
    // The response answers the enrolment it was asked under. If `connect`
    // or `disconnect` replaced that enrolment while the request was out,
    // acting on the response would put the former enrolment's policy over
    // whatever the current one has accepted since, so nothing is written.
    if let Some(change) = basis.superseded_in(home) {
        return superseded(home, &policy_url, at, &change, now);
    }
    let mut state = State::read(home)?;
    let response = match sent {
        Err(sync) => return conclude(home, &pinned, &policy_url, state, sync, now),
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return conclude(
                home,
                &pinned,
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
            &pinned,
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
    // Match the hub's explicit absence response, not any 404: a wrong URL
    // or a proxy error must still fail onboarding. No envelope is accepted
    // and any policy already applied remains in force.
    if response.status == 404
        && serde_json::from_slice::<Value>(&response.body).is_ok_and(|body| {
            body["detail"] == "no policy revision has been published for this organisation"
        })
    {
        return conclude(
            home,
            &pinned,
            &policy_url,
            state,
            Sync {
                at,
                outcome: "no_revision".to_owned(),
                revision: None,
                digest: None,
                reason: Some("the hub has not published a policy revision yet; the policy already in force stays".to_owned()),
            },
            now,
        );
    }
    // Any other non-envelope response is an unavailable policy endpoint.
    if response.status != 200 {
        return conclude(
            home,
            &pinned,
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
                &pinned,
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
    // Revisions are ordered within one organisation and signer, so the
    // counter is the applied revision's only when it was verified under the
    // pair pinned now. A revision from a previous enrolment is not compared:
    // the pinned pair's first revision replaces it, whatever its number.
    let same_pin = state
        .applied
        .as_ref()
        .is_some_and(|applied| applied.pin(home).governs(&organisation, &signer.key_id));
    if let Some(applied) = state.applied.as_ref().filter(|_| same_pin) {
        if verified.revision < applied.revision {
            return conclude(
                home,
                &pinned,
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
                &pinned,
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
                if let Some(applied) = state.applied.as_mut() {
                    if applied.expires_at != verified.expires_at
                        || applied.issued_at != verified.issued_at
                    {
                        declaration::replace(&State::last_known_good_path(home), &response.body)?;
                        applied.issued_at.clone_from(&verified.issued_at);
                        applied.expires_at.clone_from(&verified.expires_at);
                    }
                    // A state from 0.4.6 or earlier, or one whose organisation
                    // is empty, learns its organisation from the envelope that
                    // has just verified under it.
                    if applied.organisation.as_deref().is_none_or(str::is_empty) {
                        applied.organisation = Some(organisation.clone());
                    }
                }
                return conclude(
                    home,
                    &pinned,
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
    let reapplied = same_pin
        && state
            .applied
            .as_ref()
            .is_some_and(|applied| applied.revision == verified.revision);

    // The loader's own validity rule, before anything is written, so a
    // policy the runtime would refuse is refused here.
    if let Err(error) = PolicyDocument::validate(&verified.policy) {
        return conclude(
            home,
            &pinned,
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
        organisation: Some(organisation.clone()),
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
        previous: None,
        applied: state.applied,
    })
}

/// Send one request with name resolution inside the budget as well as the
/// exchange. The standard resolver has no timeout of its own, so it runs on
/// a thread that is left behind if it has not answered in time; a resolver
/// that does not answer within the budget is the hub not reached, recorded
/// like any other unreachable outcome. Errors name the policy URL by its
/// origin, since it can carry credentials; the transport's own errors name
/// only the host or the authority of a URL that has already parsed.
pub(crate) fn send_within(
    url: &str,
    request: commonmeasure_http::Request,
    budget: Duration,
) -> Result<commonmeasure_http::Response, String> {
    send_resolving_within(url, request, budget, commonmeasure_http::resolve)
}

/// [`send_within`] with the resolver passed in, so a test can hold one past
/// the budget without a name lookup leaving the machine.
fn send_resolving_within<E>(
    url: &str,
    request: commonmeasure_http::Request,
    budget: Duration,
    resolve: impl FnOnce(&str) -> Result<Vec<std::net::SocketAddr>, E> + Send + 'static,
) -> Result<commonmeasure_http::Response, String>
where
    E: std::fmt::Display + Send + 'static,
{
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let target = url.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(resolve(&target));
    });
    let addresses = match rx.recv_timeout(budget) {
        Ok(resolved) => resolved.map_err(|error| format!("{error:#}"))?,
        Err(_) => {
            return Err(format!(
                "name resolution for {} did not complete within {} s: timed out",
                crate::relay_config::receiver_origin(url)
                    .unwrap_or_else(|| "the policy URL".to_owned()),
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
    pinned: &Pin,
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
        previous: state
            .applied
            .as_ref()
            .and_then(|applied| previous(home, applied, pinned)),
        applied: state.applied,
    })
}

/// Report a synchronisation whose enrolment was replaced while it was out,
/// without recording it: `state.json`, the kept envelope and `policy.json`
/// stay as the current enrolment left them. The report describes what is in
/// force now, against the pair pinned now.
fn superseded(
    home: &Path,
    policy_url: &str,
    at: String,
    change: &str,
    now: DateTime<Utc>,
) -> Result<SyncReport, String> {
    let applied = State::read(home)?.applied;
    let pinned = match Deployment::read(home) {
        Ok(Deployment::Managed {
            signer,
            organisation,
            ..
        }) => Some(Pin {
            organisation: Some(organisation),
            signer_key_id: signer.key_id,
        }),
        _ => None,
    };
    Ok(SyncReport {
        policy_url: policy_url.to_owned(),
        sync: Sync {
            at,
            outcome: SUPERSEDED.to_owned(),
            revision: None,
            digest: None,
            reason: Some(format!(
                "{change} while this request was outstanding; its response was discarded and \
                 nothing was written. The next synchronisation asks under the current enrolment"
            )),
        },
        stale_since: applied
            .as_ref()
            .and_then(|applied| applied.stale_since(now)),
        previous: applied
            .as_ref()
            .zip(pinned.as_ref())
            .and_then(|(applied, pinned)| previous(home, applied, pinned)),
        applied,
    })
}

/// The applied revision's pair and the pinned one, where they differ.
fn previous(home: &Path, applied: &Applied, pinned: &Pin) -> Option<Previous> {
    let organisation = pinned.organisation.as_deref().unwrap_or_default();
    let own = applied.pin(home);
    (!own.governs(organisation, &pinned.signer_key_id)).then(|| Previous {
        applied: own,
        pinned: pinned.clone(),
    })
}

/// Where the revision in force on this edge was verified under a pair
/// other than the one `deployment.json` pins, what `status` and `doctor`
/// name beside it. `None` on a local edge, before any revision is applied,
/// and where the deployment or state file does not load, which those
/// commands report as unavailable.
pub fn previous_pin(home: &Path) -> Option<Previous> {
    let Ok(Deployment::Managed {
        signer,
        organisation,
        ..
    }) = Deployment::read(home)
    else {
        return None;
    };
    let applied = State::read(home).ok()?.applied?;
    previous(
        home,
        &applied,
        &Pin {
            organisation: Some(organisation),
            signer_key_id: signer.key_id,
        },
    )
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

    /// A `policy_url` can carry userinfo (written by hand, or built from a
    /// 0.4.1 hub URL), in either spelling the parser accepts. The reasons
    /// below are recorded in the session log, the timeout in the state file
    /// too, and the console serves them, so they name the policy URL by its
    /// origin.
    #[test]
    fn a_recorded_reason_names_the_policy_url_by_its_origin() {
        let leaks = |text: &str| text.contains("PLANTED") || text.contains("op:");
        for url in [
            "https://op:ak_PLANTED@hub.example/api/v1/policy/desired",
            "https:/op:ak_PLANTED@hub.example/api/v1/policy/desired",
        ] {
            let error = send_resolving_within(
                url,
                commonmeasure_http::Request::get("/"),
                Duration::from_millis(20),
                |_| -> Result<Vec<std::net::SocketAddr>, String> {
                    std::thread::sleep(Duration::from_secs(2));
                    Err("not asked".to_owned())
                },
            )
            .unwrap_err();
            assert!(error.contains("did not complete within"), "{error}");
            assert!(error.contains("https://hub.example "), "{error}");
            assert!(!leaks(&error), "{error}");
        }
        for url in [
            "http://op:ak_PLANTED@hub.example/api/v1/policy/desired",
            "http:/op:ak_PLANTED@hub.example/api/v1/policy/desired",
        ] {
            let error = policy_url_accepted(url).unwrap_err();
            assert!(error.contains("http://hub.example"), "{error}");
            assert!(!leaks(&error), "{error}");
        }
        // A value that does not parse has no origin and is named by no part
        // of itself.
        let error = policy_url_accepted("https://op:ak_PLANTED@[hub.example/policy").unwrap_err();
        assert!(!leaks(&error), "{error}");
    }

    /// A `policy_url` that does not parse is refused with the parser's
    /// reason, which quotes no part of the value, so the operator can see
    /// the mistake without the value being recorded.
    #[test]
    fn a_policy_url_that_does_not_parse_is_refused_with_the_parsers_reason() {
        let error =
            policy_url_accepted("https://op:ak_PLANTED@hub.example:99999/api/v1/policy/desired")
                .unwrap_err();
        assert_eq!(error, "policy_url is not a URL: invalid port number");
    }

    #[test]
    fn an_envelope_issued_beyond_the_tolerance_is_not_yet_valid() {
        let hub = hub("hub-1");
        let mut premature = payload(1);
        premature["issued_at"] =
            json!((now() + ISSUED_AT_TOLERANCE + chrono::Duration::seconds(1)).to_rfc3339());
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

        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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
            sync(home.path(), &unknown_edge(), now)
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
            sync(home.path(), &unknown_edge(), now)
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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

        let error = sync(home.path(), &unknown_edge(), now).unwrap_err();
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "unreachable");
        assert!(report.sync.reason.as_deref().unwrap().contains("404"));
    }

    #[test]
    fn no_revision_waits_then_accepts_and_never_erases_the_last_known_good() {
        use std::sync::{Arc, Mutex};
        let hub = hub("hub-1");
        let absent = commonmeasure_http::Response::json(
            404,
            r#"{"detail":"no policy revision has been published for this organisation"}"#,
        );
        let served = Arc::new(Mutex::new(absent.clone()));
        let handler = served.clone();
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| handler.lock().unwrap().clone())
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
        let local = br#"{"policy_mode":"strict"}"#;
        std::fs::write(home.path().join("policy.json"), local).unwrap();
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert!(report.awaiting_first_revision());
        assert!(!report.converged());
        assert_eq!(
            std::fs::read(home.path().join("policy.json")).unwrap(),
            local
        );
        assert!(!State::last_known_good_path(home.path()).exists());

        *served.lock().unwrap() = commonmeasure_http::Response::new(200, hub.envelope(payload(2)));
        let accepted = sync(home.path(), &unknown_edge(), now).unwrap();
        assert!(accepted.converged());
        let policy = std::fs::read(home.path().join("policy.json")).unwrap();
        let envelope = std::fs::read(State::last_known_good_path(home.path())).unwrap();
        *served.lock().unwrap() = absent;
        let later = now() + chrono::Duration::days(3650);
        let report = sync(home.path(), &unknown_edge(), || later).unwrap();
        assert_eq!(report.sync.outcome, "no_revision");
        assert!(!report.awaiting_first_revision());
        assert!(!report.converged());
        assert_eq!(report.applied, accepted.applied);
        assert!(report.stale_since.is_some());
        assert_eq!(
            std::fs::read(home.path().join("policy.json")).unwrap(),
            policy
        );
        assert_eq!(
            std::fs::read(State::last_known_good_path(home.path())).unwrap(),
            envelope
        );
        assert_eq!(
            State::read(home.path()).unwrap().last_sync.unwrap().outcome,
            "no_revision"
        );
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
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
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
        let error = sync(home.path(), &unknown_edge(), now).unwrap_err();
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

        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "accepted");
        assert_eq!(report.stale_since, None);
        assert!(!report.fresh(), "an activation is worth a line");

        // Two days on: the envelope has expired, the hub still serves it,
        // and nothing relaxes.
        let later = now() + chrono::Duration::days(2);
        let report = sync(home.path(), &unknown_edge(), || later).unwrap();
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
        let report = sync(home.path(), &unknown_edge(), || later).unwrap();
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

    fn pin(home: &Path, hub: &Hub, policy_url: &str, organisation: &str) {
        std::fs::write(
            Deployment::path(home),
            serde_json::to_vec(&Deployment::Managed {
                signer: hub.signer.clone(),
                policy_url: policy_url.to_owned(),
                organisation: organisation.to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
    }

    fn for_organisation(revision: u64, organisation: &str, policy: Value) -> Value {
        let mut payload = payload(revision);
        payload["organisation"] = json!(organisation);
        payload["policy"] = policy;
        payload
    }

    fn policy_mode(home: &Path) -> crate::policy::PolicyMode {
        PolicyDocument::read(home).unwrap().resolve(None).mode()
    }

    /// An origin whose answer the test sets: an envelope, or the hub's
    /// explicit absence.
    fn settable_origin() -> (
        commonmeasure_http::ServerHandle,
        std::sync::Arc<std::sync::Mutex<commonmeasure_http::Response>>,
    ) {
        use std::sync::{Arc, Mutex};
        let served = Arc::new(Mutex::new(commonmeasure_http::Response::json(
            404,
            r#"{"detail":"no policy revision has been published for this organisation"}"#,
        )));
        let handler = Arc::clone(&served);
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| handler.lock().unwrap().clone())
            .unwrap();
        (origin, served)
    }

    /// EDG-91. An edge re-enrolled into another organisation under another
    /// signer keeps the previous organisation's revision in force, named as
    /// such, until the new organisation's first revision arrives; that
    /// revision is accepted whatever its number. From then on the counter is
    /// the new pair's: a lower revision and a reused revision are refused,
    /// and the previous organisation's envelopes are refused by signer and
    /// organisation as before.
    #[test]
    fn a_re_enrolled_edge_takes_the_new_organisations_first_revision_and_keeps_its_counter() {
        let previous_hub = hub("hub-policy-old");
        let new_hub = hub("hub-policy-new");
        let (origin, served) = settable_origin();
        let url = format!("{}/policy", origin.url());
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .unwrap();

        pin(home.path(), &previous_hub, &url, "org-old");
        let strict = json!({"policy_mode": "strict", "constraints": [
            {"kind": "allowed_source_host", "host": "www.gov.uk"}]});
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            previous_hub.envelope(for_organisation(8, "org-old", strict)),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "accepted");
        assert_eq!(
            report.applied.as_ref().unwrap().organisation.as_deref(),
            Some("org-old"),
            "the applied state records the organisation it was verified for"
        );
        assert!(report.previous.is_none());
        assert!(previous_pin(home.path()).is_none());

        // Re-enrolment rewrites the pin and nothing else. Before the new
        // organisation publishes, the previous revision stays in force and
        // is named as the previous organisation's.
        pin(home.path(), &new_hub, &url, "org-new");
        *served.lock().unwrap() = commonmeasure_http::Response::json(
            404,
            r#"{"detail":"no policy revision has been published for this organisation"}"#,
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "no_revision");
        assert_eq!(report.applied.as_ref().unwrap().revision, 8);
        assert!(
            report.awaiting_first_revision(),
            "no revision has been applied under the new pin"
        );
        let expected = Previous {
            applied: Pin {
                organisation: Some("org-old".to_owned()),
                signer_key_id: "hub-policy-old".to_owned(),
            },
            pinned: Pin {
                organisation: Some("org-new".to_owned()),
                signer_key_id: "hub-policy-new".to_owned(),
            },
        };
        assert_eq!(report.previous.as_ref(), Some(&expected));
        assert_eq!(previous_pin(home.path()), Some(expected));
        assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Strict);

        // The previous organisation's envelope, even a later revision, is
        // refused under the new pin by signer, and by organisation where the
        // new signer signed it.
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            previous_hub.envelope(for_organisation(
                9,
                "org-old",
                json!({"policy_mode": "observe"}),
            )),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("wrong_signer")
        );
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            new_hub.envelope(for_organisation(
                9,
                "org-old",
                json!({"policy_mode": "observe"}),
            )),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("wrong_organisation")
        );
        assert_eq!(report.applied.as_ref().unwrap().revision, 8);
        assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Strict);

        // The new organisation's first revision is lower than the previous
        // one and is accepted.
        let prefer = json!({"policy_mode": "prefer"});
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            new_hub.envelope(for_organisation(1, "org-new", prefer.clone())),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "accepted", "{:?}", report.sync.reason);
        let applied = report.applied.clone().unwrap();
        assert_eq!(applied.revision, 1);
        assert_eq!(applied.organisation.as_deref(), Some("org-new"));
        assert_eq!(applied.signer_key_id, "hub-policy-new");
        assert!(report.previous.is_none());
        assert!(previous_pin(home.path()).is_none());
        assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Prefer);

        // Within the new pair the counter holds: a later revision, then an
        // earlier one refused, then the same revision with another digest
        // refused.
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            new_hub.envelope(for_organisation(3, "org-new", prefer.clone())),
        );
        assert_eq!(
            sync(home.path(), &unknown_edge(), now)
                .unwrap()
                .sync
                .outcome,
            "accepted"
        );
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            new_hub.envelope(for_organisation(2, "org-new", prefer)),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("rollback refused: revision 2 is earlier than the applied revision 3")
        );
        *served.lock().unwrap() = commonmeasure_http::Response::new(
            200,
            new_hub.envelope(for_organisation(
                3,
                "org-new",
                json!({"policy_mode": "observe"}),
            )),
        );
        let report = sync(home.path(), &unknown_edge(), now).unwrap();
        assert_eq!(report.sync.outcome, "rejected");
        assert!(
            report
                .sync
                .reason
                .as_deref()
                .unwrap()
                .contains("names one policy")
        );
        assert_eq!(report.applied.as_ref().unwrap().revision, 3);
        assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Prefer);
    }

    /// The state a 0.4.6 edge leaves, as found on the owner's edge on 29
    /// September 2026 with synthetic keys, organisations and policy: the
    /// applied revision 8 names its signer and no organisation, the kept
    /// envelope names the old organisation and is bound to the edge's old
    /// key, the last synchronisation refused revision 1 as a rollback, and
    /// the new revision carries the same policy under the same digest.
    fn legacy_home(home: &Path, old_hub: &Hub, envelope: &[u8], digest: &str) {
        std::fs::create_dir_all(home.join("managed")).unwrap();
        std::fs::write(State::last_known_good_path(home), envelope).unwrap();
        std::fs::write(
            home.join("managed/state.json"),
            serde_json::to_vec_pretty(&json!({
                "applied": {
                    "revision": 8,
                    "digest": digest,
                    "issued_at": "2026-09-06T00:00:00Z",
                    "expires_at": "2026-09-07T00:00:00Z",
                    "activated_at": "2026-09-01T05:37:59.307Z",
                    "signer_key_id": old_hub.signer.key_id,
                },
                "last_sync": {
                    "at": "2026-09-06T06:26:04.573Z",
                    "outcome": "rejected",
                    "revision": 1,
                    "digest": digest,
                    "reason": "rollback refused: revision 1 is earlier than the applied revision 8",
                },
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn legacy_policy() -> Value {
        json!({"policy_mode": "strict", "constraints": [
            {"kind": "allowed_source_host", "host": "www.gov.uk"}]})
    }

    #[test]
    fn a_state_from_0_4_6_is_judged_by_the_organisation_its_kept_envelope_names() {
        let old_hub = hub("hub-policy-2d20");
        let new_hub = hub("hub-policy-adad");
        let (origin, served) = settable_origin();
        let url = format!("{}/policy", origin.url());
        let digest = canonical_digest(&legacy_policy());
        let mut kept = for_organisation(8, "org-fb3a", legacy_policy());
        kept["edge_key_id"] = json!("the-edge-key-before-re-enrolment");
        let kept = old_hub.envelope(kept);

        // The owner's shape: another organisation under another signer. And
        // a hub that signs for both organisations with one key, where only
        // the organisation the kept envelope names tells the pairs apart.
        for new_signer in [&new_hub, &old_hub] {
            let home = tempfile::tempdir().unwrap();
            enrol(home.path());
            legacy_home(home.path(), &old_hub, &kept, &digest);
            std::fs::write(
                home.path().join("policy.json"),
                serde_json::to_vec(&serde_json::from_value::<PolicyFile>(legacy_policy()).unwrap())
                    .unwrap(),
            )
            .unwrap();
            pin(home.path(), new_signer, &url, "org-04fe");

            let state = State::read(home.path()).expect("a 0.4.6 state still loads");
            assert_eq!(state.applied.as_ref().unwrap().organisation, None);
            let previous = previous_pin(home.path()).expect("the kept envelope names org-fb3a");
            assert_eq!(previous.applied.organisation.as_deref(), Some("org-fb3a"));
            assert_eq!(previous.applied.signer_key_id, "hub-policy-2d20");
            assert!(
                previous
                    .to_string()
                    .contains("from previous organisation org-fb3a")
            );

            *served.lock().unwrap() = commonmeasure_http::Response::new(
                200,
                new_signer.envelope(for_organisation(1, "org-04fe", legacy_policy())),
            );
            let report = sync(home.path(), &unknown_edge(), now).unwrap();
            assert_eq!(report.sync.outcome, "accepted", "{:?}", report.sync.reason);
            let applied = report.applied.unwrap();
            assert_eq!(
                (applied.revision, applied.digest.as_str()),
                (1, digest.as_str())
            );
            assert_eq!(applied.organisation.as_deref(), Some("org-04fe"));
            assert_eq!(applied.signer_key_id, new_signer.signer.key_id);
            assert!(previous_pin(home.path()).is_none());
            let now_kept: Value = serde_json::from_slice(
                &std::fs::read(State::last_known_good_path(home.path())).unwrap(),
            )
            .unwrap();
            assert_eq!(now_kept["payload"]["organisation"], "org-04fe");
        }
    }

    /// A 0.4.6 state under the pair still pinned keeps its counter, and
    /// learns its organisation when the hub renews the revision. Where the
    /// organisation cannot be recovered, the counter is kept for the signer
    /// that verified it rather than dropped.
    #[test]
    fn a_state_from_0_4_6_under_the_same_pin_still_refuses_rollback() {
        let old_hub = hub("hub-policy-2d20");
        let (origin, served) = settable_origin();
        let url = format!("{}/policy", origin.url());
        let digest = canonical_digest(&legacy_policy());
        let kept = old_hub.envelope(for_organisation(8, "org-fb3a", legacy_policy()));
        let loader_form =
            serde_json::to_vec(&serde_json::from_value::<PolicyFile>(legacy_policy()).unwrap())
                .unwrap();

        for kept_envelope in [Some(kept.clone()), None] {
            let home = tempfile::tempdir().unwrap();
            enrol(home.path());
            legacy_home(home.path(), &old_hub, &kept, &digest);
            if kept_envelope.is_none() {
                std::fs::remove_file(State::last_known_good_path(home.path())).unwrap();
            }
            std::fs::write(home.path().join("policy.json"), &loader_form).unwrap();
            pin(home.path(), &old_hub, &url, "org-fb3a");
            assert!(previous_pin(home.path()).is_none());

            *served.lock().unwrap() = commonmeasure_http::Response::new(
                200,
                old_hub.envelope(for_organisation(1, "org-fb3a", legacy_policy())),
            );
            let report = sync(home.path(), &unknown_edge(), now).unwrap();
            assert_eq!(report.sync.outcome, "rejected");
            assert!(
                report
                    .sync
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("rollback refused")
            );

            let mut renewed = for_organisation(8, "org-fb3a", legacy_policy());
            renewed["expires_at"] = json!("2026-09-08T00:00:00Z");
            *served.lock().unwrap() =
                commonmeasure_http::Response::new(200, old_hub.envelope(renewed));
            let report = sync(home.path(), &unknown_edge(), now).unwrap();
            assert_eq!(report.sync.outcome, "already_applied");
            let applied = State::read(home.path()).unwrap().applied.unwrap();
            assert_eq!(applied.organisation.as_deref(), Some("org-fb3a"));
            assert_eq!(applied.expires_at, "2026-09-08T00:00:00Z");
        }

        // A kept envelope that is not the applied revision's does not lend
        // the state its organisation.
        let home = tempfile::tempdir().unwrap();
        legacy_home(
            home.path(),
            &old_hub,
            &old_hub.envelope(for_organisation(9, "org-other", legacy_policy())),
            &digest,
        );
        let applied = State::read(home.path()).unwrap().applied.unwrap();
        assert_eq!(applied.pin(home.path()).organisation, None);
    }

    /// EDG-91. The enrolment a request was made under is rechecked under the
    /// policy file's lock, not before it: a pin replaced while the response
    /// waits for that lock, which `connect` and `disconnect` hold while they
    /// replace one, still supersedes the response. Nothing is written, the
    /// state file included. The pause before the pin changes gives a
    /// recheck made before the lock the chance to pass on the old pin, which
    /// this test would then catch as an accepted revision.
    #[test]
    fn a_pin_replaced_while_the_response_waits_for_the_lock_supersedes_it() {
        use std::sync::{Arc, Condvar, Mutex};
        let old_hub = hub("hub-policy-old");
        let new_hub = hub("hub-policy-new");
        let home = tempfile::tempdir().unwrap();
        enrol(home.path());
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"strict"}"#,
        )
        .unwrap();
        let body = old_hub.envelope(for_organisation(
            1,
            "org-old",
            json!({"policy_mode": "observe"}),
        ));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (arrived, arrival) = std::sync::mpsc::channel();
        let handler_gate = Arc::clone(&gate);
        let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| {
                arrived.send(()).ok();
                let (released, wake) = &*handler_gate;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = wake.wait(released).unwrap();
                }
                commonmeasure_http::Response::new(200, body.clone())
            })
            .unwrap();
        let url = format!("{}/policy", origin.url());
        pin(home.path(), &old_hub, &url, "org-old");

        let pending = std::thread::spawn({
            let home = home.path().to_owned();
            move || sync(&home, &unknown_edge(), now)
        });
        arrival
            .recv_timeout(Duration::from_secs(10))
            .expect("the request arrives");
        let held = PolicyDocument::lock(home.path()).unwrap();
        {
            let (released, wake) = &*gate;
            *released.lock().unwrap() = true;
            wake.notify_all();
        }
        std::thread::sleep(Duration::from_millis(300));
        pin(home.path(), &new_hub, &url, "org-new");
        drop(held);

        let report = pending.join().unwrap().unwrap();
        assert_eq!(report.sync.outcome, SUPERSEDED);
        assert!(!report.converged());
        let reason = report.sync.reason.unwrap();
        assert!(
            reason.contains("the pinned organisation changed from org-old to org-new; the pinned signer changed from hub-policy-old to hub-policy-new"),
            "{reason}"
        );
        assert!(report.applied.is_none());
        assert!(!State::path(home.path()).exists(), "nothing is recorded");
        assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Strict);
    }

    /// EDG-91. An empty organisation cannot have been verified, so it is
    /// unknown wherever it is read, in the applied state or in a 0.4.6
    /// state's kept envelope, and an unknown organisation under the pinned
    /// signer keeps the counter.
    #[test]
    fn an_empty_applied_or_kept_organisation_keeps_the_counter() {
        let signer = hub("hub-policy");
        let (origin, served) = settable_origin();
        let url = format!("{}/policy", origin.url());
        for variant in ["applied", "kept"] {
            let home = tempfile::tempdir().unwrap();
            enrol(home.path());
            pin(home.path(), &signer, &url, "org-1");
            let strict = json!({"policy_mode": "strict"});
            let accepted = signer.envelope(for_organisation(8, "org-1", strict));
            *served.lock().unwrap() = commonmeasure_http::Response::new(200, accepted);
            assert_eq!(
                sync(home.path(), &unknown_edge(), now)
                    .unwrap()
                    .sync
                    .outcome,
                "accepted"
            );
            let mut state = State::read(home.path()).unwrap();
            let applied = state.applied.as_mut().unwrap();
            if variant == "applied" {
                applied.organisation = Some(String::new());
            } else {
                applied.organisation = None;
                let path = State::last_known_good_path(home.path());
                let mut kept: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                kept["payload"]["organisation"] = json!("");
                std::fs::write(&path, serde_json::to_vec(&kept).unwrap()).unwrap();
            }
            state.write(home.path()).unwrap();
            assert_eq!(
                state
                    .applied
                    .as_ref()
                    .unwrap()
                    .pin(home.path())
                    .organisation,
                None,
                "{variant}"
            );

            *served.lock().unwrap() = commonmeasure_http::Response::new(
                200,
                signer.envelope(for_organisation(
                    1,
                    "org-1",
                    json!({"policy_mode": "observe"}),
                )),
            );
            let report = sync(home.path(), &unknown_edge(), now).unwrap();
            assert_eq!(report.sync.outcome, "rejected", "{variant}");
            assert!(
                report
                    .sync
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("rollback refused"),
                "{variant}: {:?}",
                report.sync.reason
            );
            assert!(report.previous.is_none(), "{variant}");
            assert_eq!(policy_mode(home.path()), crate::policy::PolicyMode::Strict);
        }
    }
}
