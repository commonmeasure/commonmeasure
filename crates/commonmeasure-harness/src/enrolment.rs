//! The enrolment record: what this edge knows about its own enrolment with
//! a hub, read by every process that records session evidence so a session
//! can name the key id it runs under.
//!
//! `<home>/enrolment.json` holds the public facts only: the hub, the
//! organisation, the key id the hub assigned, where the hub publishes that
//! key, and the key's standing. The ingest key lives in `relay.json` and the
//! private key in `edge-key.json`; nothing here can read or write either.
//! Absent means not enrolled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the hub publishes this edge's key and what it tells publishers, as
/// the hub returned it at enrolment. Absolute URLs on the origin the hub
/// serves its identity documents on: the edge never builds one, so it cannot
/// sign under an origin it was not enrolled on and cannot point a publisher
/// at a page that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrolledIdentity {
    /// The origin serving this fleet's Web Bot Auth key directory, named in
    /// `Signature-Agent` on every signed request. The directory itself is at
    /// the well-known path under it, which the draft fixes and the verifier
    /// appends (`crate::identity::DIRECTORY_PATH`).
    pub origin: String,
    /// The page explaining the bot, named in the user agent.
    pub bot_page: String,
    /// How a publisher reaches Common Measure about this fleet, named in the
    /// user agent. Absent where the hub's deploy published none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

/// The organisation the hub enrolled this edge in, as the hub named it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolledOrganization {
    pub id: String,
    pub name: String,
}

/// Unknown fields are load errors, like the policy file's: a misspelled key
/// must fail the load rather than read as an edge that is not enrolled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrolmentRecord {
    /// The hub's base URL as `connect` was given it.
    pub hub: String,
    pub organization: EnrolledOrganization,
    /// The name the owner gave the edge when minting the token; the hub's
    /// label for both keys.
    pub name: String,
    /// The RFC 7638 thumbprint the hub assigned: the pseudonymous identifier
    /// publishers see, the id every session records, and the agent identifier
    /// on the Content Telemetry wire.
    pub key_id: String,
    pub identity: EnrolledIdentity,
    pub enrolled_at: String,
    /// Set once a relay run learnt from the hub that the key is revoked.
    /// `revocation` says which side revoked it (`owner` or `edge`);
    /// `learnt_at` is when this edge found out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_learnt_at: Option<String>,
}

/// The key id this edge enrolled under, read from `<home>/enrolment.json`.
/// `None` when the edge is not enrolled; a revoked key still answers,
/// because it is still the identity this edge holds and every session names
/// it, and `EnrolmentRecord::load` says whether it stands. The one function
/// the rest of the binary reads the edge's identity from.
pub fn enrolled_key_id(home: &Path) -> Result<Option<String>, String> {
    Ok(EnrolmentRecord::load(home)?.map(|record| record.key_id))
}

impl EnrolmentRecord {
    pub fn path(home: &Path) -> PathBuf {
        home.join("enrolment.json")
    }

    /// Load `<home>/enrolment.json`. Absent means not enrolled.
    pub fn load(home: &Path) -> Result<Option<Self>, String> {
        let source = Self::path(home);
        if !source.exists() {
            return Ok(None);
        }
        let encoded = std::fs::read(&source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        let record: Self = serde_json::from_slice(&encoded).map_err(|error| {
            format!(
                "{} is not a valid enrolment record: {error}",
                source.display()
            )
        })?;
        Ok(Some(record))
    }

    /// Write atomically (write-then-rename): a crash mid-write must leave
    /// either the old record or the new one, never a half-written file that
    /// reads as a malformed enrolment.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let path = Self::path(home);
        let tmp = path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise enrolment record: {error}"))?;
        std::fs::write(&tmp, encoded)
            .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(())
    }

    /// `enrolled` while the key stands, `revoked` once a revocation was
    /// learnt. The word a session record carries.
    pub fn standing(&self) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else {
            "enrolled"
        }
    }
}
