//! The enrolment record: what this edge knows about its own enrolment with
//! a hub, read by every process that records session evidence so a session
//! can name the key id it runs under.
//!
//! `<home>/enrolment.json` holds the public facts only: the hub, the
//! organisation, the key id the hub assigned, where the hub publishes that
//! key, and the key's standing. The ingest key lives in `relay.json` and the
//! private key in `edge-key.json`; nothing here can read or write either.
//! Absent means not enrolled.
//!
//! What the edge learns about its key's place in the hub's key directory
//! lives beside it in `<home>/directory-listing.json`, not in the record:
//! the record keeps the shape every released binary reads, so binaries of
//! different versions can share one operator home.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::identity::{DIRECTORY_PROOF_MARGIN_SECS, authority_of};

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

/// How long after its creation a held directory proof is replaced on the
/// next contact with the hub. A proof lives for the lifetime the hub
/// states, a week by default, so an edge that reaches the hub at least once
/// in that week stays listed, and signs at most once a day.
pub const DIRECTORY_PROOF_REFRESH_SECS: i64 = 86_400;

/// How long a session or server start waits before trying again after an
/// attempt to hold a current proof failed. A relay run or `connect` always
/// asks; a start that has the host waiting does not pay for a hub that did
/// not answer a moment ago.
pub const DIRECTORY_PROOF_RETRY_SECS: i64 = 3_600;

/// What this edge last learnt about its listing in the hub's key directory,
/// held in `<home>/directory-listing.json`. A key is listed only while the
/// hub holds a current proof signed by it
/// (`crate::identity::SigningIdentity::directory_proof`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryListing {
    /// The key this was learnt for. A file naming another key is from an
    /// earlier enrolment and is not read.
    pub key_id: String,
    /// When this edge last asked the hub, or tried to.
    pub checked_at: String,
    /// What the hub stated. Absent when its answer carried no statement,
    /// which is a hub that does not take directory proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stated: Option<ProofStatement>,
    /// Why the last attempt to hold a current proof at the hub did not
    /// succeed. Absent when it did, or when none was due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// The hub's `directory_proof` statement on the exchange and status answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofStatement {
    /// The authority the hub serves its key directory from, which is the
    /// authority a proof must cover.
    pub authority: String,
    /// The longest validity window the hub accepts on a proof.
    pub lifetime_secs: i64,
    /// The expiry of the proof the hub holds for this key at that
    /// authority; absent when it holds none.
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Whether a new directory proof should be signed and uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofNeed {
    /// The held proof is recent enough; nothing is sent.
    Current,
    /// A proof should be signed and uploaded, for the reason given.
    Due(String),
    /// No proof this edge could make would be accepted, for the reason
    /// given; nothing is sent.
    Unsignable(String),
}

/// Whether this edge's key is in the hub's key directory, as the edge last
/// learnt it: the fact a session record carries beside the key id, because a
/// signed request from a key the directory does not list verifies nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Listed until the hub stops serving the held proof: its expiry less
    /// the margin the hub keeps for cached copies.
    ListedUntil(DateTime<Utc>),
    Unlisted(String),
}

impl ProofStatement {
    /// Whether a proof is due, against the authority this edge enrolled
    /// under. The edge signs only for that authority, never for one a hub
    /// names later, so an edge cannot be listed at an origin it was not
    /// enrolled on.
    pub fn need(&self, enrolled_authority: &str, now: DateTime<Utc>) -> ProofNeed {
        if self.authority != enrolled_authority {
            return ProofNeed::Unsignable(format!(
                "the hub serves its key directory from {} but this edge enrolled under {}, and \
                 its signed requests name that origin; a proof is made only for the enrolled \
                 origin, so enrol again to be listed",
                self.authority, enrolled_authority
            ));
        }
        if self.lifetime_secs <= DIRECTORY_PROOF_MARGIN_SECS {
            return ProofNeed::Unsignable(format!(
                "the hub accepts proofs valid for at most {} s, and it serves one only while at \
                 least {DIRECTORY_PROOF_MARGIN_SECS} s of it remain",
                self.lifetime_secs
            ));
        }
        let Some(expires_at) = &self.expires_at else {
            return ProofNeed::Due("the hub holds no directory proof for this key".to_owned());
        };
        let Ok(expires) = DateTime::parse_from_rfc3339(expires_at) else {
            return ProofNeed::Due(format!(
                "the hub states the held proof expires at {expires_at}, which is not an RFC 3339 \
                 time"
            ));
        };
        let remaining = (expires.with_timezone(&Utc) - now).num_seconds();
        if remaining < DIRECTORY_PROOF_MARGIN_SECS {
            return ProofNeed::Due(format!(
                "the held proof expires at {expires_at}, too soon for the hub to serve it"
            ));
        }
        if self.lifetime_secs - remaining > DIRECTORY_PROOF_REFRESH_SECS {
            return ProofNeed::Due(format!(
                "the held proof, expiring at {expires_at}, was signed more than a day ago"
            ));
        }
        ProofNeed::Current
    }
}

impl DirectoryListing {
    pub fn path(home: &Path) -> PathBuf {
        home.join("directory-listing.json")
    }

    /// Load the listing held for `key_id`. `None` when there is no file, or
    /// when the file is for another key.
    pub fn load(home: &Path, key_id: &str) -> Result<Option<Self>, String> {
        let source = Self::path(home);
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let listing: Self = serde_json::from_slice(&encoded).map_err(|error| {
            format!(
                "{} is not a valid directory listing: {error}",
                source.display()
            )
        })?;
        Ok((listing.key_id == key_id).then_some(listing))
    }

    /// Write atomically, as the enrolment record is written.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let path = Self::path(home);
        let tmp = path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise directory listing: {error}"))?;
        std::fs::write(&tmp, encoded)
            .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(())
    }
}

/// `now` as every record writes a time.
pub fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn seconds_since(at: &str, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| (now - at.with_timezone(&Utc)).num_seconds())
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

    /// The authority the enrolled origin names, which is the one authority
    /// this edge signs directory proofs for.
    pub fn enrolled_authority(&self) -> Result<String, String> {
        authority_of(&self.identity.origin)
    }

    /// Whether a session or server start should ask the hub and upload a
    /// directory proof, judged from this record and the listing held under
    /// `home` alone, so that a start with nothing due makes no request. A
    /// listing file that cannot be read is due, so the refresh replaces it.
    pub fn directory_proof_due_at(&self, home: &Path, now: DateTime<Utc>) -> bool {
        match DirectoryListing::load(home, &self.key_id) {
            Ok(listing) => self.directory_proof_due(listing.as_ref(), now),
            Err(_) => self.revoked_at.is_none(),
        }
    }

    /// Whether the hub's key directory lists this key, from the listing held
    /// under `home`.
    pub fn listing_at(&self, home: &Path, now: DateTime<Utc>) -> Listing {
        match DirectoryListing::load(home, &self.key_id) {
            Ok(listing) => self.listing(listing.as_ref(), now),
            Err(reason) => Listing::Unlisted(reason),
        }
    }

    /// [`Self::directory_proof_due_at`] over a listing already read. A relay
    /// run and `connect` judge from the hub's own answer instead
    /// ([`ProofStatement::need`]).
    pub fn directory_proof_due(
        &self,
        listing: Option<&DirectoryListing>,
        now: DateTime<Utc>,
    ) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        let Some(listing) = listing else {
            return true;
        };
        let checked_ago = seconds_since(&listing.checked_at, now);
        if listing.failure.is_some()
            && checked_ago.is_some_and(|ago| ago < DIRECTORY_PROOF_RETRY_SECS)
        {
            return false;
        }
        let asked_today = checked_ago.is_some_and(|ago| ago < DIRECTORY_PROOF_REFRESH_SECS);
        let Some(stated) = &listing.stated else {
            // A hub that took no proofs when last asked is asked again once
            // a day, so an edge learns when it starts to.
            return !asked_today;
        };
        let Ok(authority) = self.enrolled_authority() else {
            return false;
        };
        match stated.need(&authority, now) {
            ProofNeed::Current => false,
            ProofNeed::Due(_) => true,
            ProofNeed::Unsignable(_) => !asked_today,
        }
    }

    /// Whether the hub's key directory lists this key, as this edge last
    /// learnt it in `listing`.
    pub fn listing(&self, listing: Option<&DirectoryListing>, now: DateTime<Utc>) -> Listing {
        if let Some(revoked_at) = &self.revoked_at {
            return Listing::Unlisted(format!(
                "the key was revoked at {revoked_at}, and the directory lists no revoked key"
            ));
        }
        let Some(listing) = listing else {
            return Listing::Unlisted(
                "this edge has not yet learnt from the hub whether it holds a directory proof \
                 for this key"
                    .to_owned(),
            );
        };
        let failure = listing
            .failure
            .as_deref()
            .map(|failure| format!("; the last attempt to upload one failed: {failure}"))
            .unwrap_or_default();
        let Some(stated) = &listing.stated else {
            return Listing::Unlisted(format!(
                "the hub stated no directory proof when last asked at {}, so the directory \
                 carries no signature by this key and a verifier that uses only signed keys \
                 does not use it{failure}",
                listing.checked_at
            ));
        };
        match self.enrolled_authority() {
            Ok(authority) if authority == stated.authority => {}
            Ok(authority) => {
                return Listing::Unlisted(format!(
                    "the hub serves its key directory from {} but this edge enrolled under \
                     {authority}{failure}",
                    stated.authority
                ));
            }
            Err(reason) => return Listing::Unlisted(reason),
        }
        let Some(expires) = stated
            .expires_at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        else {
            return Listing::Unlisted(format!(
                "the hub held no directory proof for this key when last asked at {}{failure}",
                listing.checked_at
            ));
        };
        let until =
            expires.with_timezone(&Utc) - chrono::Duration::seconds(DIRECTORY_PROOF_MARGIN_SECS);
        if until <= now {
            return Listing::Unlisted(format!(
                "the directory proof held at the hub left the directory at {}; the edge signs a \
                 new one on its next contact with the hub{failure}",
                timestamp(until)
            ));
        }
        Listing::ListedUntil(until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> EnrolmentRecord {
        EnrolmentRecord {
            hub: "https://hub.example".to_owned(),
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
            enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
            revoked_at: None,
            revocation: None,
            revocation_learnt_at: None,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("a time")
            .with_timezone(&Utc)
    }

    fn statement(expires_at: Option<&str>) -> ProofStatement {
        ProofStatement {
            authority: "hub.example".to_owned(),
            lifetime_secs: 604_800,
            expires_at: expires_at.map(str::to_owned),
        }
    }

    /// A held proof signed within the day is current; one signed more than
    /// a day ago, one too close to expiry to be served, and none at all are
    /// due.
    #[test]
    fn a_proof_is_due_when_absent_more_than_a_day_old_or_about_to_leave() {
        let now = at("2026-09-14T12:00:00Z");
        assert_eq!(
            statement(Some("2026-09-21T11:00:00Z")).need("hub.example", now),
            ProofNeed::Current,
            "signed an hour ago"
        );
        assert!(matches!(
            statement(Some("2026-09-20T11:59:00Z")).need("hub.example", now),
            ProofNeed::Due(_)
        ));
        assert!(matches!(
            statement(Some("2026-09-14T13:59:00Z")).need("hub.example", now),
            ProofNeed::Due(_)
        ));
        assert!(matches!(
            statement(None).need("hub.example", now),
            ProofNeed::Due(_)
        ));
    }

    /// The edge signs only for the authority it enrolled under, and not at
    /// all when the hub's lifetime leaves nothing it would serve.
    #[test]
    fn a_proof_for_another_authority_or_an_unservable_lifetime_is_not_made() {
        let now = at("2026-09-14T12:00:00Z");
        let ProofNeed::Unsignable(reason) = statement(None).need("other.example", now) else {
            panic!("another authority is not signed for");
        };
        assert!(reason.contains("enrolled under other.example"), "{reason}");
        let short = ProofStatement {
            lifetime_secs: 7_200,
            ..statement(None)
        };
        assert!(matches!(
            short.need("hub.example", now),
            ProofNeed::Unsignable(_)
        ));
    }

    /// A start asks the hub only when the record says a proof is due, and
    /// not again within the hour after a failed attempt.
    #[test]
    fn the_record_says_when_a_start_should_ask() {
        let now = at("2026-09-14T12:00:00Z");
        assert!(record().directory_proof_due(None, now), "never asked");

        let current = DirectoryListing {
            key_id: "key-1".to_owned(),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(Some("2026-09-21T11:00:00Z"))),
            failure: None,
        };
        assert!(!record().directory_proof_due(Some(&current), now));

        let failed = DirectoryListing {
            key_id: "key-1".to_owned(),
            checked_at: "2026-09-14T11:30:00.000Z".to_owned(),
            stated: Some(statement(None)),
            failure: Some("the hub could not be reached".to_owned()),
        };
        assert!(!record().directory_proof_due(Some(&failed), now));
        assert!(record().directory_proof_due(Some(&failed), at("2026-09-14T12:31:00Z")));

        let old_hub = DirectoryListing {
            stated: None,
            ..current.clone()
        };
        assert!(!record().directory_proof_due(Some(&old_hub), now));
        assert!(record().directory_proof_due(Some(&old_hub), at("2026-09-15T11:30:00Z")));

        let mut revoked = record();
        revoked.revoked_at = Some("2026-09-14T00:00:00.000Z".to_owned());
        assert!(!revoked.directory_proof_due(None, now));
    }

    /// The listing is its own file, bound to the key it was learnt for: a
    /// file left by an earlier enrolment is not read, and the enrolment record
    /// never carries it.
    #[test]
    fn the_listing_file_is_read_only_for_the_key_it_names() {
        let home = tempfile::tempdir().expect("home");
        let now = at("2026-09-14T12:00:00Z");
        record().store(home.path()).expect("store the record");
        assert!(record().directory_proof_due_at(home.path(), now), "no file");

        let listing = DirectoryListing {
            key_id: "key-1".to_owned(),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(Some("2026-09-21T11:00:00Z"))),
            failure: None,
        };
        listing.store(home.path()).expect("store the listing");
        assert_eq!(
            DirectoryListing::load(home.path(), "key-1").expect("load"),
            Some(listing.clone())
        );
        assert!(!record().directory_proof_due_at(home.path(), now));
        assert_eq!(
            record().listing_at(home.path(), now),
            Listing::ListedUntil(at("2026-09-21T09:00:00Z"))
        );
        assert_eq!(
            DirectoryListing::load(home.path(), "key-2").expect("load"),
            None,
            "a listing for another key is not this key's"
        );

        let written: serde_json::Value = serde_json::from_slice(
            &std::fs::read(EnrolmentRecord::path(home.path())).expect("read"),
        )
        .expect("json");
        assert!(written.get("directory_listing").is_none(), "{written}");

        std::fs::write(DirectoryListing::path(home.path()), "{").expect("write");
        let Listing::Unlisted(reason) = record().listing_at(home.path(), now) else {
            panic!("an unreadable listing is not a listing");
        };
        assert!(reason.contains("not a valid directory listing"), "{reason}");
        assert!(record().directory_proof_due_at(home.path(), now));
    }

    /// The listing is the held proof's expiry less the serving margin, and
    /// every other state is unlisted with its reason.
    #[test]
    fn the_listing_names_until_when_or_why_not() {
        let now = at("2026-09-14T12:00:00Z");
        let listed = DirectoryListing {
            key_id: "key-1".to_owned(),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(Some("2026-09-21T11:00:00Z"))),
            failure: None,
        };
        assert_eq!(
            record().listing(Some(&listed), now),
            Listing::ListedUntil(at("2026-09-21T09:00:00Z"))
        );
        let Listing::Unlisted(reason) = record().listing(Some(&listed), at("2026-09-21T09:00:00Z"))
        else {
            panic!("a proof inside the margin is not served");
        };
        assert!(reason.contains("left the directory"), "{reason}");

        let Listing::Unlisted(reason) = record().listing(None, now) else {
            panic!("never asked");
        };
        assert!(reason.contains("not yet learnt"), "{reason}");

        let refused = DirectoryListing {
            key_id: "key-1".to_owned(),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(None)),
            failure: Some("the hub refused the proof (422): created is in the future".to_owned()),
        };
        let Listing::Unlisted(reason) = record().listing(Some(&refused), now) else {
            panic!("no proof held");
        };
        assert!(reason.contains("created is in the future"), "{reason}");
    }
}
