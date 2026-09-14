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
//! `disconnect` revokes both credentials at the hub when it can be reached,
//! and removes the three files either way.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use commonmeasure_harness::enrolment::{EnrolledIdentity, EnrolledOrganization, EnrolmentRecord};
use commonmeasure_harness::identity::EdgeKey;
use commonmeasure_harness::managed::{Deployment, Signer, policy_url_accepted};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::USER_AGENT;
use crate::config::RelayConfig;

/// The hub routes this module speaks to, relative to the hub URL.
pub const EXCHANGE_PATH: &str = "/api/v1/enrolment/exchange";
pub const STATUS_PATH: &str = "/api/v1/enrolment/status";
pub const DISCONNECT_PATH: &str = "/api/v1/enrolment/disconnect";
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
    pub relay: std::result::Result<crate::RelayReport, String>,
    /// `None` unless `--managed` was asked for; then what was pinned, or
    /// why nothing was and the edge stays in `local` mode.
    pub managed: Option<std::result::Result<ManagedPin, String>>,
}

fn normalise_hub(hub: &str) -> Result<String> {
    let hub = hub.trim().trim_end_matches('/');
    if !(hub.starts_with("http://") || hub.starts_with("https://")) {
        bail!("the hub URL must start with http:// or https://, got {hub}");
    }
    Ok(hub.to_owned())
}

fn detail_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|parsed| parsed["detail"].as_str().map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(160).collect())
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
        && existing.revoked_at.is_none()
    {
        bail!(
            "already enrolled with {} as key {}; run `commonmeasure disconnect` first",
            existing.hub,
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
    if enrolled.key_id != key.thumbprint() {
        bail!(
            "{hub} assigned key id {} but the key's thumbprint is {}; nothing was stored",
            enrolled.key_id,
            key.thumbprint()
        );
    }

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
    let receiver = format!("{hub}{}", enrolled.telemetry_path);
    RelayConfig {
        receiver: receiver.clone(),
        api_key: Some(enrolled.api_key.clone()),
    }
    .store(home)
    .map_err(|error| anyhow::anyhow!(error))?;

    // The policy signer, pinned now, while the edge already holds the
    // credential that reads it: the one extra step a managed machine had
    // was writing this file by hand from four values an owner read off the
    // hub. A pin that cannot be made leaves the edge in local mode and says
    // why; the enrolment stands either way.
    let managed = managed.then(|| pin_signer(home, &hub, &enrolled));

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
             nothing was pinned"
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
    // Through a temporary neighbour and a rename, so a synchronisation
    // reading the file mid-write sees the old deployment or the new one.
    let temporary = path.with_extension("json.commonmeasure-tmp");
    std::fs::write(&temporary, encoded)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("cannot replace {}: {error}", path.display()));
    }
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
        revoked_at: String,
        revocation: String,
    },
    /// The hub refused the ingest key itself, which is what a revoked key,
    /// a closed organisation or a key that was never this hub's answers.
    /// Delivery under it fails the same way, so the operator reads this
    /// line before the failure.
    IngestKeyRefused {
        key_id: String,
        detail: String,
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
            | Self::IngestKeyRefused { key_id, .. }
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
            } => write!(
                formatter,
                "edge key {key_id}: revoked at {revoked_at} by the {revocation}; it is out of the \
                 key directory"
            ),
            Self::IngestKeyRefused { key_id, detail } => write!(
                formatter,
                "edge key {key_id}: revoked, or the ingest key is not this hub's: the hub \
                 refused it ({detail}) and accepts nothing from this edge. Run `commonmeasure \
                 disconnect`, then `commonmeasure connect` with a new token, to rejoin"
            ),
            Self::Unchecked { key_id, reason } => write!(
                formatter,
                "edge key {key_id}: standing not checked this run ({reason})"
            ),
        }
    }
}

/// Ask the hub for the enrolled key's standing and record a revocation on
/// the enrolment record. `None` when this edge is not enrolled. Never fails
/// a relay run: a hub that does not answer leaves the recorded standing.
pub fn check_standing(home: &Path, api_key: Option<&str>) -> Result<Option<Standing>> {
    let Some(mut record) = EnrolmentRecord::load(home).map_err(|error| anyhow::anyhow!(error))?
    else {
        return Ok(None);
    };
    let key_id = record.key_id.clone();
    let Some(api_key) = api_key else {
        return Ok(Some(Standing::Unchecked {
            key_id,
            reason: "no ingest key is configured".to_owned(),
        }));
    };
    let mut request = commonmeasure_http::Request::get(STATUS_PATH);
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("X-API-Key", api_key);
    let response = match commonmeasure_http::send(&format!("{}{STATUS_PATH}", record.hub), request)
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(Some(Standing::Unchecked {
                key_id,
                reason: format!("{error:#}"),
            }));
        }
    };
    if response.status == 401 {
        return Ok(Some(Standing::IngestKeyRefused {
            key_id,
            detail: detail_of(&response.body),
        }));
    }
    if response.status != 200 {
        return Ok(Some(Standing::Unchecked {
            key_id,
            reason: format!(
                "the hub answered {}: {}",
                response.status,
                detail_of(&response.body)
            ),
        }));
    }
    let answer: Value = match serde_json::from_slice(&response.body) {
        Ok(answer) => answer,
        Err(error) => {
            return Ok(Some(Standing::Unchecked {
                key_id,
                reason: format!("the hub answered 200 but not with a standing: {error}"),
            }));
        }
    };
    if answer["key_id"].as_str() != Some(key_id.as_str()) {
        return Ok(Some(Standing::Unchecked {
            key_id,
            reason: "the hub answered for a different key".to_owned(),
        }));
    }
    match (answer["revoked_at"].as_str(), answer["revocation"].as_str()) {
        (Some(revoked_at), Some(revocation)) => {
            if record.revoked_at.is_none() {
                record.revoked_at = Some(revoked_at.to_owned());
                record.revocation = Some(revocation.to_owned());
                record.revocation_learnt_at =
                    Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
                record.store(home).map_err(|error| anyhow::anyhow!(error))?;
            }
            Ok(Some(Standing::Revoked {
                key_id,
                revoked_at: revoked_at.to_owned(),
                revocation: revocation.to_owned(),
            }))
        }
        _ => Ok(Some(Standing::Enrolled { key_id })),
    }
}

/// What `disconnect` did.
pub struct DisconnectReport {
    pub hub: String,
    pub key_id: String,
    /// `Ok` when the hub revoked both credentials; `Err` names why it did
    /// not, in which case the key must be revoked from the hub.
    pub revoked_at_hub: std::result::Result<(), String>,
    pub removed: Vec<PathBuf>,
    /// The policy URL of a managed deployment under the hub being left,
    /// removed with the enrolment so the edge does not stay pinned to a hub
    /// it can no longer authenticate to.
    pub removed_deployment: Option<String>,
    /// The policy URL of a managed deployment kept because it names another
    /// hub; the operator is told rather than second-guessed.
    pub kept_deployment: Option<String>,
}

/// Leave the hub: revoke both credentials there when it can be reached, and
/// remove the relay configuration, the private key and the enrolment
/// record either way. The relay's durable account of what was delivered
/// stays.
pub fn disconnect(home: &Path) -> Result<DisconnectReport> {
    let Some(record) = EnrolmentRecord::load(home).map_err(|error| anyhow::anyhow!(error))? else {
        bail!(
            "not enrolled: {} does not exist, so there is nothing to disconnect",
            EnrolmentRecord::path(home).display()
        );
    };
    let api_key = RelayConfig::load(home)
        .ok()
        .flatten()
        .and_then(|config| config.api_key);

    let revoked_at_hub = match api_key {
        None => Err("no ingest key is configured, so the hub could not be asked".to_owned()),
        Some(api_key) => {
            let mut request =
                commonmeasure_http::Request::post(DISCONNECT_PATH, Vec::new(), "application/json");
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
    for path in [
        home.join("relay.json"),
        EdgeKey::path(home),
        EnrolmentRecord::path(home),
    ] {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed.push(path);
        }
    }
    // A managed deployment under this hub goes with the enrolment: the
    // policy endpoint authenticates the edge by the key just given up, so
    // the pin could only ever answer unauthenticated from here on.
    let mut removed_deployment = None;
    let mut kept_deployment = None;
    if let Ok(Deployment::Managed { policy_url, .. }) = Deployment::read(home) {
        if policy_url.starts_with(&record.hub) {
            let path = Deployment::path(home);
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed.push(path);
            removed_deployment = Some(policy_url);
        } else {
            kept_deployment = Some(policy_url);
        }
    }
    Ok(DisconnectReport {
        hub: record.hub,
        key_id: record.key_id,
        revoked_at_hub,
        removed,
        removed_deployment,
        kept_deployment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_url_without_a_scheme_is_refused() {
        assert!(normalise_hub("hub.example").is_err());
        assert_eq!(
            normalise_hub("https://hub.example/").unwrap(),
            "https://hub.example"
        );
    }
}
