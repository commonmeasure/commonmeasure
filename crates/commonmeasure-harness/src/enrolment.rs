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

/// The release reported and recorded by every directory proof upload.
pub const RELEASE: &str = env!("CARGO_PKG_VERSION");

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
    /// The hub's revocation time, when supplied. A 401 leaves it unknown.
    /// `revocation` names the side (`owner` or `edge`) or the hub's refusal;
    /// `revocation_learnt_at` is when this edge learnt it was revoked.
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
/// attempt to hold a current proof failed. Relay runs also wait when only
/// the release changed; proofs due by age or expiry still upload each run.
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
    /// When this edge last asked the hub, or tried to. During a release-only
    /// retry delay, status reads preserve the failed attempt's time.
    pub checked_at: String,
    /// The binary release that made the last accepted proof upload. This is
    /// the edge's own record, independent of the hub's statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_uploaded_release: Option<String>,
    /// What the hub stated, with any conclusion this edge drew from a
    /// refused upload ([`ProofStatement::concluded`]). Absent when its answer
    /// carried no statement, which is a hub that does not take directory
    /// proofs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "held_statement"
    )]
    pub stated: Option<ProofStatement>,
    /// Why the last attempt to hold a current proof at the hub did not
    /// succeed, or why an accepted upload's statement could not be read.
    /// Absent when both succeeded, or when none was due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// The hub's `directory_proof` statement on exchange, upload and status answers.
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
    /// The hub's whole listing decision, independent of proof expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed: Option<bool>,
    /// The hub's explanation, preserved verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlisted_reason: Option<String>,
    /// This edge's conclusion that the key is unlisted, drawn from the hub's
    /// refusal of an upload; never something the hub stated. Held only in
    /// `directory-listing.json`, as `edge_conclusion` inside `stated`: the
    /// hub's statement is read without it, so no member the hub sends can
    /// set it. While it is set, `listed` and `unlisted_reason` repeat it
    /// for readers of 0.4.2 and earlier, which ignore the member. The next
    /// statement from the hub replaces it.
    #[serde(skip)]
    pub concluded: Option<RefusalConclusion>,
}

/// A refusal of a directory proof upload in the hub's own error shape, from
/// which the edge concludes the key is unlisted. Unknown members are ignored,
/// so a later release can store more beside these without this one failing
/// the whole listing file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalConclusion {
    /// The refusal's HTTP status: 401, 404 or 409.
    pub status: u16,
    /// The hub's `detail`, verbatim.
    pub detail: String,
}

/// `stated` as `directory-listing.json` holds it: the hub's statement with
/// the edge's conclusion beside it as `edge_conclusion`. A 0.4.2 reader
/// parses the statement without the member, since `ProofStatement` ignores
/// unknown members, and reads the repeated `listed: false`.
mod held_statement {
    use super::{ProofStatement, RefusalConclusion};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Held<S> {
        #[serde(flatten)]
        statement: S,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge_conclusion: Option<RefusalConclusion>,
    }

    pub fn serialize<S: Serializer>(
        stated: &Option<ProofStatement>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        stated
            .as_ref()
            .map(|statement| Held {
                statement,
                edge_conclusion: statement.concluded.clone(),
            })
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ProofStatement>, D::Error> {
        let held = Option::<Held<ProofStatement>>::deserialize(deserializer)?;
        Ok(held.map(|held| ProofStatement {
            concluded: held.edge_conclusion,
            ..held.statement
        }))
    }
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
    /// enrolled on. A new release uploads even while the held proof is current,
    /// so the hub can reconsider its listing against the reported release.
    pub fn need(
        &self,
        enrolled_authority: &str,
        now: DateTime<Utc>,
        last_uploaded_release: Option<&str>,
    ) -> ProofNeed {
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
        if last_uploaded_release != Some(RELEASE) {
            return ProofNeed::Due(
                "this release has no recorded accepted directory proof upload".to_owned(),
            );
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

    /// Write atomically, through a temporary file of this writer's own, as
    /// the enrolment record is written.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise directory listing: {error}"))?;
        crate::declaration::replace(&Self::path(home), &encoded)
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

/// The host-based origin of `url`: what decides whether two URLs name the
/// same receiver, for the instance issuer and for the owner of an API key.
/// A URL with no such origin matches nothing.
pub fn origin_of(url: &str) -> Option<url::Origin> {
    let origin = url::Url::parse(url).ok()?.origin();
    origin.is_tuple().then_some(origin)
}

/// Why nothing is sent to a stored hub URL. A home enrolled before `connect`
/// held hub URLs to the rules below can hold either fault; the operator must
/// reconnect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubUrlFault {
    /// Not https, nor http to a loopback origin (the transport rule managed
    /// policy uses): the ingest key and signed requests would cross the
    /// network in the clear.
    Cleartext,
    /// Credentials, a query or a fragment. The transport never sends
    /// credentials, and every hub path is appended to the URL, so a query or
    /// fragment swallows it. The hosted service pins the URL as its token
    /// issuer and publishes that to unauthenticated callers.
    CarriesParts,
}

impl HubUrlFault {
    /// The fault in `hub`, if any.
    pub fn of(hub: &str) -> Option<Self> {
        if crate::managed::policy_url_accepted(hub).is_err() {
            return Some(Self::Cleartext);
        }
        let url = url::Url::parse(hub).ok()?;
        let carries_parts = !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some();
        carries_parts.then_some(Self::CarriesParts)
    }

    /// Completes "the hub URL at <origin> …".
    pub fn reason(self) -> &'static str {
        match self {
            Self::Cleartext => "is neither https nor http to a loopback origin",
            Self::CarriesParts => "carries credentials, a query or a fragment",
        }
    }

    /// The key standing a session record and `status` carry.
    fn standing(self) -> &'static str {
        match self {
            Self::Cleartext => "cleartext_hub",
            Self::CarriesParts => "unusable_hub_url",
        }
    }
}

/// ` at <origin>` for a hub URL, or nothing where it has no origin. Output
/// names a stored hub this way alone: 0.4.1's `connect` stored hub URLs with
/// credentials in them.
pub fn at_hub_origin(hub: &str) -> String {
    crate::relay_config::receiver_origin(hub)
        .map(|origin| format!(" at {origin}"))
        .unwrap_or_default()
}

/// A session record's `edge_identity` `hub` as any output shows it: the
/// origin alone, or null where it has none. A record can also hold the
/// whole stored URL, and 0.4.1's `connect` stored hub URLs with credentials
/// in them. Session logs are append-only evidence
/// that is not rewritten, so every reader reduces the field on read.
pub fn recorded_hub_origin(hub: &serde_json::Value) -> serde_json::Value {
    serde_json::json!(hub.as_str().and_then(crate::relay_config::receiver_origin))
}

/// Every hub request passes here before the stored hub URL is used
/// ([`HubUrlFault`]). The refusal names the hub by its origin alone; it
/// reaches directory status and so MCP `context_enrol`.
pub fn hub_url_accepted(hub: &str) -> Result<(), String> {
    let Some(fault) = HubUrlFault::of(hub) else {
        return Ok(());
    };
    let connect_with = match fault {
        HubUrlFault::Cleartext => "an https hub",
        HubUrlFault::CarriesParts => "the hub's address alone",
    };
    Err(format!(
        "the enrolled hub URL{at} {reason}, so nothing is sent to it: relay, standing checks, \
         directory proofs, managed policy, reporting approvals, instance requests, supplier \
         credentials and the hosted service are refused. Run `commonmeasure disconnect`, revoke \
         this edge's key and its ingest key on the hub's API keys page, then run \
         `commonmeasure connect` with {connect_with}",
        at = at_hub_origin(hub),
        reason = fault.reason(),
    ))
}

/// A JSON error by its category, line and column alone. serde's message can
/// quote a value from the file, such as a hand-edited hub URL with its
/// credentials, and the error reaches `status`, `doctor` and the console.
fn json_error_position(error: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let category = match error.classify() {
        Category::Io => "an I/O error",
        Category::Syntax => "a syntax error",
        Category::Data => "a data error",
        Category::Eof => "an end-of-file error",
    };
    format!(
        "{category} at line {}, column {}",
        error.line(),
        error.column()
    )
}

/// How long a write of the enrolment record waits for `enrolment.lock`
/// before it refuses. A holder keeps the lock for one read and one durable
/// rename, milliseconds each, never across a request to the hub; ten
/// seconds is far past any queue of such holders, and short enough that a
/// relay run behind a lock another process will not release refuses and
/// says so within a session-end hook's time.
pub const ENROLMENT_LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

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
                "{} is not a valid enrolment record: {}",
                source.display(),
                json_error_position(&error)
            )
        })?;
        Ok(Some(record))
    }

    /// Replace the record with this one, holding [`Self::lock`]. `connect`
    /// writes a new enrolment this way; a change to the stored record goes
    /// through [`Self::transition`] instead, so it applies to the record as
    /// it stands rather than to a copy read earlier.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let _lock = Self::lock(home)?;
        self.write(home)
    }

    /// Write atomically: bytes into a temporary file of this writer's own,
    /// then a rename. Every signature reads this file, so a reader must see
    /// the old record or the new one, never a half of either; a temporary
    /// name shared between writers let one rename away a file the other was
    /// still writing. Callers hold [`Self::lock`].
    fn write(&self, home: &Path) -> Result<(), String> {
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise enrolment record: {error}"))?;
        crate::declaration::replace(&Self::path(home), &encoded)
    }

    /// Hold `<home>/enrolment.lock` exclusively, across processes, until the
    /// returned guard is dropped, or refuse within
    /// [`ENROLMENT_LOCK_DEADLINE`]. Readers do not take it: the rename
    /// already gives them a whole record. Writers do, so that no process
    /// writes back a copy that lacks another's newer change, such as a
    /// revocation it would otherwise undo. The wait is bounded because a
    /// process that holds the lock and does not let go, another local user's
    /// included, would otherwise stop every writer behind it for good,
    /// silently; a refused write says so and names the file, and a
    /// revocation the hub stated or refused is asked again on the next
    /// relay run.
    fn lock(home: &Path) -> Result<crate::declaration::Lock, String> {
        let path = home.join("enrolment.lock");
        crate::declaration::lock_within(&path, ENROLMENT_LOCK_DEADLINE).map_err(|refused| {
            match refused {
                crate::declaration::LockRefused::Busy(_) => format!(
                    "another process has held {} for {}s; the enrolment record was not changed",
                    path.display(),
                    ENROLMENT_LOCK_DEADLINE.as_secs()
                ),
                other => other.to_string(),
            }
        })
    }

    /// Apply `change` to the record as stored now, under [`Self::lock`], and
    /// write it when `change` says it changed something. `self` becomes the
    /// stored record.
    ///
    /// When the stored record is gone (disconnected) or names another key
    /// (re-enrolled), what this process learnt is about an enrolment that no
    /// longer exists: `change` applies to `self` only and nothing is
    /// written, so a stale copy cannot bring back a record that was removed
    /// or replaced. `true` when the record was rewritten.
    pub fn transition(
        &mut self,
        home: &Path,
        change: impl FnOnce(&mut Self) -> bool,
    ) -> Result<bool, String> {
        let _lock = Self::lock(home)?;
        match Self::load(home)? {
            Some(mut stored) if stored.key_id == self.key_id => {
                let changed = change(&mut stored);
                if changed {
                    stored.write(home)?;
                }
                *self = stored;
                Ok(changed)
            }
            _ => {
                change(self);
                Ok(false)
            }
        }
    }

    /// Remove `<home>/enrolment.json` under [`Self::lock`], so that no
    /// transition in flight writes it back. `true` when there was one.
    pub fn remove(home: &Path) -> Result<bool, String> {
        let _lock = Self::lock(home)?;
        let path = Self::path(home);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
        }
    }

    /// The ingest key this edge may present to its enrolled hub: the key in
    /// `<home>/relay.json`, where the receiver there has the hub's origin.
    /// Enrolment writes both, so they agree unless the operator re-pointed
    /// `relay.json` at another receiver with that receiver's key, and that
    /// key is not the hub's to see (EGR-07). `None` when there is no file,
    /// no key, or the receiver is another origin; a file that cannot be read
    /// is an error.
    pub fn hub_ingest_key(&self, home: &Path) -> Result<Option<String>, String> {
        let source = home.join("relay.json");
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let relay: serde_json::Value = serde_json::from_slice(&encoded).map_err(|error| {
            format!("{} is not a valid relay config: {error}", source.display())
        })?;
        let hub = origin_of(&self.hub);
        let same_origin = hub.is_some() && relay["receiver"].as_str().and_then(origin_of) == hub;
        Ok(relay["api_key"]
            .as_str()
            .filter(|_| same_origin)
            .map(str::to_owned))
    }

    /// A refusal can establish revocation without supplying the hub's time.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some() || self.revocation_learnt_at.is_some()
    }

    /// Record an ingest-key refusal once, preserving any earlier revocation.
    /// The hub's revocation time stays unknown when its 401 supplies none.
    /// Callers record only a refusal of the enrolled ingest key
    /// ([`Self::hub_ingest_key`]); a 401 for any other key says nothing
    /// about this enrolment.
    pub fn record_refusal(&mut self, home: &Path, detail: &str) -> Result<(), String> {
        self.transition(home, |record| {
            if record.is_revoked() {
                return false;
            }
            record.revocation = Some(format!("hub: {detail}"));
            record.revocation_learnt_at = Some(timestamp(Utc::now()));
            true
        })?;
        Ok(())
    }

    /// Record a revocation the hub stated: its time and its side. The first
    /// time this edge learnt it was revoked stays, including one learnt from
    /// an earlier 401; the hub's time and side replace the 401's detail,
    /// because they are what makes the revocation permanent
    /// ([`Self::withdraw_refusal`]). A stated revocation already held stays
    /// as it is.
    pub fn record_revocation(
        &mut self,
        home: &Path,
        revoked_at: &str,
        revocation: &str,
    ) -> Result<(), String> {
        self.transition(home, |record| {
            if record.revoked_at.is_some() {
                return false;
            }
            record.revoked_at = Some(revoked_at.to_owned());
            record.revocation = Some(revocation.to_owned());
            if record.revocation_learnt_at.is_none() {
                record.revocation_learnt_at = Some(timestamp(Utc::now()));
            }
            true
        })?;
        Ok(())
    }

    /// Withdraw a revocation learnt only from a 401, once the hub has
    /// answered for this key with no revocation. The hub never answers `200`
    /// for a revoked key, so that 401 came from something other than a
    /// revocation. A revocation the hub stated (`revoked_at`) is permanent.
    ///
    /// `asked_at` is when the `200` was requested. Only an answer asked for
    /// after the refusal was learnt withdraws it: a relay run whose request
    /// went out before another process recorded a 401 carries an answer
    /// older than that 401, and must not resume signing on it. The learnt
    /// time is stored to the millisecond ([`timestamp`]), so the true time
    /// lies anywhere in that millisecond: the comparison is between whole
    /// milliseconds, and an answer asked for in the millisecond the refusal
    /// was learnt keeps it. A learnt time that does not parse is not
    /// withdrawn. The tests are made against the record as stored, so a
    /// process that read the record before another recorded the hub's
    /// revocation cannot clear it. `true` when the record was rewritten.
    ///
    /// Every relay run on an enrolled edge whose key stands comes here while
    /// it holds the spool, so the record is read first without
    /// [`Self::lock`], and the lock is taken only when there is a refusal to
    /// withdraw. The record is replaced whole by rename, so the unlocked
    /// read is a whole record. A refusal recorded after that read was learnt
    /// after `asked_at`, so this answer would not withdraw it either;
    /// under the lock the record is read again before anything is written.
    pub fn withdraw_refusal(
        &mut self,
        home: &Path,
        asked_at: DateTime<Utc>,
    ) -> Result<bool, String> {
        match Self::load(home) {
            Ok(Some(stored)) if stored.key_id == self.key_id => {
                if !stored.refusal_withdrawable(asked_at) {
                    *self = stored;
                    return Ok(false);
                }
            }
            // Gone or re-enrolled: `transition` would change `self` alone.
            Ok(_) if !self.refusal_withdrawable(asked_at) => return Ok(false),
            // An unreadable record is `transition`'s to report.
            _ => {}
        }
        self.transition(home, |record| {
            if !record.refusal_withdrawable(asked_at) {
                return false;
            }
            record.revocation = None;
            record.revocation_learnt_at = None;
            true
        })
    }

    /// Whether an answer asked for at `asked_at` withdraws this record's
    /// refusal ([`Self::withdraw_refusal`]).
    fn refusal_withdrawable(&self, asked_at: DateTime<Utc>) -> bool {
        self.revoked_at.is_none()
            && self
                .revocation_learnt_at
                .as_deref()
                .and_then(|learnt| DateTime::parse_from_rfc3339(learnt).ok())
                .is_some_and(|learnt| learnt.timestamp_millis() < asked_at.timestamp_millis())
    }

    /// Why this edge sends its enrolled hub nothing: the stored URL fails the
    /// transport rule ([`hub_url_accepted`]). `None` for an accepted hub.
    pub fn hub_refused(&self) -> Option<String> {
        hub_url_accepted(&self.hub).err()
    }

    /// `enrolled` while the key stands, `revoked` once a revocation was
    /// learnt, and, while the key stands but the stored hub URL is one
    /// nothing is sent to ([`Self::hub_refused`]), `cleartext_hub` or
    /// `unusable_hub_url` by its [`HubUrlFault`]. The word a session record
    /// carries.
    pub fn standing(&self) -> &'static str {
        if self.is_revoked() {
            "revoked"
        } else if let Some(fault) = HubUrlFault::of(&self.hub) {
            fault.standing()
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
            Err(_) => !self.is_revoked(),
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
    /// run and `connect` use the hub's answer and the last uploaded release
    /// ([`ProofStatement::need`]).
    pub fn directory_proof_due(
        &self,
        listing: Option<&DirectoryListing>,
        now: DateTime<Utc>,
    ) -> bool {
        if self.is_revoked() {
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
        match stated.need(&authority, now, listing.last_uploaded_release.as_deref()) {
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
        if self.is_revoked() {
            return Listing::Unlisted(format!(
                "the key is revoked ({}): the hub refused the enrolled ingest key, so this edge \
                 stopped signing and renews no directory proof; whether the directory still \
                 lists the key is not known here",
                self.revocation.as_deref().unwrap_or("hub")
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
        if let Some(concluded) = &stated.concluded {
            return Listing::Unlisted(format!(
                "this edge concluded the key is unlisted when the hub refused its directory \
                 proof ({}): {}",
                concluded.status, concluded.detail
            ));
        }
        if stated.listed == Some(false) {
            return Listing::Unlisted(match &stated.unlisted_reason {
                Some(reason) => format!("hub: {reason}"),
                None => "the hub states that this key is unlisted; no reason supplied".to_owned(),
            });
        }
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
            listed: None,
            unlisted_reason: None,
            concluded: None,
        }
    }

    #[test]
    fn proof_statements_preserve_listing_decisions_and_ignore_unknown_members() {
        let stated: ProofStatement = serde_json::from_value(serde_json::json!({
            "authority": "hub.example",
            "lifetime_secs": 604_800,
            "expires_at": "2026-09-21T11:00:00Z",
            "listed": false,
            "unlisted_reason": "the hub withheld this key",
            "future_member": {"anything": true}
        }))
        .unwrap();
        assert_eq!(stated.listed, Some(false));
        assert_eq!(
            stated.unlisted_reason.as_deref(),
            Some("the hub withheld this key")
        );
        let held = DirectoryListing {
            key_id: "key-1".to_owned(),
            last_uploaded_release: Some(RELEASE.to_owned()),
            checked_at: "2026-09-14T12:00:00Z".to_owned(),
            stated: Some(stated.clone()),
            failure: None,
        };
        assert_eq!(
            record().listing(Some(&held), at("2026-09-14T12:00:00Z")),
            Listing::Unlisted("hub: the hub withheld this key".to_owned())
        );
        let no_reason = DirectoryListing {
            stated: Some(ProofStatement {
                unlisted_reason: None,
                ..stated
            }),
            ..held
        };
        assert!(matches!(
            record().listing(Some(&no_reason), at("2026-09-14T12:00:00Z")),
            Listing::Unlisted(_)
        ));
    }

    fn concluded(status: u16) -> DirectoryListing {
        DirectoryListing {
            key_id: "key-1".to_owned(),
            last_uploaded_release: Some("0.0.0".to_owned()),
            checked_at: "2026-09-14T12:00:00.000Z".to_owned(),
            stated: Some(ProofStatement {
                listed: Some(false),
                unlisted_reason: Some("the edge key is revoked".to_owned()),
                concluded: Some(RefusalConclusion {
                    status,
                    detail: "the edge key is revoked".to_owned(),
                }),
                ..statement(Some("2026-09-21T11:00:00Z"))
            }),
            failure: Some("the hub refused the proof (409): the edge key is revoked".to_owned()),
        }
    }

    /// The conclusion is written inside `stated` as `edge_conclusion`, read
    /// back as it was written, and shown as the edge's, never as `hub: …`.
    #[test]
    fn an_edge_conclusion_is_stored_and_shown_as_the_edges() {
        let home = tempfile::tempdir().unwrap();
        let held = concluded(409);
        held.store(home.path()).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(DirectoryListing::path(home.path())).unwrap())
                .unwrap();
        assert_eq!(
            written["stated"]["edge_conclusion"],
            serde_json::json!({"status": 409, "detail": "the edge key is revoked"})
        );
        assert_eq!(written["stated"]["listed"], false);
        assert_eq!(
            DirectoryListing::load(home.path(), "key-1").unwrap(),
            Some(held.clone())
        );
        let Listing::Unlisted(reason) = record().listing(Some(&held), at("2026-09-14T12:00:00Z"))
        else {
            panic!("a concluded key is unlisted");
        };
        assert!(!reason.starts_with("hub:"), "{reason}");
        assert_eq!(
            reason,
            "this edge concluded the key is unlisted when the hub refused its directory proof \
             (409): the edge key is revoked"
        );
    }

    /// A later release may store more in `edge_conclusion`; this one still
    /// loads the file and the conclusion it knows.
    #[test]
    fn an_edge_conclusion_with_an_unknown_member_still_loads() {
        let home = tempfile::tempdir().unwrap();
        let held = concluded(409);
        held.store(home.path()).unwrap();
        let path = DirectoryListing::path(home.path());
        let mut written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        written["stated"]["edge_conclusion"]["code"] = serde_json::json!("key_revoked");
        std::fs::write(&path, written.to_string()).unwrap();
        assert_eq!(
            DirectoryListing::load(home.path(), "key-1").unwrap(),
            Some(held)
        );
    }

    /// The hub's statement is read without the conclusion, so a hub that
    /// sends `edge_conclusion` (or `concluded`) states only what it states.
    #[test]
    fn a_statement_from_the_hub_cannot_claim_an_edge_conclusion() {
        let stated: ProofStatement = serde_json::from_value(serde_json::json!({
            "authority": "hub.example",
            "lifetime_secs": 604_800,
            "expires_at": "2026-09-21T11:00:00Z",
            "listed": false,
            "unlisted_reason": "the hub withheld this key",
            "edge_conclusion": {"status": 409, "detail": "claimed"},
            "concluded": {"status": 409, "detail": "claimed"}
        }))
        .unwrap();
        assert_eq!(stated.concluded, None);
        let held = DirectoryListing {
            stated: Some(stated),
            ..concluded(409)
        };
        assert_eq!(
            record().listing(Some(&held), at("2026-09-14T12:00:00Z")),
            Listing::Unlisted("hub: the hub withheld this key".to_owned())
        );
    }

    /// A file 0.4.2 wrote after a 401, 404 or 409 holds the edge's
    /// conclusion as though the hub had stated it, with nothing to tell the
    /// two apart. It reads as 0.4.2 read it, until the next statement from
    /// the hub replaces it.
    #[test]
    fn a_listing_written_by_0_4_2_reads_as_it_did() {
        let home = tempfile::tempdir().unwrap();
        let written = serde_json::json!({
            "key_id": "key-1",
            "checked_at": "2026-09-14T12:00:00.000Z",
            "last_uploaded_release": "0.4.1",
            "stated": {
                "authority": "hub.example",
                "lifetime_secs": 604_800,
                "expires_at": "2026-09-21T11:00:00Z",
                "listed": false,
                "unlisted_reason": "the edge key is revoked; a revoked key is never listed"
            },
            "failure": "the hub refused the proof (409): the edge key is revoked; a revoked key is never listed"
        });
        std::fs::write(DirectoryListing::path(home.path()), written.to_string()).unwrap();
        let held = DirectoryListing::load(home.path(), "key-1")
            .unwrap()
            .unwrap();
        assert_eq!(held.stated.as_ref().unwrap().concluded, None);
        assert_eq!(
            record().listing(Some(&held), at("2026-09-14T12:00:00Z")),
            Listing::Unlisted(
                "hub: the edge key is revoked; a revoked key is never listed".to_owned()
            )
        );
    }

    /// Downgrade: 0.4.2 reads this file with the structs below, copied from
    /// `crates/commonmeasure-harness/src/enrolment.rs` at 4c89c34 (lines
    /// 97-139). Its top level denies unknown members; its statement ignores
    /// them. It parses a concluded file without error and finds
    /// `listed: false`, which its `listing` (line 695) turns into `Unlisted`
    /// before the only `ListedUntil` it makes (line 731).
    #[test]
    fn a_0_4_2_reader_reads_a_concluded_listing_as_unlisted() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct DirectoryListing042 {
            key_id: String,
            checked_at: String,
            #[serde(default)]
            last_uploaded_release: Option<String>,
            #[serde(default)]
            stated: Option<ProofStatement042>,
            #[serde(default)]
            failure: Option<String>,
        }
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct ProofStatement042 {
            authority: String,
            lifetime_secs: i64,
            #[serde(default)]
            expires_at: Option<String>,
            #[serde(default)]
            listed: Option<bool>,
            #[serde(default)]
            unlisted_reason: Option<String>,
        }
        for status in [401, 404, 409] {
            let home = tempfile::tempdir().unwrap();
            concluded(status).store(home.path()).unwrap();
            let encoded = std::fs::read(DirectoryListing::path(home.path())).unwrap();
            let read: DirectoryListing042 =
                serde_json::from_slice(&encoded).expect("0.4.2 parses a file this release writes");
            let stated = read.stated.expect("the statement is kept");
            assert_eq!(stated.listed, Some(false));
            assert_eq!(
                stated.unlisted_reason.as_deref(),
                Some("the edge key is revoked")
            );
        }
    }

    /// A held proof signed within the day is current; one signed more than
    /// a day ago, one too close to expiry to be served, and none at all are
    /// due.
    #[test]
    fn a_proof_is_due_when_absent_more_than_a_day_old_or_about_to_leave() {
        let now = at("2026-09-14T12:00:00Z");
        assert_eq!(
            statement(Some("2026-09-21T11:00:00Z")).need("hub.example", now, Some(RELEASE)),
            ProofNeed::Current,
            "signed an hour ago"
        );
        assert!(matches!(
            statement(Some("2026-09-20T11:59:00Z")).need("hub.example", now, Some(RELEASE)),
            ProofNeed::Due(_)
        ));
        assert!(matches!(
            statement(Some("2026-09-14T13:59:00Z")).need("hub.example", now, Some(RELEASE)),
            ProofNeed::Due(_)
        ));
        assert!(matches!(
            statement(None).need("hub.example", now, Some(RELEASE)),
            ProofNeed::Due(_)
        ));
    }

    /// The edge signs only for the authority it enrolled under, and not at
    /// all when the hub's lifetime leaves nothing it would serve.
    #[test]
    fn a_proof_for_another_authority_or_an_unservable_lifetime_is_not_made() {
        let now = at("2026-09-14T12:00:00Z");
        let ProofNeed::Unsignable(reason) =
            statement(None).need("other.example", now, Some(RELEASE))
        else {
            panic!("another authority is not signed for");
        };
        assert!(reason.contains("enrolled under other.example"), "{reason}");
        let short = ProofStatement {
            lifetime_secs: 7_200,
            ..statement(None)
        };
        assert!(matches!(
            short.need("hub.example", now, Some(RELEASE)),
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
            last_uploaded_release: Some(RELEASE.to_owned()),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(Some("2026-09-21T11:00:00Z"))),
            failure: None,
        };
        assert!(!record().directory_proof_due(Some(&current), now));

        let failed = DirectoryListing {
            key_id: "key-1".to_owned(),
            last_uploaded_release: Some(RELEASE.to_owned()),
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
            last_uploaded_release: Some(RELEASE.to_owned()),
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
            last_uploaded_release: Some(RELEASE.to_owned()),
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
            last_uploaded_release: Some(RELEASE.to_owned()),
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: Some(statement(None)),
            failure: Some("the hub refused the proof (422): created is in the future".to_owned()),
        };
        let Listing::Unlisted(reason) = record().listing(Some(&refused), now) else {
            panic!("no proof held");
        };
        assert!(reason.contains("created is in the future"), "{reason}");
    }

    /// Every signature reads the record, so a writer must never publish a
    /// partial one. Writers of different lengths, whole-record stores and
    /// transitions together, with a reader loading throughout.
    #[test]
    fn concurrent_writers_never_publish_a_malformed_record() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let home = tempfile::tempdir().unwrap();
        record().store(home.path()).unwrap();
        let done = AtomicBool::new(false);
        let reads = AtomicUsize::new(0);
        let bad_reads = std::sync::Mutex::new(Vec::new());
        let bad_writes = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    match EnrolmentRecord::load(home.path()) {
                        Ok(Some(_)) => {}
                        other => bad_reads.lock().unwrap().push(format!("{other:?}")),
                    }
                    reads.fetch_add(1, Ordering::Relaxed);
                }
            });
            let writers: Vec<_> = (0..3)
                .map(|n| {
                    let mut value = record();
                    value.name = "x".repeat(500 + n * 100);
                    let (home, bad_writes) = (home.path(), &bad_writes);
                    scope.spawn(move || {
                        for _ in 0..40 {
                            if let Err(error) = value.store(home) {
                                bad_writes.lock().unwrap().push(error);
                            }
                        }
                    })
                })
                .collect();
            let (home_path, bad_writes_ref) = (home.path(), &bad_writes);
            let toggler = scope.spawn(move || {
                let mut value = record();
                for _ in 0..20 {
                    for step in [
                        value.record_refusal(home_path, "refused"),
                        value.withdraw_refusal(home_path, Utc::now()).map(|_| ()),
                    ] {
                        if let Err(error) = step {
                            bad_writes_ref.lock().unwrap().push(error);
                        }
                    }
                }
            });
            for writer in writers {
                writer.join().unwrap();
            }
            toggler.join().unwrap();
            done.store(true, Ordering::Relaxed);
        });
        assert!(reads.load(Ordering::Relaxed) > 0);
        assert_eq!(bad_reads.into_inner().unwrap(), Vec::<String>::new());
        assert_eq!(bad_writes.into_inner().unwrap(), Vec::<String>::new());
        let leftovers: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// The listing has no lock: its writers each replace the whole file, and
    /// only their own temporary file keeps a reader from seeing half of one.
    #[test]
    fn concurrent_listing_writers_never_publish_a_malformed_listing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let home = tempfile::tempdir().unwrap();
        let listing = |failure: usize| DirectoryListing {
            key_id: "key-1".to_owned(),
            last_uploaded_release: None,
            checked_at: "2026-09-14T11:00:00.000Z".to_owned(),
            stated: None,
            failure: Some("x".repeat(500 + failure * 100)),
        };
        listing(0).store(home.path()).unwrap();
        let done = AtomicBool::new(false);
        let bad = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    match DirectoryListing::load(home.path(), "key-1") {
                        Ok(Some(_)) => {}
                        other => bad.lock().unwrap().push(format!("{other:?}")),
                    }
                }
            });
            let writers: Vec<_> = (0..4)
                .map(|n| {
                    let (value, home, bad) = (listing(n), home.path(), &bad);
                    scope.spawn(move || {
                        for _ in 0..40 {
                            if let Err(error) = value.store(home) {
                                bad.lock().unwrap().push(error);
                            }
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }
            done.store(true, Ordering::Relaxed);
        });
        assert_eq!(bad.into_inner().unwrap(), Vec::<String>::new());
    }

    /// A process holding a copy read before another process recorded the
    /// hub's revocation can neither clear it nor turn it back into a 401-only
    /// revocation that a later clean answer would withdraw.
    #[test]
    fn a_stale_copy_cannot_clear_or_demote_a_stated_revocation() {
        let home = tempfile::tempdir().unwrap();
        record().store(home.path()).unwrap();
        let mut unrevoked = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        let mut learnt = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        learnt.record_refusal(home.path(), "refused").unwrap();
        let mut refused_only = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        assert!(refused_only.revoked_at.is_none());

        let mut stating = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        stating
            .record_revocation(home.path(), "2026-09-25T09:00:00Z", "owner")
            .unwrap();
        let stated = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        assert_eq!(stated.revoked_at.as_deref(), Some("2026-09-25T09:00:00Z"));
        assert_eq!(stated.revocation.as_deref(), Some("owner"));
        assert_eq!(stated.revocation_learnt_at, learnt.revocation_learnt_at);

        assert!(
            !refused_only
                .withdraw_refusal(home.path(), Utc::now() + chrono::Duration::seconds(1))
                .unwrap()
        );
        assert_eq!(refused_only, stated);
        unrevoked.record_refusal(home.path(), "later").unwrap();
        assert_eq!(unrevoked, stated);
        assert_eq!(EnrolmentRecord::load(home.path()).unwrap().unwrap(), stated);
    }

    /// A clean answer withdraws a 401-only revocation only when it was asked
    /// for after the 401 was learnt; an older answer, from a run that sent
    /// its request before another process recorded the 401, does not. The
    /// learnt time is stored to the millisecond, so an answer asked for in
    /// that millisecond may be the older one and keeps the refusal.
    #[test]
    fn only_an_answer_asked_for_after_the_refusal_withdraws_it() {
        let home = tempfile::tempdir().unwrap();
        let mut refused = record();
        refused.revocation = Some("hub: refused".to_owned());
        refused.revocation_learnt_at = Some(timestamp(at("2026-09-25T08:00:00.000900Z")));
        refused.store(home.path()).unwrap();
        for asked_at in [
            "2026-09-25T07:59:59Z",
            "2026-09-25T08:00:00Z",
            "2026-09-25T08:00:00.000100Z",
            "2026-09-25T08:00:00.000999999Z",
        ] {
            assert!(
                !refused
                    .clone()
                    .withdraw_refusal(home.path(), at(asked_at))
                    .unwrap(),
                "{asked_at}"
            );
        }
        assert_eq!(
            EnrolmentRecord::load(home.path()).unwrap(),
            Some(refused.clone())
        );
        assert!(
            refused
                .withdraw_refusal(home.path(), at("2026-09-25T08:00:00.001Z"))
                .unwrap()
        );
        assert!(
            !EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .is_revoked()
        );
    }

    /// A 401 learnt after a clean answer was asked for, in the same
    /// millisecond, keeps refusing: the record keeps it and a signer loaded
    /// before the refusal was recorded does not sign again.
    #[test]
    fn an_answer_asked_for_in_the_millisecond_of_the_refusal_leaves_the_signer_refusing() {
        use crate::identity::{EdgeKey, Identity};
        let home = tempfile::tempdir().unwrap();
        let key = EdgeKey::generate().unwrap();
        key.store(home.path()).unwrap();
        let mut value = record();
        value.key_id = key.thumbprint();
        value.store(home.path()).unwrap();
        let identity = Identity::load(home.path()).unwrap();
        let signer = identity.signer().expect("an enrolled key signs");
        assert!(signer.directory_proof("hub.example", 1000, 604800).is_ok());

        let asked_at = at("2026-09-25T08:00:00.000100Z");
        value.revocation = Some("hub: refused".to_owned());
        value.revocation_learnt_at = Some(timestamp(at("2026-09-25T08:00:00.000900Z")));
        value.store(home.path()).unwrap();
        assert!(signer.directory_proof("hub.example", 1000, 604800).is_err());

        assert!(!value.withdraw_refusal(home.path(), asked_at).unwrap());
        assert!(
            EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .is_revoked()
        );
        assert!(signer.directory_proof("hub.example", 1000, 604800).is_err());
    }

    /// A copy of an enrolment that was since removed or replaced by another
    /// key writes nothing back.
    #[test]
    fn a_stale_copy_cannot_bring_back_a_removed_or_replaced_record() {
        let home = tempfile::tempdir().unwrap();
        record().store(home.path()).unwrap();
        let mut stale = EnrolmentRecord::load(home.path()).unwrap().unwrap();
        assert!(EnrolmentRecord::remove(home.path()).unwrap());
        stale.record_refusal(home.path(), "refused").unwrap();
        assert!(stale.is_revoked());
        assert_eq!(EnrolmentRecord::load(home.path()).unwrap(), None);

        let mut stale = record();
        let mut replacement = record();
        replacement.key_id = "key-2".to_owned();
        replacement.store(home.path()).unwrap();
        stale.record_refusal(home.path(), "refused").unwrap();
        stale
            .record_revocation(home.path(), "2026-09-25T09:00:00Z", "owner")
            .unwrap();
        assert_eq!(
            EnrolmentRecord::load(home.path()).unwrap(),
            Some(replacement)
        );
    }

    /// A transition waits for the lock and then applies to the record as it
    /// stands, not to the one it read before waiting. Held here while the
    /// hub's revocation is written, a withdrawal of the 401 then finds the
    /// stated revocation and leaves it.
    #[test]
    fn a_transition_waits_for_the_lock_and_applies_to_the_record_as_it_then_stands() {
        let home = tempfile::tempdir().unwrap();
        let mut refused = record();
        refused.revocation = Some("hub: refused".to_owned());
        refused.revocation_learnt_at = Some("2026-09-25T08:00:00.000Z".to_owned());
        refused.store(home.path()).unwrap();
        let before = std::fs::read(EnrolmentRecord::path(home.path())).unwrap();

        let lock = EnrolmentRecord::lock(home.path()).unwrap();
        let path = home.path();
        let (withdrawn, removed) = std::thread::scope(|scope| {
            let mut copy = refused.clone();
            let asked_at = at("2026-09-25T08:30:00Z");
            let withdrawing = scope.spawn(move || copy.withdraw_refusal(path, asked_at));
            let removing = scope.spawn(move || EnrolmentRecord::remove(path));
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert_eq!(
                std::fs::read(EnrolmentRecord::path(home.path())).unwrap(),
                before,
                "a transition or a removal wrote while another process held the lock"
            );
            let mut stated = refused.clone();
            stated.revoked_at = Some("2026-09-25T09:00:00Z".to_owned());
            stated.revocation = Some("owner".to_owned());
            stated.write(home.path()).unwrap();
            drop(lock);
            (withdrawing.join().unwrap(), removing.join().unwrap())
        });
        assert!(removed.unwrap());
        // Whichever ran first, the stated revocation was never cleared: the
        // withdrawal either found it or found no record at all.
        assert!(!withdrawn.unwrap());
        assert_eq!(EnrolmentRecord::load(home.path()).unwrap(), None);
    }

    /// 0.4.1's `connect` stored a cleartext remote hub URL with credentials
    /// in it. The refusal names the hub by origin, or by nothing where the
    /// stored value has none.
    #[test]
    fn a_refused_hub_url_is_named_by_its_origin_alone() {
        let mut stored = record();
        stored.hub = "http://user:ak_PLANTED@hub.remote.example".to_owned();
        let refusal = stored
            .hub_refused()
            .expect("a cleartext remote hub is refused");
        assert!(
            refusal.starts_with(
                "the enrolled hub URL at http://hub.remote.example is neither https nor http"
            ),
            "{refusal}"
        );
        assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
        let refusal = hub_url_accepted("hub.remote.example/ak_PLANTED").unwrap_err();
        assert!(
            refusal.starts_with("the enrolled hub URL is neither https nor http"),
            "{refusal}"
        );
        assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
    }

    /// 0.4.1's `connect` also stored an https or loopback hub URL with
    /// credentials, a query or a fragment. Nothing is sent to it, by the same
    /// check every hub request makes, and the refusal names it by origin.
    #[test]
    fn a_hub_url_carrying_credentials_a_query_or_a_fragment_is_refused() {
        for (hub, origin) in [
            ("https://ops:ak_PLANTED@hub.example", "https://hub.example"),
            ("https://ak_PLANTED@hub.example/base", "https://hub.example"),
            ("https://:ak_PLANTED@hub.example", "https://hub.example"),
            (
                "http://ops:ak_PLANTED@127.0.0.1:8443",
                "http://127.0.0.1:8443",
            ),
            (
                "https://hub.example?api_key=ak_PLANTED",
                "https://hub.example",
            ),
            (
                "https://hub.example/?api_key=ak_PLANTED",
                "https://hub.example",
            ),
            ("https://hub.example#ak_PLANTED", "https://hub.example"),
        ] {
            let mut stored = record();
            stored.hub = hub.to_owned();
            let refusal = hub_url_accepted(hub).unwrap_err();
            assert_eq!(stored.hub_refused().as_deref(), Some(refusal.as_str()));
            assert!(
                refusal.starts_with(&format!(
                    "the enrolled hub URL at {origin} carries credentials, a query or a \
                     fragment, so nothing is sent to it"
                )),
                "{hub}: {refusal}"
            );
            assert!(
                refusal.ends_with("`commonmeasure connect` with the hub's address alone"),
                "{refusal}"
            );
            assert!(!refusal.contains("ak_PLANTED"), "{refusal}");
            assert_eq!(stored.standing(), "unusable_hub_url", "{hub}");
        }
        for hub in [
            "https://hub.example",
            "https://hub.example/",
            "https://hub.example/base",
            "http://127.0.0.1:8443",
            "http://[::1]:8443",
        ] {
            assert_eq!(hub_url_accepted(hub), Ok(()), "{hub}");
        }
    }

    /// `enrolment.lock` is created readable by its owner only, so another
    /// local user who can reach the home cannot open it to hold it. One
    /// that exists already keeps its mode.
    #[cfg(unix)]
    #[test]
    fn a_created_enrolment_lock_is_owner_only_and_an_existing_one_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("enrolment.lock");
        record().store(home.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        record().store(home.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    /// A holder that does not let go of `enrolment.lock` makes a write
    /// refuse within [`ENROLMENT_LOCK_DEADLINE`], naming the file, instead
    /// of waiting for as long as it is held. Nothing is written.
    #[test]
    fn a_held_enrolment_lock_refuses_within_the_deadline_and_names_the_file() {
        let home = tempfile::tempdir().unwrap();
        record().store(home.path()).unwrap();
        let before = std::fs::read(EnrolmentRecord::path(home.path())).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .open(home.path().join("enrolment.lock"))
            .unwrap();
        held.lock().unwrap();

        let (sent, received) = std::sync::mpsc::channel();
        let path = home.path().to_owned();
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            let mut changed = record();
            changed.name = "desktop".to_owned();
            sent.send(changed.store(&path)).ok();
        });
        let refusal = received
            .recv_timeout(ENROLMENT_LOCK_DEADLINE * 2)
            .expect("the write waited past its deadline")
            .expect_err("a held lock refuses the write");
        assert!(
            started.elapsed() >= ENROLMENT_LOCK_DEADLINE,
            "refused before the deadline: {refusal}"
        );
        assert!(refusal.contains("enrolment.lock"), "{refusal}");
        assert!(refusal.contains("another process"), "{refusal}");
        assert!(!refusal.contains("writable"), "{refusal}");
        assert_eq!(
            std::fs::read(EnrolmentRecord::path(home.path())).unwrap(),
            before
        );
        drop(held);
    }

    /// EGR-119. An `enrolment.lock` that exists and cannot be opened is that
    /// file's fault: the refusal names it and says what to do with it, apart
    /// from the busy refusal above.
    #[cfg(unix)]
    #[test]
    fn an_enrolment_lock_that_cannot_be_opened_names_its_own_remedy() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let home = tempfile::tempdir().unwrap();
        if std::fs::metadata(home.path()).unwrap().uid() == 0 {
            return;
        }
        let path = home.path().join("enrolment.lock");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refusal = record().store(home.path()).unwrap_err();
        assert!(refusal.contains(&path.display().to_string()), "{refusal}");
        assert!(
            refusal.contains("make that lock file readable and writable by this user"),
            "{refusal}"
        );
        assert!(!EnrolmentRecord::path(home.path()).exists());
    }
}
