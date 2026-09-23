//! The supplier credential release client: the edge half of
//! `docs/contracts/supplier-credentials.md` §Release and §Edge side.
//!
//! The hosted service asks its hub which supplier credentials this enrolled
//! edge is authorised to hold, `GET <hub>/api/v1/supplier-credentials`, signed
//! with the enrolled key as the desired-policy request is
//! (`commonmeasure_harness::identity::SigningIdentity::sign_request_covering`):
//! RFC 9421 under the Web Bot Auth profile, with a fresh nonce each time
//! because the hub refuses a repeat. Unlike the policy request it also covers
//! `@method` and `@path`, so a signature made for another route, or one made
//! as the policy request is, is refused here (§Signature components of the
//! release request). The request carries no API key.
//!
//! Released values go into a
//! [`commonmeasure_supply::credentials::ReleasedStore`] and nowhere else:
//! not the environment, a file, a record, a log line or an error. A reason
//! this module records for an answer is built from a status code, the hub's
//! stable reason on a status other than `200`, or fixed text. A `200` body is
//! never quoted, and neither a parser's nor the transport's message is passed
//! on, because either may quote the bytes it failed on: the HTTP client names
//! a chunk-size line it cannot read, and on a misframed `200` that line is
//! the release document.
//!
//! A fetch has one of the contract's four outcomes. `accepted` and
//! `unchanged` replace the held set with what the hub served. `refused` (a
//! `401` that rules on the key, or an edge that holds no key to sign with)
//! clears it at once, and so does a `200` that serves nothing. `unreachable`
//! keeps the last released set: the hub was not reached, or it answered
//! something that rules on nothing (another status, a `200` that is not a
//! release document or is not addressed to this edge). An edge that has
//! never completed a fetch holds nothing, so every provider without a local
//! credential stays unavailable, naming its variable (`docs/FAIL-POLICY.md`
//! §5).
//!
//! A `401` whose `code` is one of the hub's seven fault codes (`FAULTS`: a
//! skewed clock, a repeated nonce, a signature the hub could not verify) is a
//! fault in that one request, so the request is signed again with a fresh
//! nonce and sent once more inside the same budget, provided at least a
//! quarter of the budget is left (`worth_retrying`). A second fault, or a
//! fault with too little budget left to retry, is `unreachable`, recorded
//! with its code. Every other `401` rules on the key and clears the set:
//! `key_unknown`, `key_revoked`, no `code` at all (a hub before the stable
//! codes), and any code this edge does not know, since a wrongly cleared set
//! comes back at the next release and a wrongly kept one delays revocation
//! (`docs/contracts/supplier-credentials.md` §Signature components of the
//! release request; EGR-13).
//!
//! A kept set is not kept for ever. The store stops supplying a set once
//! [`MAX_AGE_INTERVALS`] fetch intervals have passed since the last
//! `accepted` or `unchanged` fetch, and checks that where a value is read
//! ([`commonmeasure_supply::credentials::ReleasedStore`]), so it holds
//! whether the hub is unreachable, [`fetch`] refuses to try or nothing calls
//! [`fetch`] any more. A fetch that finds the set expired records it under
//! `expired`. The multiple is steering's fail-closed reading of the contract
//! (19 September 2026) and the owner's to revise.
//!
//! What each fetch did is kept in `<home>/supplier-credentials.json`, owner
//! readable only: the outcome, the status, the hub's stable reason on a
//! refusal, and the held credentials by connection, provider and variable.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use commonmeasure_harness::EnrolmentRecord;
use commonmeasure_harness::identity::{Derived, Identity};
use commonmeasure_harness::instance::write_owner_only;
use commonmeasure_harness::managed::{Deployment, policy_url_accepted};
use commonmeasure_supply::credentials::{Released, ReleasedName, ReleasedStore};

/// The release route, on the origin of the deployment's `policy_url`.
pub const RELEASE_PATH: &str = "/api/v1/supplier-credentials";

/// Where the last fetch is recorded, under the operator home.
pub const STATE_FILE: &str = "supplier-credentials.json";

/// The time one fetch may take once the hub's name has resolved: the request
/// budget in the contract's revocation bound (`interval_seconds` plus this).
/// Name resolution is outside it, as it is outside
/// [`commonmeasure_http::CLIENT_TIMEOUT`].
pub const BUDGET: Duration = commonmeasure_http::CLIENT_TIMEOUT;

/// How many fetch intervals a released set is used for after the last fetch
/// the hub answered with a release: 900 s at the default interval.
pub const MAX_AGE_INTERVALS: u32 = 3;

/// The store a service fetching every `interval` holds its releases in.
pub fn store_for(interval: Duration) -> ReleasedStore {
    ReleasedStore::new(interval.saturating_mul(MAX_AGE_INTERVALS))
}

const USER_AGENT: &str = concat!("commonmeasure/", env!("CARGO_PKG_VERSION"));

/// The longest hub reason kept. A stable reason is a short token or sentence;
/// anything longer is cut, so a misbehaving origin cannot fill the record.
const REASON_LIMIT: usize = 200;

/// The reason recorded for every transport failure. The transport's own
/// message is dropped whole, not bounded or filtered: it can quote response
/// bytes, and the response to this request carries credential values.
const NO_USABLE_ANSWER: &str = "the hub gave no usable answer: it was not reached, did not \
     answer within the request budget, or its answer was not a well-formed HTTP response";

/// The `401` codes that fault one request rather than rule on the key,
/// compared exactly. The hub's table of codes is closed; a code outside this
/// list, including a new one, is a ruling.
const FAULTS: [&str; 7] = [
    "signature_invalid",
    "signature_expired",
    "window_too_long",
    "authority_mismatch",
    "components_missing",
    "nonce_missing",
    "nonce_repeated",
];

/// The share of the budget that must be left after a fault for the retry to
/// be sent: a quarter. Below it a retry could not usefully complete, and the
/// fault stands as the answer.
const RETRY_FLOOR_DIVISOR: u32 = 4;

/// Whether a fault answered after `elapsed` leaves enough of `budget` to
/// sign again and retry: at least a quarter of it, and never nothing.
fn worth_retrying(budget: Duration, elapsed: Duration) -> bool {
    let remaining = budget.saturating_sub(elapsed);
    !remaining.is_zero() && remaining >= budget / RETRY_FLOOR_DIVISOR
}

/// The contract's four fetch outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The hub served a set that differs from the one held.
    Accepted,
    /// The hub served the set already held.
    Unchanged,
    /// No ruling was received; the last released set is kept.
    Unreachable,
    /// The hub does not accept this edge's key; the held set is cleared.
    Refused,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Unchanged => "unchanged",
            Self::Unreachable => "unreachable",
            Self::Refused => "refused",
        }
    }
}

/// One held credential as the record names it. Never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub connection_id: String,
    pub provider: String,
    pub variable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<String>,
    pub fetched_at: String,
}

/// One served entry this edge did not hold, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ignored {
    pub connection_id: String,
    pub provider: String,
    pub reason: String,
}

/// What one fetch did, as `<home>/supplier-credentials.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetch {
    pub version: u32,
    pub at: String,
    /// The URL asked.
    pub url: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// The hub's stable reason on a refusal, or what stopped the fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The code of a `401` fault the request was signed again after. The
    /// outcome, status and reason are the second answer's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retried_after: Option<String>,
    /// The hub's `served_at` on a `200`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_at: Option<String>,
    /// The credentials held after this fetch.
    pub held: Vec<Held>,
    /// Served entries not held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<Ignored>,
    /// Released credentials no longer used after this fetch because no fetch
    /// has confirmed them for `max_age_seconds`. Their `fetched_at` is the
    /// last fetch that did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expired: Vec<Held>,
    /// The store's maximum age, stated wherever `expired` names anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u64>,
}

impl Fetch {
    pub fn path(home: &Path) -> PathBuf {
        home.join(STATE_FILE)
    }

    /// The last recorded fetch, or `None` where none was recorded.
    pub fn read(home: &Path) -> Result<Option<Self>, String> {
        let path = Self::path(home);
        match std::fs::read(&path) {
            Ok(encoded) => serde_json::from_slice(&encoded)
                .map(Some)
                .map_err(|error| format!("{} is not a fetch record: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    /// One line for the service's journal. Names only.
    pub fn journal_line(&self) -> String {
        let held = if self.held.is_empty() {
            "none held".to_owned()
        } else {
            format!(
                "holding {}",
                self.held
                    .iter()
                    .map(|held| format!("{} ({})", held.provider, held.connection_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let status = self
            .http_status
            .map(|status| format!(" {status}"))
            .unwrap_or_default();
        let reason = self
            .reason
            .as_deref()
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        let retried = self
            .retried_after
            .as_deref()
            .map(|code| format!(" (signed again after 401 {code})"))
            .unwrap_or_default();
        let ignored = if self.ignored.is_empty() {
            String::new()
        } else {
            format!("; {} served entries not held", self.ignored.len())
        };
        let expired = match (self.expired.as_slice(), self.max_age_seconds) {
            ([], _) => String::new(),
            (expired, max_age) => format!(
                "; no longer using {}, not confirmed by the hub for {}",
                expired
                    .iter()
                    .map(|held| format!("{} ({})", held.provider, held.connection_id))
                    .collect::<Vec<_>>()
                    .join(", "),
                max_age.map_or_else(|| "its maximum age".to_owned(), |age| format!("{age}s"))
            ),
        };
        format!(
            "supplier credentials {}{status}{reason}{retried}; {held}{ignored}{expired}",
            self.outcome.as_str()
        )
    }
}

/// The hub's `200` body. No `Debug`: it holds values.
#[derive(Deserialize)]
struct ReleaseDocument {
    organisation: String,
    key_id: String,
    served_at: String,
    credentials: Vec<ServedCredential>,
}

#[derive(Deserialize)]
struct ServedCredential {
    connection_id: String,
    provider: String,
    variable: String,
    value: String,
    #[serde(default)]
    rotated_at: Option<String>,
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The release URL: [`RELEASE_PATH`] on the origin of the deployment's
/// `policy_url`. That origin is the one the hub verifies an edge's
/// `@authority` against for the policy request, and the contract signs this
/// request under the same authority. The path set here is the `@path` signed:
/// the path the edge addresses, which the contract requires a hub behind its
/// web layer's `/api` proxy to verify in place of the path forwarded to it.
/// The query is removed because the contract has the hub refuse one. Held to
/// the policy URL's transport rule, since the answer carries a secret under
/// TLS and nothing else.
fn release_url(policy_url: &str) -> Result<String, String> {
    let mut url = url::Url::parse(policy_url)
        .map_err(|error| format!("policy_url {policy_url:?} is not a URL: {error}"))?;
    url.set_path(RELEASE_PATH);
    url.set_query(None);
    url.set_fragment(None);
    let url = url.to_string();
    policy_url_accepted(&url).map_err(|_| {
        format!("supplier credentials are fetched over https, or http to a loopback origin: {url}")
    })?;
    Ok(url)
}

/// A hub reason made safe to keep: control characters dropped, cut to
/// [`REASON_LIMIT`].
fn bounded(reason: &str) -> String {
    reason
        .chars()
        .filter(|character| !character.is_control())
        .take(REASON_LIMIT)
        .collect()
}

/// The first non-empty string among `fields` of a JSON answer, bounded.
fn stable_field(body: &[u8], fields: &[&str]) -> Option<String> {
    let body = serde_json::from_slice::<Value>(body).ok()?;
    fields
        .iter()
        .filter_map(|field| body[*field].as_str().map(bounded))
        .find(|value| !value.is_empty())
}

/// The `code` of a `401` answer when it is exactly one of [`FAULTS`], compared
/// before any bounding so a padded code is not read as a fault.
fn fault_code(body: &[u8]) -> Option<String> {
    let body = serde_json::from_slice::<Value>(body).ok()?;
    let code = body["code"].as_str()?;
    FAULTS.contains(&code).then(|| code.to_owned())
}

/// The stable reason a refusal body carries (the hub's `code`, then `reason`,
/// as the registration routes answer, then `detail`), or the status where it
/// carries none.
fn refusal_reason(status: u16, body: &[u8]) -> String {
    stable_field(body, &["code", "reason", "detail"]).unwrap_or_else(|| format!("http_{status}"))
}

/// Fetch this edge's released credentials into `store` and record what
/// happened. The caller supplies `now`, used for the signature and as the
/// fetch time.
///
/// `Err` is a refusal to try, or a record that could not be written: an edge
/// that is not enrolled or not managed, a release URL this runtime will not
/// send a signed request to. Nothing is changed in `store` on a refusal to
/// try. Everything the hub does or fails to do is an `Ok` with its outcome.
pub fn fetch(
    home: &Path,
    store: &ReleasedStore,
    now: DateTime<Utc>,
    budget: Duration,
) -> Result<Fetch, String> {
    let Some(enrolment) = EnrolmentRecord::load(home)? else {
        return Err(format!(
            "this edge is not enrolled with a hub, so no hub releases credentials to it: no \
             enrolment record at {}",
            EnrolmentRecord::path(home).display()
        ));
    };
    let Deployment::Managed { policy_url, .. } = Deployment::read(home)? else {
        return Err(format!(
            "deployment mode is local: supplier credentials are released to a managed hosted \
             edge only ({})",
            Deployment::path(home).display()
        ));
    };
    let url = release_url(&policy_url)?;
    let at = rfc3339(now);
    let mut fetch = Fetch {
        version: 1,
        at: at.clone(),
        url: url.clone(),
        outcome: Outcome::Unreachable,
        http_status: None,
        reason: None,
        retried_after: None,
        served_at: None,
        held: Vec::new(),
        ignored: Vec::new(),
        expired: Vec::new(),
        max_age_seconds: None,
    };

    let identity = Identity::load(home)?;
    if identity.signer().is_none() {
        // The hub would answer an unsigned request `401`, and an edge that
        // knows its key is revoked has already been told the same thing.
        store.clear();
        fetch.outcome = Outcome::Refused;
        fetch.reason = Some(format!(
            "{}; no request was made",
            identity
                .presented()
                .unsigned
                .unwrap_or_else(|| "this edge holds no key to sign with".to_owned())
        ));
        return conclude(home, store, fetch);
    }
    // Resolved once: name resolution is outside the budget, and a retry goes
    // to the addresses the first request went to.
    let Ok(addresses) = commonmeasure_http::resolve(&url) else {
        fetch.reason = Some(NO_USABLE_ANSWER.to_owned());
        return conclude(home, store, fetch);
    };
    let started = Instant::now();
    let response = loop {
        let elapsed = started.elapsed();
        let remaining = budget.saturating_sub(elapsed);
        let signed_at = now + chrono::Duration::from_std(elapsed).unwrap_or_default();
        let request = match signed_request(&identity, &url, signed_at) {
            Ok(request) => request,
            Err(reason) => {
                fetch.reason = Some(format!("the request could not be signed: {reason}"));
                return conclude(home, store, fetch);
            }
        };
        let Ok(response) = commonmeasure_http::send_to(&url, &addresses, request, remaining) else {
            fetch.reason = Some(NO_USABLE_ANSWER.to_owned());
            return conclude(home, store, fetch);
        };
        if response.status == 401
            && let Some(code) = fault_code(&response.body)
        {
            if fetch.retried_after.is_none() && worth_retrying(budget, started.elapsed()) {
                fetch.retried_after = Some(code);
                continue;
            }
            // A fault twice over, or one that left too little budget to ask
            // again, rules on nothing: the set is kept within its maximum age.
            fetch.http_status = Some(401);
            fetch.reason = Some(code);
            return conclude(home, store, fetch);
        }
        break response;
    };
    fetch.http_status = Some(response.status);
    match response.status {
        200 => {}
        401 => {
            // `key_unknown`, `key_revoked`, no code (as a hub before the
            // stable codes answers every `401`), or a code that is not one of
            // `FAULTS`.
            store.clear();
            fetch.outcome = Outcome::Refused;
            fetch.reason = Some(refusal_reason(401, &response.body));
            return conclude(home, store, fetch);
        }
        status => {
            // A `400`, a `404` from a hub with no public origin, a `429`, a
            // `5xx`: none rules on what this edge may hold.
            fetch.reason = Some(refusal_reason(status, &response.body));
            return conclude(home, store, fetch);
        }
    }

    let Ok(document) = serde_json::from_slice::<ReleaseDocument>(&response.body) else {
        fetch.reason = Some("the hub answered 200 with no release document".to_owned());
        return conclude(home, store, fetch);
    };
    if document.key_id != enrolment.key_id || document.organisation != enrolment.organization.id {
        fetch.reason = Some(
            "the hub answered 200 with a release addressed to another key or organisation"
                .to_owned(),
        );
        return conclude(home, store, fetch);
    }
    fetch.served_at = Some(bounded(&document.served_at));

    let mut providers = BTreeSet::new();
    let mut released = Vec::new();
    for served in document.credentials {
        let mut ignore = |reason: &str| {
            fetch.ignored.push(Ignored {
                connection_id: bounded(&served.connection_id),
                provider: bounded(&served.provider),
                reason: reason.to_owned(),
            });
        };
        if providers.contains(&served.provider) {
            ignore("a second connection for a provider already served; the contract allows one");
            continue;
        }
        match Released::new(
            &served.connection_id,
            &served.provider,
            &served.variable,
            &served.value,
            served.rotated_at.as_deref().map(bounded),
            &at,
        ) {
            Some(credential) => {
                providers.insert(served.provider);
                released.push(credential);
            }
            None => ignore("not a remote provider's own variable with a value, so it is not held"),
        }
    }
    // An empty list is what a withdrawn edge sees: the store is cleared.
    fetch.outcome = if store.replace(released) {
        Outcome::Accepted
    } else {
        Outcome::Unchanged
    };
    conclude(home, store, fetch)
}

/// A release request signed at `at` with a fresh nonce.
fn signed_request(
    identity: &Identity,
    url: &str,
    at: DateTime<Utc>,
) -> Result<commonmeasure_http::Request, String> {
    let signer = identity
        .signer()
        .ok_or("this edge holds no key to sign with")?;
    let mut request = commonmeasure_http::Request::get(RELEASE_PATH);
    request.headers.set("User-Agent", USER_AGENT);
    request.headers.set("Accept", "application/json");
    // The hub refuses a release request that does not cover both. They are
    // asked for here and nowhere else: a hub before this signature refuses
    // a signature that covers them, and the contract forbids them on the
    // policy request and on any request signed for a publisher.
    signer.sign_request_covering(
        url,
        &mut request,
        &[Derived::Method, Derived::Path],
        &[],
        at.timestamp(),
    )?;
    Ok(request)
}

/// Record the fetch with what `store` holds after it, in use and expired.
fn conclude(home: &Path, store: &ReleasedStore, mut fetch: Fetch) -> Result<Fetch, String> {
    let held = |names: Vec<ReleasedName>| -> Vec<Held> {
        names
            .into_iter()
            .map(|name| Held {
                rotated_at: name.rotated_at,
                connection_id: name.connection_id,
                provider: name.provider,
                variable: name.variable,
                fetched_at: name.fetched_at,
            })
            .collect()
    };
    fetch.held = held(store.names());
    fetch.expired = held(store.expired());
    if !fetch.expired.is_empty() {
        fetch.max_age_seconds = Some(store.max_age().as_secs());
    }
    let encoded = serde_json::to_vec_pretty(&fetch)
        .map_err(|error| format!("serialise the fetch record: {error}"))?;
    write_owner_only(&Fetch::path(home), &encoded)?;
    Ok(fetch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_url_drops_the_policy_urls_path_query_and_fragment() {
        assert_eq!(
            release_url("https://h/x?y=1#z").unwrap(),
            format!("https://h{RELEASE_PATH}")
        );
        assert_eq!(
            release_url("http://127.0.0.1:8080/api/v1/policy/desired?tenant=a").unwrap(),
            format!("http://127.0.0.1:8080{RELEASE_PATH}")
        );
    }
    // Catches (EGR-13): a retry sent with a sliver of budget, which can only
    // time out and loses the fault's code from the record.
    #[test]
    fn a_retry_needs_a_quarter_of_the_budget_left() {
        let budget = Duration::from_secs(4);
        let at = Duration::from_millis;
        assert!(worth_retrying(budget, at(0)));
        assert!(worth_retrying(budget, at(3_000)));
        assert!(!worth_retrying(budget, at(3_001)));
        assert!(!worth_retrying(budget, at(4_000)));
        assert!(!worth_retrying(budget, at(5_000)));
        assert!(!worth_retrying(Duration::ZERO, at(0)));
    }
}
