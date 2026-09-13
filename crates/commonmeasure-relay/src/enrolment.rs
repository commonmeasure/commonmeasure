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
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::USER_AGENT;
use crate::config::RelayConfig;

/// The hub routes this module speaks to, relative to the hub URL.
pub const EXCHANGE_PATH: &str = "/api/v1/enrolment/exchange";
pub const STATUS_PATH: &str = "/api/v1/enrolment/status";
pub const DISCONNECT_PATH: &str = "/api/v1/enrolment/disconnect";

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
pub fn connect(home: &Path, hub: &str, token: &str) -> Result<ConnectReport> {
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
        api_key: Some(enrolled.api_key),
    }
    .store(home)
    .map_err(|error| anyhow::anyhow!(error))?;

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
    })
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
    /// The hub refused the ingest key itself: revoked at the hub, or never
    /// its. Delivery under it fails the same way.
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
                "edge key {key_id}: the hub refused the ingest key ({detail}); revoke the \
                 enrolment from the hub and run `commonmeasure connect` again"
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
    Ok(DisconnectReport {
        hub: record.hub,
        key_id: record.key_id,
        revoked_at_hub,
        removed,
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
