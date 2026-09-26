//! One-command hub enrolment: `commonmeasure connect` and `commonmeasure
//! disconnect`, and the standing check every relay run makes.
//!
//! `connect` mints an Ed25519 key pair, exchanges the owner's short-lived
//! token at the hub for an org-scoped ingest key, and registers the public
//! half in the same call with a proof of possession (the token bytes signed
//! with the private half). The hub assigns the key id, the RFC 7638
//! thumbprint of the public key, and returns it with the absolute URLs of the
//! documents it publishes the key in, which is what the edge names on every
//! signed request. Three files result under
//! `<home>`: `edge-key.json`, the private key, which never leaves the
//! machine; `relay.json`, the receiver and ingest key the relay reads; and
//! `enrolment.json`, the public facts every session records. The two files
//! that hold a credential are owner-readable only. The ingest key is
//! written straight from the exchange answer, so it never passes through a
//! chat or a shell history.
//!
//! Revocation is learnt, never assumed: each relay run asks the hub for the
//! key's standing under the ingest key and records a revocation on the
//! enrolment record, from which the next session's evidence names it.
//!
//! The hub lists a key in its key directory only while it holds a current
//! directory proof signed by that key, which only the edge can make. `connect`
//! uploads one, each relay run renews it from the status answer once it is a
//! day old, and a session or server start does the same when the enrolment
//! record says one is due ([`refresh_directory_proof`]).
//! `disconnect` revokes both credentials at the hub when it can be reached,
//! and removes the three files either way.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use commonmeasure_harness::enrolment::{
    DIRECTORY_PROOF_RETRY_SECS, DirectoryListing, EnrolledIdentity, EnrolledOrganization,
    EnrolmentRecord, HubUrlFault, Listing, ProofNeed, ProofStatement, RELEASE, RefusalConclusion,
    at_hub_origin, hub_url_accepted, origin_of, timestamp,
};
use commonmeasure_harness::identity::{EdgeKey, Identity};
use commonmeasure_harness::managed::{Deployment, Signer, policy_url_accepted};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::USER_AGENT;
use crate::config::{RelayConfig, receiver_endpoint, receiver_origin};

/// The hub routes this module speaks to, relative to the hub URL.
pub const EXCHANGE_PATH: &str = "/api/v1/enrolment/exchange";
pub const STATUS_PATH: &str = "/api/v1/enrolment/status";
pub const DISCONNECT_PATH: &str = "/api/v1/enrolment/disconnect";
/// Where the edge uploads the proof that its key agrees to be listed in the
/// hub's key directory.
pub const DIRECTORY_PROOF_PATH: &str = "/api/v1/enrolment/directory-proof";
/// Where the hub publishes the policy signer an edge's deployment file pins.
pub const SIGNER_PATH: &str = "/api/v1/policy/signer";

/// The hub's policy signer as it publishes it for pinning: the key by id
/// and raw public key, the organisation whose envelopes it signs, and the
/// path the desired policy is served at (`docs/contracts/policy-envelope.md`
/// §The hub side).
#[derive(Debug, Clone, Deserialize)]
pub struct PolicySigner {
    pub key_id: String,
    pub algorithm: String,
    pub public_key: String,
    pub organisation: String,
    /// The path the desired policy is served at, relative to the hub.
    #[serde(default)]
    pub policy_path: Option<String>,
    /// The absolute URL the desired policy is served at, built by the hub
    /// from its public origin. Preferred over the address the operator
    /// typed: the hub verifies signatures over that origin alone, so an edge
    /// that enrolled through an internal address must not pin it.
    #[serde(default)]
    pub policy_url: Option<String>,
}

impl PolicySigner {
    /// Where the desired policy is fetched from: the hub's own absolute URL
    /// where it gave one, otherwise the typed address plus the path.
    fn policy_url_for(&self, hub: &str) -> std::result::Result<String, String> {
        match (&self.policy_url, &self.policy_path) {
            (Some(url), _) => Ok(url.clone()),
            (None, Some(path)) => Ok(format!("{hub}{path}")),
            (None, None) => Err(
                "the hub's signer names neither a policy_url nor a policy_path, so there is \
                 nothing to pin"
                    .to_owned(),
            ),
        }
    }
}

/// The hub's answer to the exchange, as documented by the hub.
#[derive(Debug, Deserialize)]
struct Enrolled {
    organization: EnrolledOrganization,
    name: String,
    key_id: String,
    /// Where the hub publishes this key and what it tells publishers.
    identity: EnrolledIdentity,
    api_key: String,
    telemetry_path: String,
    /// The policy signer, when the hub includes it in the exchange answer;
    /// otherwise `connect --managed` reads it from [`SIGNER_PATH`].
    #[serde(default)]
    policy_signer: Option<PolicySigner>,
}

/// What `connect --managed` wrote to `deployment.json`.
#[derive(Debug, Clone)]
pub struct ManagedPin {
    pub key_id: String,
    pub policy_url: String,
    pub organisation: String,
    pub path: PathBuf,
    /// What `deployment.json` held before this pin replaced it, when a file
    /// was there: a local deployment, a managed one and its signer, or a
    /// file this runtime could not read.
    pub replaced: Option<String>,
}

/// What `connect` did. The relay run is the probe: the first delivery
/// through the real relay path, or the reason there was none.
pub struct ConnectReport {
    pub hub: String,
    pub organization: EnrolledOrganization,
    pub name: String,
    pub key_id: String,
    /// Where the hub publishes the key: the origin, and the directory URL a
    /// publisher reaches from it.
    pub directory: String,
    pub receiver: String,
    /// A receiver `relay.json` named before this enrolment replaced it.
    pub replaced_receiver: Option<String>,
    /// What keeping the new key listed in the key directory did.
    pub directory_proof: ProofRefresh,
    pub relay: std::result::Result<crate::RelayReport, String>,
    /// `None` unless `--managed` was asked for; then what was pinned, or
    /// why nothing was and the edge stays in `local` mode.
    pub managed: Option<std::result::Result<ManagedPin, String>>,
}

/// The hub URL `connect` was given, held to the transport rule
/// ([`hub_url_accepted`]) and to the relay's receiver rule before any
/// request or write. The receiver `connect` stores is this URL followed by
/// the hub's telemetry path, so a hub URL with credentials, a query or a
/// fragment would enrol and then leave a `relay.json` the relay refuses.
fn normalise_hub(hub: &str) -> Result<String> {
    let hub = hub.trim().trim_end_matches('/');
    if HubUrlFault::of(hub) == Some(HubUrlFault::Cleartext) {
        bail!(
            "the hub URL{at} must be https, or http to a loopback origin (127.0.0.1, \
             localhost or [::1]); nothing was sent and nothing was written",
            at = at_hub_origin(hub)
        );
    }
    if let Err(error) = receiver_endpoint(hub) {
        bail!(
            "the hub URL cannot name the relay's receiver: {error}. Give the hub's address \
             alone, with no credentials, query or fragment. Nothing was sent and nothing was \
             written"
        );
    }
    Ok(hub.to_owned())
}

/// The receiver `connect` stores for `hub`: the hub URL followed by the
/// telemetry path the hub gave, as a string so a hub URL's base path is
/// kept. It must be one the relay loads, and on the hub's own origin:
/// `hub_ingest_key` gives the key only to a receiver on that origin, and a
/// path such as `.other.example/x` appended to a portless hub URL moves the
/// host. The error completes "the telemetry path … ".
fn hub_receiver(hub: &str, telemetry_path: &str) -> std::result::Result<String, String> {
    let receiver = format!("{hub}{telemetry_path}");
    let config = RelayConfig {
        receiver: receiver.clone(),
        api_key: None,
        suppliers: None,
    };
    if let Err(error) = config.check() {
        return Err(format!("makes a receiver the relay refuses: {error}"));
    }
    let same_origin = matches!(
        (origin_of(&receiver), origin_of(hub)),
        (Some(receiver), Some(hub)) if receiver == hub
    );
    if !same_origin {
        let moved = receiver_origin(&receiver).unwrap_or_else(|| "no origin".to_owned());
        return Err(format!(
            "moves the receiver off the hub's origin, to {moved}"
        ));
    }
    Ok(receiver)
}

fn detail_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|parsed| {
            parsed["detail"]
                .as_str()
                .or_else(|| parsed["message"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            let end = text.floor_char_boundary(2 * 1024);
            if end == text.len() {
                text.into_owned()
            } else {
                format!("{}… (truncated, {} bytes)", &text[..end], body.len())
            }
        })
}

/// The `detail` of a refusal the hub wrote. On the directory-proof route
/// (`PUT /api/v1/enrolment/directory-proof`) and the status route
/// (`GET /api/v1/enrolment/status`) the hub answers 401, 404 and 409 only
/// through its error type, as `application/json` holding
/// `{"detail": "<text>"}` (`commonmeasure-hub` `crates/core/src/error.rs`),
/// and so every other refusal it writes itself on those routes. A string
/// `code` beside `detail` is also accepted, so that the hub can add one
/// without this release reading its refusals as coming from something else;
/// any other member is not. An answer in any other shape, such as an ingress
/// or proxy page, or axum's empty 404 for a path the hub does not route, did
/// not come from the hub's decision on the key. This checks the shape alone;
/// which statuses are a decision on the key is the caller's.
fn hub_refusal(response: &commonmeasure_http::Response) -> Option<String> {
    let json = response
        .headers
        .get("Content-Type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return None;
    }
    let Ok(Value::Object(members)) = serde_json::from_slice::<Value>(&response.body) else {
        return None;
    };
    let only_code_beside = members.iter().all(|(name, value)| match name.as_str() {
        "detail" => true,
        "code" => value.is_string(),
        _ => false,
    });
    match members.get("detail") {
        Some(Value::String(detail)) if only_code_beside && !detail.is_empty() => {
            Some(detail.clone())
        }
        _ => None,
    }
}

/// Why an answer whose status the hub writes only in its own shape
/// ([`hub_refusal`]) came in another: something in front of the hub
/// answered `request`, and the hub was not reached.
fn answered_in_front(request: &str, response: &commonmeasure_http::Response) -> String {
    format!(
        "the hub was not reached: the {request} was answered {} by something in front of it, \
         in a shape the hub does not answer in: {}",
        response.status,
        if response.body.is_empty() {
            "no body".to_owned()
        } else {
            detail_of(&response.body)
        }
    )
}

/// Enrol this edge with `hub` using an owner's token. Refuses when the edge
/// is already enrolled with a key that stands; a revoked key is replaced by
/// a fresh pair.
pub fn connect(home: &Path, hub: &str, token: &str, managed: bool) -> Result<ConnectReport> {
    let hub = normalise_hub(hub)?;
    let token = token.trim();
    if token.is_empty() {
        bail!("the enrolment token is empty");
    }
    if let Some(existing) = EnrolmentRecord::load(home).map_err(|error| anyhow::anyhow!(error))?
        && !existing.is_revoked()
    {
        bail!(
            "already enrolled with the hub{} as key {}; run `commonmeasure disconnect` first",
            at_hub_origin(&existing.hub),
            existing.key_id
        );
    }
    std::fs::create_dir_all(home).with_context(|| format!("create {}", home.display()))?;

    let key = EdgeKey::generate().map_err(|error| anyhow::anyhow!(error))?;
    let body = json!({
        "token": token,
        "public_key": { "kty": "OKP", "crv": "Ed25519", "x": key.jwk_x() },
        "proof": key.sign(token.as_bytes()),
    });
    let mut request = commonmeasure_http::Request::post(
        EXCHANGE_PATH,
        serde_json::to_vec(&body)?,
        "application/json",
    );
    request.headers.set("User-Agent", USER_AGENT);
    let response = commonmeasure_http::send(&format!("{hub}{EXCHANGE_PATH}"), request)
        .with_context(|| format!("reach {hub}"))?;
    if response.status != 201 {
        bail!(
            "{hub} refused the enrolment ({}): {}",
            response.status,
            detail_of(&response.body)
        );
    }
    let enrolled: Enrolled = serde_json::from_slice(&response.body)
        .with_context(|| format!("{hub} answered 201 but not with an enrolment"))?;
    let answer: Value = serde_json::from_slice(&response.body)
        .with_context(|| format!("{hub} answered 201 but not with an enrolment"))?;
    if enrolled.key_id != key.thumbprint() {
        bail!(
            "{hub} assigned key id {} but the key's thumbprint is {}; nothing was stored",
            enrolled.key_id,
            key.thumbprint()
        );
    }

    // Checked before anything is stored: the hub chose the telemetry path,
    // and a receiver the relay would refuse must not leave an enrolment
    // whose relay never runs.
    let relay_config = RelayConfig {
        receiver: hub_receiver(&hub, &enrolled.telemetry_path).map_err(|fault| {
            anyhow::anyhow!(
                "{hub} gave the telemetry path {:?}, which {fault}. Nothing was stored; revoke \
                 key {} and its ingest key on the hub's API keys page",
                enrolled.telemetry_path,
                enrolled.key_id
            )
        })?,
        api_key: Some(enrolled.api_key.clone()),
        // Enrolment names the operator's own hub, which takes every cleared
        // event; a supplier scope belongs to a supplier's receiver.
        suppliers: None,
    };
    let receiver = relay_config.receiver.clone();

    // The private key first: an enrolment record without its key is a
    // worse state than a key without its record.
    key.store(home).map_err(|error| anyhow::anyhow!(error))?;
    let record = EnrolmentRecord {
        hub: hub.clone(),
        organization: enrolled.organization.clone(),
        name: enrolled.name.clone(),
        key_id: enrolled.key_id.clone(),
        identity: enrolled.identity.clone(),
        enrolled_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    };
    record.store(home).map_err(|error| anyhow::anyhow!(error))?;

    let replaced_receiver = RelayConfig::load(home)
        .ok()
        .flatten()
        .map(|config| config.receiver);
    relay_config
        .store(home)
        .map_err(|error| anyhow::anyhow!(error))?;

    // The policy signer, pinned now, while the edge already holds the
    // credential that reads it: the one extra step a managed machine had
    // was writing this file by hand from four values an owner read off the
    // hub. A pin that cannot be made leaves the edge in local mode and says
    // why; the enrolment stands either way.
    let managed = managed.then(|| pin_signer(home, &hub, &enrolled));

    // The key's directory proof, from the exchange answer, before the first
    // relay run: an edge whose key the directory does not list signs
    // requests nobody verifies, so listing is part of enrolling.
    let directory_proof = refresh_directory_proof(
        home,
        &enrolled.api_key,
        &answer,
        commonmeasure_http::CLIENT_TIMEOUT,
    );

    // The probe: the real relay path, end to end. It delivers what the
    // operator has cleared and nothing else; with nothing cleared it still
    // asks the hub for the key's standing under the new ingest key, which is
    // the credential proven.
    let relay =
        crate::relay(home, &crate::RelayOptions::default()).map_err(|error| format!("{error:#}"));

    Ok(ConnectReport {
        hub,
        organization: enrolled.organization,
        name: enrolled.name,
        key_id: enrolled.key_id,
        directory: format!(
            "{}{}",
            enrolled.identity.origin,
            commonmeasure_harness::identity::DIRECTORY_PATH
        ),
        receiver,
        replaced_receiver,
        directory_proof,
        relay,
        managed,
    })
}

/// Read the hub's policy signer under the new ingest key, unless the
/// exchange answer already carried it, and write `deployment.json` in
/// `managed` mode pinned to it. The file is read back through the loader
/// that every synchronisation uses, so a file this runtime would refuse is
/// never left behind as the deployment.
fn pin_signer(
    home: &Path,
    hub: &str,
    enrolled: &Enrolled,
) -> std::result::Result<ManagedPin, String> {
    let signer = match &enrolled.policy_signer {
        Some(signer) => signer.clone(),
        None => fetch_signer(hub, &enrolled.api_key)?,
    };
    if signer.organisation != enrolled.organization.id {
        return Err(format!(
            "the hub's signer names organisation {} but this edge enrolled in {}; nothing was \
             pinned",
            signer.organisation, enrolled.organization.id
        ));
    }
    let path = Deployment::path(home);
    // Whatever stood in the file is reported, so a pin never silently
    // replaces a deployment an operator wrote by hand.
    let replaced = path.exists().then(|| match Deployment::read(home) {
        Ok(Deployment::Managed {
            signer: previous,
            policy_url,
            ..
        }) => format!(
            "a managed deployment pinned to signer {} with policy from {policy_url}",
            previous.key_id
        ),
        Ok(Deployment::Local) => "a local deployment".to_owned(),
        Err(reason) => format!("a deployment file this runtime could not read ({reason})"),
    });
    let policy_url = signer.policy_url_for(hub)?;
    // The synchronisation-time rule, applied before anything is written: a
    // hub that names a plain http policy URL off the machine is refused
    // here, with the enrolment kept and nothing pinned.
    if let Err(reason) = policy_url_accepted(&policy_url) {
        return Err(format!(
            "the hub's signer names a policy URL this runtime will not fetch from ({reason}); \
             nothing was pinned. The hub must name an https policy URL; once it does, run \
             `commonmeasure disconnect`, then `commonmeasure connect --managed` again"
        ));
    }
    let deployment = Deployment::Managed {
        signer: Signer {
            key_id: signer.key_id.clone(),
            algorithm: signer.algorithm.clone(),
            public_key: signer.public_key.clone(),
        },
        policy_url: policy_url.clone(),
        organisation: signer.organisation.clone(),
    };
    let mut encoded = serde_json::to_vec_pretty(&deployment)
        .map_err(|error| format!("the deployment could not be encoded: {error}"))?;
    encoded.push(b'\n');
    // Through a temporary file of this writer's own and a rename, so a
    // synchronisation reading the file mid-write sees the old deployment or
    // the new one, and two `connect --managed` runs cannot rename each
    // other's file away.
    commonmeasure_harness::declaration::replace(&path, &encoded)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    if let Err(refused) = Deployment::read(home) {
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "the hub's signer does not make a deployment this runtime accepts ({refused}); the \
             file was not kept"
        ));
    }
    Ok(ManagedPin {
        key_id: signer.key_id,
        policy_url,
        organisation: signer.organisation,
        path,
        replaced,
    })
}

/// `GET <hub>/api/v1/policy/signer` under the ingest key. A hub that does
/// not let the edge read it says so, and the operator is told what an owner
/// can read instead.
fn fetch_signer(hub: &str, api_key: &str) -> std::result::Result<PolicySigner, String> {
    hub_url_accepted(hub)?;
    let mut request = commonmeasure_http::Request::get(SIGNER_PATH);
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("Accept", "application/json");
    request.headers.set("X-API-Key", api_key);
    let response = commonmeasure_http::send(&format!("{hub}{SIGNER_PATH}"), request)
        .map_err(|error| format!("reach {hub}{SIGNER_PATH}: {error:#}"))?;
    match response.status {
        200 => serde_json::from_slice(&response.body).map_err(|error| {
            format!("{hub}{SIGNER_PATH} answered 200 but not with a signer: {error}")
        }),
        401 | 403 => Err(format!(
            "{hub}{SIGNER_PATH} did not let this edge read the policy signer ({}: {}). An owner \
             can read that route and write {} by hand, or the hub can allow the ingest key on \
             it",
            response.status,
            detail_of(&response.body),
            "~/.commonmeasure/deployment.json"
        )),
        status => Err(format!(
            "{hub}{SIGNER_PATH} answered {status}: {}",
            detail_of(&response.body)
        )),
    }
}

/// The key's standing as the hub answered it on one relay run, or why the
/// hub could not say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    Enrolled {
        key_id: String,
    },
    Revoked {
        key_id: String,
        /// Absent when a 401 supplied no revocation time.
        revoked_at: Option<String>,
        revocation: String,
    },
    /// The hub gave no usable answer; the standing recorded before stands.
    Unchecked {
        key_id: String,
        reason: String,
    },
}

impl Standing {
    pub fn key_id(&self) -> &str {
        match self {
            Self::Enrolled { key_id }
            | Self::Revoked { key_id, .. }
            | Self::Unchecked { key_id, .. } => key_id,
        }
    }
}

impl std::fmt::Display for Standing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enrolled { key_id } => write!(formatter, "edge key {key_id}: enrolled"),
            Self::Revoked {
                key_id,
                revoked_at,
                revocation,
            } => match revoked_at {
                Some(at) => write!(
                    formatter,
                    "edge key {key_id}: revoked at {at} by the {revocation}; it is out of the key \
                     directory"
                ),
                // A 401 says the hub refused the enrolled ingest key, not what
                // became of the edge key or its directory listing.
                None => write!(
                    formatter,
                    "edge key {key_id}: revoked ({revocation}): the hub refused the enrolled \
                     ingest key, so this edge stopped signing; whether the key directory still \
                     lists the key is not known here"
                ),
            },
            Self::Unchecked { key_id, reason } => write!(
                formatter,
                "edge key {key_id}: standing not checked this run ({reason})"
            ),
        }
    }
}

/// What one relay run learnt about the enrolled key: its standing, and what
/// keeping it listed in the key directory did.
#[derive(Debug, Clone)]
pub struct EnrolmentCheck {
    pub standing: Standing,
    /// `None` when no answer from the hub could say, which is every standing
    /// but `Enrolled`.
    pub directory_proof: Option<ProofRefresh>,
}

/// Ask the hub for the enrolled key's standing, record a revocation on the
/// enrolment record, and on a key that stands keep its directory proof
/// current from the same answer. `None` when this edge is not enrolled.
/// A hub that does not answer leaves the recorded standing. An unsafe hub
/// URL or a revocation that cannot be stored fails the run.
pub fn check_standing(home: &Path, api_key: Option<&str>) -> Result<Option<EnrolmentCheck>> {
    let Some((mut standing, answer)) = standing(home, api_key)? else {
        return Ok(None);
    };
    let directory_proof =
        match (&standing, answer, api_key) {
            (Standing::Enrolled { .. }, Some(answer), Some(api_key)) => Some(
                refresh_directory_proof(home, api_key, &answer, commonmeasure_http::CLIENT_TIMEOUT),
            ),
            _ => None,
        };
    // An upload can revoke the key after the status request said it stood.
    if let Some(record) = EnrolmentRecord::load(home).map_err(|reason| anyhow::anyhow!(reason))?
        && record.is_revoked()
    {
        standing = recorded_revocation(&record);
    }
    Ok(Some(EnrolmentCheck {
        standing,
        directory_proof,
    }))
}

fn recorded_revocation(record: &EnrolmentRecord) -> Standing {
    Standing::Revoked {
        key_id: record.key_id.clone(),
        revoked_at: record.revoked_at.clone(),
        revocation: record
            .revocation
            .clone()
            .unwrap_or_else(|| "hub".to_owned()),
    }
}

/// The standing, with the status answer it was read from when the hub gave
/// one.
fn standing(home: &Path, api_key: Option<&str>) -> Result<Option<(Standing, Option<Value>)>> {
    let Some(mut record) = EnrolmentRecord::load(home).map_err(|error| anyhow::anyhow!(error))?
    else {
        return Ok(None);
    };
    hub_url_accepted(&record.hub).map_err(|reason| anyhow::anyhow!(reason))?;
    let key_id = record.key_id.clone();
    let Some(api_key) = api_key else {
        return Ok(Some((
            Standing::Unchecked {
                key_id,
                reason: "no ingest key for the hub is configured".to_owned(),
            },
            None,
        )));
    };
    // Before the request: an answer withdraws only a refusal learnt before
    // it was asked for.
    let asked_at = Utc::now();
    let response = match status(&record.hub, api_key, commonmeasure_http::CLIENT_TIMEOUT) {
        Ok(response) => response,
        Err(reason) => {
            return Ok(Some((Standing::Unchecked { key_id, reason }, None)));
        }
    };
    let answer = match response {
        StatusAnswer::Refused(detail) => {
            if let Err(source) = enrolled_key_presented(&record, home, api_key) {
                return Ok(Some((
                    Standing::Unchecked {
                        key_id,
                        reason: format!(
                            "the hub refused the key this run presented ({detail}); {source}, \
                             so the refusal says nothing about this enrolment"
                        ),
                    },
                    None,
                )));
            }
            record
                .record_refusal(home, &detail)
                .map_err(|reason| anyhow::anyhow!(reason))?;
            return Ok(Some((recorded_revocation(&record), None)));
        }
        StatusAnswer::Other(reason) => {
            return Ok(Some((Standing::Unchecked { key_id, reason }, None)));
        }
        StatusAnswer::Standing(answer) => answer,
    };
    if answer["key_id"].as_str() != Some(key_id.as_str()) {
        return Ok(Some((
            Standing::Unchecked {
                key_id,
                reason: "the hub answered for a different key".to_owned(),
            },
            None,
        )));
    }
    match (answer["revoked_at"].as_str(), answer["revocation"].as_str()) {
        (Some(revoked_at), Some(revocation)) => {
            // Kept even after a 401 revocation, whose learnt time stays:
            // the hub's statement is what makes the revocation permanent.
            record
                .record_revocation(home, revoked_at, revocation)
                .map_err(|reason| anyhow::anyhow!(reason))?;
            Ok(Some((
                Standing::Revoked {
                    key_id,
                    revoked_at: Some(revoked_at.to_owned()),
                    revocation: revocation.to_owned(),
                },
                Some(answer),
            )))
        }
        _ => {
            record
                .withdraw_refusal(home, asked_at)
                .map_err(|reason| anyhow::anyhow!(reason))?;
            Ok(Some((Standing::Enrolled { key_id }, Some(answer))))
        }
    }
}

/// Whether `api_key` is the enrolled ingest key: the key in `relay.json`
/// for the hub's origin ([`EnrolmentRecord::hub_ingest_key`]). Only a 401
/// for that key is a revocation of this enrolment. A relay run presents any
/// other key only because `--api-key` gave it, and a mistyped key must not
/// revoke a valid enrolment. `Err` names where the key came from.
fn enrolled_key_presented(
    record: &EnrolmentRecord,
    home: &Path,
    api_key: &str,
) -> std::result::Result<(), String> {
    match record.hub_ingest_key(home) {
        Ok(Some(enrolled)) if enrolled == api_key => Ok(()),
        Ok(Some(_)) => Err(
            "that key was given with --api-key and is not the enrolled ingest key in relay.json"
                .to_owned(),
        ),
        Ok(None) => Err(
            "relay.json holds no ingest key for the hub, so that key was given with --api-key"
                .to_owned(),
        ),
        Err(reason) => Err(format!(
            "{reason}, so whether that key is the enrolled ingest key is not known"
        )),
    }
}

/// The hub's answer to `GET /api/v1/enrolment/status`.
enum StatusAnswer {
    /// 200 with a JSON body.
    Standing(Value),
    /// 401 in the hub's own shape ([`hub_refusal`]): the hub refused the
    /// ingest key.
    Refused(String),
    /// Any other answer, described. An answer in any other shape, whatever
    /// its status, is here, described as the hub not reached: something in
    /// front of the hub answered, and it says nothing about the key.
    Other(String),
}

/// `GET <hub>/api/v1/enrolment/status` under the ingest key. `Err` is a hub
/// that gave no HTTP answer.
fn status(
    hub: &str,
    api_key: &str,
    budget: std::time::Duration,
) -> std::result::Result<StatusAnswer, String> {
    hub_url_accepted(hub)?;
    let mut request = commonmeasure_http::Request::get(STATUS_PATH);
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("X-API-Key", api_key);
    let response =
        commonmeasure_http::send_with_timeout(&format!("{hub}{STATUS_PATH}"), request, budget)
            .map_err(|error| format!("{error:#}"))?;
    Ok(match response.status {
        200 => match serde_json::from_slice(&response.body) {
            Ok(answer) => StatusAnswer::Standing(answer),
            Err(error) => StatusAnswer::Other(format!(
                "the hub answered 200 but not with a standing: {error}"
            )),
        },
        status => match (status, hub_refusal(&response)) {
            (401, Some(detail)) => StatusAnswer::Refused(detail),
            (_, Some(detail)) => {
                StatusAnswer::Other(format!("the hub answered {status}: {detail}"))
            }
            (_, None) => StatusAnswer::Other(answered_in_front("status request", &response)),
        },
    })
}

/// What one attempt to keep this edge's key listed in the key directory did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofAction {
    /// The hub's answer carried no `directory_proof` statement: a hub that
    /// takes no proofs. Nothing was sent.
    NotStated,
    /// The proof the hub holds was signed within the day. Nothing was sent.
    Current,
    /// A new proof was signed and the hub accepted it.
    Uploaded,
    /// No proof was sent, for the reason given.
    NotSent(String),
    /// A proof was sent and the hub refused it, or never answered.
    Failed(String),
}

/// One attempt to keep the key listed, with the listing that resulted as
/// the enrolment record now states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofRefresh {
    pub action: ProofAction,
    pub listing: Listing,
}

impl std::fmt::Display for ProofRefresh {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match &self.action {
            ProofAction::NotStated => "the hub takes no directory proofs".to_owned(),
            ProofAction::Current => "current, nothing sent".to_owned(),
            ProofAction::Uploaded => "signed and accepted by the hub".to_owned(),
            ProofAction::NotSent(reason) => format!("not sent: {reason}"),
            ProofAction::Failed(reason) => format!("not accepted: {reason}"),
        };
        match &self.listing {
            Listing::ListedUntil(until) => write!(
                formatter,
                "directory proof: {action}; the key directory lists this key until {}",
                timestamp(*until)
            ),
            Listing::Unlisted(reason) => write!(
                formatter,
                "directory proof: {action}; the key directory does not list this key: {reason}"
            ),
        }
    }
}

/// Read the hub's `directory_proof` statement from an exchange or status
/// `answer`, sign and upload a proof when one is due, and write what the hub
/// now holds on the enrolment record. Never fails the caller: an unanswered
/// or refused upload leaves the recorded proof and the reason.
///
/// The edge signs only for the authority of the origin it enrolled under,
/// whatever authority the hub states, so a hub cannot obtain this key's
/// agreement to be listed anywhere else.
pub fn refresh_directory_proof(
    home: &Path,
    api_key: &str,
    answer: &Value,
    budget: std::time::Duration,
) -> ProofRefresh {
    let now = Utc::now();
    let mut record = match EnrolmentRecord::load(home) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return ProofRefresh {
                action: ProofAction::NotSent("this edge is not enrolled".to_owned()),
                listing: Listing::Unlisted("this edge is not enrolled".to_owned()),
            };
        }
        Err(reason) => {
            return ProofRefresh {
                action: ProofAction::NotSent(reason.clone()),
                listing: Listing::Unlisted(reason),
            };
        }
    };
    if let Err(reason) = hub_url_accepted(&record.hub) {
        return conclude_proof(home, now, None, ProofAction::NotSent(reason));
    }
    let Some(member) = answer.get("directory_proof") else {
        return conclude_proof(home, now, None, ProofAction::NotStated);
    };
    let stated: ProofStatement = match serde_json::from_value(member.clone()) {
        Ok(stated) => stated,
        Err(error) => {
            let reason = format!("the hub's directory_proof statement is not readable: {error}");
            return conclude_proof(home, now, None, ProofAction::NotSent(reason));
        }
    };
    if record.is_revoked() {
        let reason = "the key is revoked".to_owned();
        return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
    }
    let authority = match record.enrolled_authority() {
        Ok(authority) => authority,
        Err(reason) => {
            return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
        }
    };
    let held = DirectoryListing::load(home, &record.key_id).ok().flatten();
    let last_uploaded_release = held
        .as_ref()
        .and_then(|listing| listing.last_uploaded_release.as_deref());
    match stated.need(&authority, now, last_uploaded_release) {
        ProofNeed::Current => return conclude_proof(home, now, Some(stated), ProofAction::Current),
        ProofNeed::Unsignable(reason) => {
            return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
        }
        ProofNeed::Due(_) => {}
    }

    // Supplying this release isolates the age/expiry decision without parsing
    // a human-readable due reason. Status reads must not restart the delay.
    if stated.need(&authority, now, Some(RELEASE)) == ProofNeed::Current
        && let Some(mut held) = held
        && held.failure.is_some()
        && chrono::DateTime::parse_from_rfc3339(&held.checked_at).is_ok_and(|checked| {
            (now - checked.with_timezone(&Utc)).num_seconds() < DIRECTORY_PROOF_RETRY_SECS
        })
    {
        held.stated = Some(stated);
        return store_proof(
            home,
            now,
            &record,
            held,
            ProofAction::NotSent("waiting to retry the failed release upload".to_owned()),
        );
    }

    let identity = match Identity::load(home) {
        Ok(identity) => identity,
        Err(reason) => {
            return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
        }
    };
    let Some(signer) = identity.signer() else {
        let reason = identity
            .presented()
            .unsigned
            .unwrap_or_else(|| "this edge holds no key to sign with".to_owned());
        return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
    };
    let proof = match signer.directory_proof(&authority, now.timestamp(), stated.lifetime_secs) {
        Ok(proof) => proof,
        Err(reason) => {
            return conclude_proof(home, now, Some(stated), ProofAction::NotSent(reason));
        }
    };
    let body = json!({
        "key_id": signer.key_id(),
        "release": RELEASE,
        "signature_input": proof.signature_input,
        "signature": proof.signature,
    });
    let mut request = commonmeasure_http::Request::post(
        DIRECTORY_PROOF_PATH,
        body.to_string().into_bytes(),
        "application/json",
    );
    request.method = "PUT".to_owned();
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("X-API-Key", api_key);
    let sent = commonmeasure_http::send_with_timeout(
        &format!("{}{DIRECTORY_PROOF_PATH}", record.hub),
        request,
        budget,
    );
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            let reason = format!("the hub could not be reached: {error:#}");
            return conclude_proof(home, now, Some(stated), ProofAction::Failed(reason));
        }
    };
    if !(200..300).contains(&response.status) {
        // Only the hub's own 401, 404 or 409 is a decision on the key. Every
        // refusal the hub's handlers and middleware write on this route is in
        // its own shape, so an answer in another, whatever its status, is
        // described as coming from something in front of the hub. The hub's
        // empty 503 on its request timeout and axum's plain-text body
        // rejections read that way too; neither is a decision on the proof.
        let key_refused = matches!(response.status, 401 | 404 | 409);
        let from_hub = hub_refusal(&response);
        // A 429 or a 5xx in the hub's shape is the hub failing to take the
        // proof, which rules on nothing.
        let hub_fault = response.status == 429 || response.status >= 500;
        let reason = match &from_hub {
            Some(detail) if hub_fault => format!(
                "the hub could not take the proof ({}): {detail}; nothing was concluded from it",
                response.status
            ),
            Some(detail) => format!("the hub refused the proof ({}): {detail}", response.status),
            None => answered_in_front("upload", &response),
        };
        let refusal = from_hub.filter(|_| key_refused);
        if response.status == 401
            && let Some(detail) = &refusal
            && enrolled_key_presented(&record, home, api_key).is_ok()
            && let Err(error) = record.record_refusal(home, detail)
        {
            return conclude_proof(
                home,
                now,
                Some(stated),
                ProofAction::Failed(format!("{reason}; could not record revocation: {error}")),
            );
        }
        let mut stated = stated;
        if let Some(detail) = refusal {
            stated.listed = Some(false);
            stated.unlisted_reason = Some(detail.clone());
            stated.concluded = Some(RefusalConclusion {
                status: response.status,
                detail,
            });
        }
        return conclude_proof(home, now, Some(stated), ProofAction::Failed(reason));
    }
    // A complete statement is authoritative, including a decision to withhold
    // a key whose proof is still current.
    let answer = match serde_json::from_slice::<Value>(&response.body) {
        Ok(answer) => answer,
        Err(error) if !response.body.is_empty() => {
            return conclude_proof_with_failure(
                home,
                now,
                Some(stated),
                ProofAction::Uploaded,
                Some(format!("the hub's upload answer is not readable: {error}")),
            );
        }
        Err(_) => Value::Null,
    };
    if let Some(member) = answer.get("directory_proof") {
        return match serde_json::from_value::<ProofStatement>(member.clone()) {
            Ok(stated) => conclude_proof(home, now, Some(stated), ProofAction::Uploaded),
            Err(error) => conclude_proof_with_failure(
                home,
                now,
                Some(stated),
                ProofAction::Uploaded,
                Some(format!(
                    "the hub's directory_proof statement is not readable: {error}"
                )),
            ),
        };
    }
    // The hub keeps whichever proof expires later, so what it holds now
    // expires no earlier than this one. Where it states the expiry, that is
    // the fact recorded.
    let answered = answer
        .get("expires_at")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let ours = chrono::DateTime::from_timestamp(proof.expires, 0).map(timestamp);
    let held = answered.or_else(|| {
        let earlier = stated
            .expires_at
            .as_deref()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp());
        match earlier {
            Some(earlier) if earlier > proof.expires => stated.expires_at.clone(),
            _ => ours,
        }
    });
    let stated = ProofStatement {
        expires_at: held,
        listed: None,
        unlisted_reason: None,
        concluded: None,
        ..stated
    };
    conclude_proof(home, now, Some(stated), ProofAction::Uploaded)
}

/// Write what was learnt to `<home>/directory-listing.json`, never to the
/// enrolment record, whose shape every released binary reads. The record is
/// re-read just before, so a revocation another process recorded in the
/// meantime decides the listing. A listing that cannot be written leaves the
/// action's reason and says so.
fn conclude_proof(
    home: &Path,
    now: chrono::DateTime<Utc>,
    stated: Option<ProofStatement>,
    action: ProofAction,
) -> ProofRefresh {
    let failure = match &action {
        ProofAction::NotSent(reason) | ProofAction::Failed(reason) => Some(reason.clone()),
        ProofAction::NotStated | ProofAction::Current | ProofAction::Uploaded => None,
    };
    conclude_proof_with_failure(home, now, stated, action, failure)
}

fn conclude_proof_with_failure(
    home: &Path,
    now: chrono::DateTime<Utc>,
    stated: Option<ProofStatement>,
    action: ProofAction,
    failure: Option<String>,
) -> ProofRefresh {
    let record = match EnrolmentRecord::load(home) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return ProofRefresh {
                action,
                listing: Listing::Unlisted("this edge is no longer enrolled".to_owned()),
            };
        }
        Err(reason) => {
            return ProofRefresh {
                action,
                listing: Listing::Unlisted(reason),
            };
        }
    };
    let last_uploaded_release = if matches!(action, ProofAction::Uploaded) {
        Some(RELEASE.to_owned())
    } else {
        DirectoryListing::load(home, &record.key_id)
            .ok()
            .flatten()
            .and_then(|listing| listing.last_uploaded_release)
    };
    let held = DirectoryListing {
        key_id: record.key_id.clone(),
        last_uploaded_release,
        checked_at: timestamp(now),
        stated,
        failure,
    };
    store_proof(home, now, &record, held, action)
}

fn store_proof(
    home: &Path,
    now: chrono::DateTime<Utc>,
    record: &EnrolmentRecord,
    held: DirectoryListing,
    action: ProofAction,
) -> ProofRefresh {
    let listing = record.listing(Some(&held), now);
    match held.store(home) {
        Ok(()) => ProofRefresh { action, listing },
        Err(reason) => ProofRefresh {
            action,
            listing: Listing::Unlisted(format!(
                "what the hub holds could not be recorded: {reason}"
            )),
        },
    }
}

/// At a session or server start: when the enrolment record says a directory
/// proof is due, ask the hub for its statement under the ingest key and
/// refresh the proof from it, the two requests inside `budget` together.
/// `None` when nothing was due, in which case no request was made.
pub fn refresh_directory_proof_if_due(
    home: &Path,
    budget: std::time::Duration,
) -> Option<ProofRefresh> {
    let started = std::time::Instant::now();
    let now = Utc::now();
    let mut record = EnrolmentRecord::load(home).ok().flatten()?;
    let held = DirectoryListing::load(home, &record.key_id).ok().flatten();
    if !record.directory_proof_due_at(home, now) {
        return None;
    }
    let stated = held.and_then(|listing| listing.stated);
    if let Err(reason) = hub_url_accepted(&record.hub) {
        return Some(conclude_proof(
            home,
            now,
            stated,
            ProofAction::NotSent(reason),
        ));
    }
    // The `relay.json` key, where that file still names this hub.
    let api_key = record.hub_ingest_key(home).ok().flatten();
    let Some(api_key) = api_key else {
        let reason = "no ingest key is configured for the hub, so it could not be asked".to_owned();
        return Some(conclude_proof(
            home,
            now,
            stated,
            ProofAction::NotSent(reason),
        ));
    };
    let answer = match status(&record.hub, &api_key, budget) {
        Ok(StatusAnswer::Standing(answer)) => answer,
        Ok(StatusAnswer::Refused(detail)) => {
            let reason = match record.record_refusal(home, &detail) {
                Ok(()) => format!("the key is revoked: {detail}"),
                Err(error) => format!(
                    "the hub refused the ingest key ({detail}); could not record revocation: {error}"
                ),
            };
            return Some(conclude_proof(
                home,
                now,
                stated,
                ProofAction::NotSent(reason),
            ));
        }
        Ok(StatusAnswer::Other(reason)) => {
            return Some(conclude_proof(
                home,
                now,
                stated,
                ProofAction::NotSent(reason),
            ));
        }
        Err(reason) => {
            let reason = format!("the hub could not be reached: {reason}");
            return Some(conclude_proof(
                home,
                now,
                stated,
                ProofAction::NotSent(reason),
            ));
        }
    };
    if answer["key_id"].as_str() != Some(record.key_id.as_str()) || answer["revoked_at"].is_string()
    {
        let reason = "the hub did not answer for this key as one that stands; the next relay run \
                      reports its standing"
            .to_owned();
        return Some(conclude_proof(
            home,
            now,
            stated,
            ProofAction::NotSent(reason),
        ));
    }
    let remaining = budget.saturating_sub(started.elapsed());
    Some(refresh_directory_proof(home, &api_key, &answer, remaining))
}

/// What `disconnect` did.
pub struct DisconnectReport {
    /// ` at <origin>` for the hub left, or nothing where the stored URL has
    /// no origin ([`at_hub_origin`]): a hub URL stored by 0.4.1's `connect`
    /// can carry credentials, and this is printed.
    pub at_hub: String,
    pub key_id: String,
    /// The edge's name, which the hub labels both keys with.
    pub name: String,
    /// `Ok` when the hub revoked both credentials; `Err` names why it did
    /// not, in which case both keys must be revoked from the hub.
    pub revoked_at_hub: std::result::Result<(), String>,
    pub removed: Vec<PathBuf>,
    /// The policy URL of a managed deployment under the hub being left,
    /// removed with the enrolment so the edge does not stay pinned to a hub
    /// it can no longer authenticate to.
    pub removed_deployment: Option<String>,
    /// The policy URL of a managed deployment kept because it names another
    /// hub; the operator is told rather than second-guessed. Both deployment
    /// fields name the URL as ` at <origin>`, like [`Self::at_hub`].
    pub kept_deployment: Option<String>,
    /// The sessions that pointed at a registered instance and no longer do.
    /// The instances were registered under the key given up here; their
    /// records under `instances/` stay. `Err` names why the pointers could
    /// not be removed; the rest of the disconnect has happened by then.
    pub retired_instance_sessions: std::result::Result<Vec<String>, String>,
}

/// Leave the hub: revoke both credentials there when it can be reached, and
/// remove the relay configuration, the private key and the enrolment
/// record either way. The relay's durable account of what was delivered
/// stays, and so do the instance records; the pointers from sessions to
/// registered instances go, because those instances belong to the key given
/// up and the hub answers `404` for them to any key this edge enrols later
/// (`docs/contracts/session-evidence.md` §Instance registration).
pub fn disconnect(home: &Path) -> Result<DisconnectReport> {
    let Some(record) = EnrolmentRecord::load(home).map_err(|error| anyhow::anyhow!(error))? else {
        bail!(
            "not enrolled: {} does not exist, so there is nothing to disconnect",
            EnrolmentRecord::path(home).display()
        );
    };
    // The `relay.json` key, where that file still names this hub.
    let api_key = record.hub_ingest_key(home).ok().flatten();

    let at_hub = at_hub_origin(&record.hub);
    let revoked_at_hub = match HubUrlFault::of(&record.hub).map_or(Ok(api_key), Err) {
        Err(fault) => Err(format!(
            "the hub was not asked, because the stored hub URL{at_hub} {}",
            fault.reason()
        )),
        Ok(None) => {
            Err("no ingest key is configured for the hub, so it could not be asked".to_owned())
        }
        Ok(Some(api_key)) => {
            let mut request =
                commonmeasure_http::Request::post(DISCONNECT_PATH, Vec::new(), "application/json");
            // The hosted load balancer requires explicit framing for an empty POST.
            request.headers.set("Content-Length", "0");
            request.headers.set("User-Agent", USER_AGENT);
            request.headers.set("X-API-Key", &api_key);
            match commonmeasure_http::send(&format!("{}{DISCONNECT_PATH}", record.hub), request) {
                Ok(response) if response.status == 204 => Ok(()),
                Ok(response) => Err(format!(
                    "the hub answered {}: {}",
                    response.status,
                    detail_of(&response.body)
                )),
                Err(error) => Err(format!("the hub could not be reached: {error:#}")),
            }
        }
    };

    let mut removed = Vec::new();
    for path in [home.join("relay.json"), EdgeKey::path(home)] {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed.push(path);
        }
    }
    // Under the record's lock, so a relay run that read the record before
    // this cannot write it back once it learns of the revocation above.
    if EnrolmentRecord::remove(home).map_err(|error| anyhow::anyhow!(error))? {
        removed.push(EnrolmentRecord::path(home));
    }
    let listing = DirectoryListing::path(home);
    if listing.exists() {
        std::fs::remove_file(&listing).with_context(|| format!("remove {}", listing.display()))?;
        removed.push(listing);
    }
    // A managed deployment under this hub goes with the enrolment: the
    // policy endpoint authenticates the edge by the key just given up, so
    // the pin could only ever answer unauthenticated from here on.
    let mut removed_deployment = None;
    let mut kept_deployment = None;
    if let Ok(Deployment::Managed { policy_url, .. }) = Deployment::read(home) {
        // 0.4.1 pinned the stored hub URL followed by the signer's policy
        // path, so the policy URL can carry the same credentials.
        let policy_at = at_hub_origin(&policy_url);
        if policy_url.starts_with(&record.hub) {
            let path = Deployment::path(home);
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed.push(path);
            removed_deployment = Some(policy_at);
        } else {
            kept_deployment = Some(policy_at);
        }
    }
    let retired_instance_sessions = commonmeasure_harness::instance::retire_session_pointers(home);
    Ok(DisconnectReport {
        at_hub,
        key_id: record.key_id,
        name: record.name,
        revoked_at_hub,
        removed,
        removed_deployment,
        kept_deployment,
        retired_instance_sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The telemetry path is appended to the hub URL as a string, which
    /// keeps a hub URL's base path; against the ordinary portless hub URL a
    /// path that does not start with `/` can move the host or add a port.
    /// Such a receiver is refused, because `hub_ingest_key` would withhold
    /// the key from it after a `connect` that reported success. The loopback
    /// enrolment fixture always has a port, which cannot show the move, so
    /// the rule is exercised here directly.
    #[test]
    fn a_telemetry_path_that_moves_the_receiver_off_the_hub_is_refused() {
        for (hub, path) in [
            ("https://hub.example", ".evil.example/x"),
            ("https://hub.example", ":9999/x"),
            ("https://hub.example", "api.evil.example/x"),
            ("https://hub.example", "https://evil.example/x"),
            ("https://hub.example", "./api/v1/telemetry"),
        ] {
            let fault = hub_receiver(hub, path).expect_err(path);
            assert!(fault.contains("off the hub's origin"), "{path}: {fault}");
        }
        assert!(
            hub_receiver("https://hub.example", ".evil.example/x")
                .unwrap_err()
                .ends_with("to https://hub.example.evil.example")
        );
        for (hub, path, receiver) in [
            (
                "https://hub.example",
                "/api/v1/telemetry",
                "https://hub.example/api/v1/telemetry",
            ),
            (
                "https://hub.example/base",
                "/api/v1/telemetry",
                "https://hub.example/base/api/v1/telemetry",
            ),
            (
                "http://127.0.0.1:8080",
                "/api/v1/telemetry",
                "http://127.0.0.1:8080/api/v1/telemetry",
            ),
        ] {
            assert_eq!(hub_receiver(hub, path).as_deref(), Ok(receiver));
        }
        let fault = hub_receiver("https://hub.example", "/t?tenant=a").unwrap_err();
        assert!(fault.contains("relay refuses"), "{fault}");
    }

    #[test]
    fn hub_error_text_is_preserved_in_full() {
        let reason = "the hub's explanation; ".repeat(200);
        for field in ["detail", "message"] {
            let body = json!({field: reason});
            assert_eq!(detail_of(&serde_json::to_vec(&body).unwrap()), reason);
        }
    }

    /// Only the hub's error body, `{"detail": …}` alone as JSON, is a refusal
    /// the hub wrote. A proxy's JSON with other members, the right body under
    /// another media type, and an empty or HTML body are not.
    #[test]
    fn only_the_hubs_own_error_shape_is_a_hub_refusal() {
        let answer = |content_type: Option<&str>, body: &str| {
            let mut response = commonmeasure_http::Response::new(404, body.as_bytes().to_vec());
            response.headers.remove("Content-Type");
            if let Some(content_type) = content_type {
                response.headers.set("Content-Type", content_type);
            }
            hub_refusal(&response)
        };
        let hub = r#"{"detail":"no edge key with that id is enrolled under this credential"}"#;
        let coded = r#"{"detail":"no edge key with that id is enrolled under this credential","code":"key_not_enrolled"}"#;
        for content_type in ["application/json", "Application/JSON; charset=utf-8"] {
            for body in [hub, coded] {
                assert_eq!(
                    answer(Some(content_type), body).as_deref(),
                    Some("no edge key with that id is enrolled under this credential"),
                    "{body}"
                );
            }
        }
        for (content_type, body) in [
            (Some("text/html"), hub),
            (None, hub),
            (Some("text/plain"), ""),
            (None, ""),
            (Some("text/html"), "<html><body>404 Not Found</body></html>"),
            (
                Some("application/json"),
                r#"{"detail":"Not Found","status":404}"#,
            ),
            (Some("application/json"), r#"{"message":"no route"}"#),
            (
                Some("application/json"),
                r#"{"detail":"refused","code":404}"#,
            ),
            (
                Some("application/json"),
                r#"{"detail":"refused","code":null}"#,
            ),
            (
                Some("application/json"),
                r#"{"detail":"refused","code":"key_not_enrolled","status":404}"#,
            ),
            (Some("application/json"), r#"{"code":"key_not_enrolled"}"#),
            (Some("application/json"), r#"{"detail":""}"#),
            (Some("application/json"), r#"{"detail":["x"]}"#),
            (Some("application/json"), r#"["detail"]"#),
            (Some("application/json"), "not json"),
        ] {
            assert_eq!(answer(content_type, body), None, "{content_type:?} {body}");
        }
    }

    #[test]
    fn raw_error_text_is_bounded_at_a_character_boundary() {
        for body in ["small error".to_owned(), "x".repeat(2048)] {
            assert_eq!(detail_of(body.as_bytes()), body);
        }
        let body = format!("{}界tail", "x".repeat(2047));
        assert_eq!(
            detail_of(body.as_bytes()),
            format!("{}… (truncated, {} bytes)", "x".repeat(2047), body.len())
        );
        let invalid_utf8 = vec![0xff; 3000];
        let detail = detail_of(&invalid_utf8);
        assert!(detail.len() < 2100);
        assert!(detail.ends_with("… (truncated, 3000 bytes)"));
    }

    /// An enrolment record that cannot be read fails identity loading, and
    /// neither `connect` nor `disconnect` repairs it: both read it first.
    /// Moving the file aside is what lets a new session run unsigned and
    /// `connect` go on to ask the hub. Port 1 on loopback refuses the
    /// connection, which shows `connect` got past the record without a hub.
    #[test]
    fn an_unreadable_record_is_repaired_by_moving_it_aside_not_by_connect() {
        let home = tempfile::tempdir().unwrap();
        let key = EdgeKey::generate().unwrap();
        key.store(home.path()).unwrap();
        let record = EnrolmentRecord::path(home.path());
        std::fs::write(&record, b"{ not json").unwrap();

        assert!(Identity::load(home.path()).is_err());
        let hub = "http://127.0.0.1:1";
        let Err(refused) = connect(home.path(), hub, "token", false) else {
            panic!("connect enrolled over an unreadable record");
        };
        assert!(
            format!("{refused:#}").contains("not a valid enrolment record"),
            "{refused:#}"
        );
        let Err(refused) = disconnect(home.path()) else {
            panic!("disconnect ran over an unreadable record");
        };
        assert!(
            format!("{refused:#}").contains("not a valid enrolment record"),
            "{refused:#}"
        );

        std::fs::rename(&record, home.path().join("enrolment.json.broken")).unwrap();
        assert!(Identity::load(home.path()).unwrap().signer().is_none());
        let Err(unreachable) = connect(home.path(), hub, "token", false) else {
            panic!("nothing listens on port 1");
        };
        assert!(
            format!("{unreachable:#}").contains(&format!("reach {hub}")),
            "{unreachable:#}"
        );
    }

    /// Two `connect --managed` runs at once each pin the signer. Each writes
    /// `deployment.json` through its own temporary file, so neither renames
    /// the other's away or reads back the other's half-written file.
    #[test]
    fn concurrent_pins_each_land_whole() {
        let home = tempfile::tempdir().unwrap();
        let enrolled = |n: usize| Enrolled {
            organization: EnrolledOrganization {
                id: "org-1".to_owned(),
                name: "Org".to_owned(),
            },
            name: "laptop".to_owned(),
            key_id: "key-1".to_owned(),
            identity: EnrolledIdentity {
                origin: "https://hub.example".to_owned(),
                bot_page: "https://hub.example/bot".to_owned(),
                contact: None,
            },
            api_key: "ingest".to_owned(),
            telemetry_path: "/telemetry".to_owned(),
            policy_signer: Some(PolicySigner {
                key_id: format!("signer-{n}-{}", "x".repeat(300 * n)),
                algorithm: "ed25519".to_owned(),
                public_key: "11".repeat(32),
                organisation: "org-1".to_owned(),
                policy_path: Some("/api/v1/policy".to_owned()),
                policy_url: None,
            }),
        };
        let failures = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for n in 0..4 {
                let (home, failures, enrolled) = (home.path(), &failures, enrolled(n));
                scope.spawn(move || {
                    for _ in 0..40 {
                        if let Err(error) = pin_signer(home, "https://hub.example", &enrolled) {
                            failures.lock().unwrap().push(error);
                        }
                    }
                });
            }
        });
        assert_eq!(failures.into_inner().unwrap(), Vec::<String>::new());
        assert!(matches!(
            Deployment::read(home.path()).unwrap(),
            Deployment::Managed { .. }
        ));
        let leftovers: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_hub_url_without_a_scheme_is_refused() {
        assert!(normalise_hub("hub.example").is_err());
        assert_eq!(
            normalise_hub("https://hub.example/").unwrap(),
            "https://hub.example"
        );
    }

    #[test]
    fn a_cleartext_hub_url_is_named_by_its_origin_alone() {
        let refusal = normalise_hub("http://user:ak_PLANTED@hub.remote.example")
            .unwrap_err()
            .to_string();
        assert!(
            refusal.starts_with("the hub URL at http://hub.remote.example must be https"),
            "{refusal}"
        );
        assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
        let refusal = normalise_hub("hub.example/ak_PLANTED")
            .unwrap_err()
            .to_string();
        assert!(
            refusal.starts_with("the hub URL must be https"),
            "{refusal}"
        );
        assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
    }

    /// An https hub URL with credentials passes the transport rule, so it is
    /// refused for the credentials, not as cleartext.
    #[test]
    fn an_https_hub_url_with_credentials_is_refused_for_them() {
        let refusal = normalise_hub("https://ops:ak_PLANTED@hub.example")
            .unwrap_err()
            .to_string();
        assert!(
            refusal.starts_with(
                "the hub URL cannot name the relay's receiver: the receiver at \
                 https://hub.example carries credentials in the URL"
            ),
            "{refusal}"
        );
        assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
    }
}
