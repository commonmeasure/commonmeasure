//! Who is calling the hosted edge: the resource-server half of its
//! authentication, which turns a bearer token into a principal.
//!
//! Two kinds of bearer token reach an endpoint. An access token the hub
//! issued is a JWT signed EdDSA under a `kid` that is the RFC 7638 thumbprint
//! of its Ed25519 key, with the claims its token module issues: `iss`, `sub`,
//! `aud` (one string, the resource as the metadata names it), `org`,
//! `client_id`, `scope`, `iat`, `exp`, `jti`. It is verified here for
//! signature, issuer, audience, expiry and organisation against the signing
//! keys the issuer pinned at enrolment publishes at the `jwks_uri` its
//! metadata names, and its `sub` becomes the principal. A
//! token this edge issued itself, for a host with no OAuth, is a random
//! secret held only as a hash; its label becomes the principal. Neither
//! verification calls the hub while ruling on a crossing: the keys are
//! cached on disk and refetched only for a key id the cache does not hold.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{SecondsFormat, Utc};
use commonmeasure_harness::policy::Principal;
use commonmeasure_runtime::declaration::{self, LockRefused};
use commonmeasure_types::canonical::sha256_digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every token this edge issues starts with this, so a presented token is
/// routed to the hash table or to the JWT verifier without guessing.
const EDGE_TOKEN_PREFIX: &str = "cmet_";

/// The one signature algorithm accepted. The hub signs with the key type the
/// rest of the fleet already uses; a token naming any other algorithm is
/// refused by name rather than verified under a weaker one.
const ACCEPTED_ALGORITHM: &str = "EdDSA";

/// The claim carrying the organisation id the hub issued the token under
/// (`org` in the hub's `AccessClaims`, its OAuth token module). It must equal
/// the organisation in the enrolment record.
pub(crate) const ORGANISATION_CLAIM: &str = "org";

/// How far the hub's clock and this edge's may disagree before a fresh token
/// reads as expired or not yet valid.
const CLOCK_LEEWAY_SECS: i64 = 60;

/// How long after one fetch of the issuer's keys a token naming an unknown
/// key id waits before another is made. Without it a stream of tokens with
/// invented key ids would have this edge fetch the issuer's keys once per
/// request.
const KEY_REFETCH_INTERVAL: Duration = Duration::from_secs(60);

/// The budget for the two requests that fetch the issuer's keys.
const KEY_FETCH_BUDGET: Duration = Duration::from_secs(5);

/// Where the issuer's keys are cached under the operator home.
const KEY_CACHE_FILE: &str = "hosted-jwks.json";

/// Where the hashes of edge-issued tokens live under the operator home.
const EDGE_TOKENS_FILE: &str = "hosted-tokens.json";

/// The lock `issue` and `revoke` hold across their read, change and write
/// of [`EDGE_TOKENS_FILE`], beside it in the operator home.
const EDGE_TOKENS_LOCK: &str = "hosted-tokens.lock";

/// Who a verified token names. Compared to bind a session to the identity
/// that opened it: a session is answered only to the same bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bearer {
    /// The `sub` claim of a hub-issued access token.
    Subject(String),
    /// The label of a token this edge issued.
    EdgeToken(String),
}

impl Bearer {
    /// The principal the policy is resolved for.
    pub(crate) fn principal(&self) -> Principal {
        match self {
            Self::Subject(subject) => Principal::oauth_subject(subject),
            Self::EdgeToken(label) => Principal::edge_token(label),
        }
    }
}

/// Verifies presented tokens for one hosted edge: one issuer, one
/// organisation, one key cache.
pub(crate) struct Verifier {
    home: PathBuf,
    issuer: String,
    organisation: String,
    keys: Mutex<KeyCache>,
}

/// The issuer's Ed25519 public keys by key id, and when they were last
/// fetched in this process.
#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, [u8; 32]>,
    last_fetch: Option<Instant>,
}

/// The disk form of the key cache: the issuer it was fetched from, so a home
/// re-enrolled with another hub does not verify under the old hub's keys.
#[derive(Serialize, Deserialize)]
struct StoredKeys {
    issuer: String,
    fetched_at: String,
    keys: Vec<StoredKey>,
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    kid: String,
    /// The public key, base64url unpadded, as the JWK carries it.
    x: String,
}

impl Verifier {
    /// A verifier for tokens `issuer` signs for `organisation`, with the
    /// key cache under `home` loaded when it is the same issuer's.
    pub(crate) fn new(home: &Path, issuer: &str, organisation: &str) -> Self {
        let issuer = issuer.trim_end_matches('/').to_owned();
        let mut cache = KeyCache::default();
        if let Ok(encoded) = std::fs::read(home.join(KEY_CACHE_FILE))
            && let Ok(stored) = serde_json::from_slice::<StoredKeys>(&encoded)
            && stored.issuer == issuer
        {
            for key in stored.keys {
                if let Some(bytes) = decode_public_key(&key.x) {
                    cache.keys.insert(key.kid, bytes);
                }
            }
        }
        Self {
            home: home.to_owned(),
            issuer,
            organisation: organisation.to_owned(),
            keys: Mutex::new(cache),
        }
    }

    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Verify `token` as presented to the endpoint `host`, whose canonical
    /// URL is `audience`. The error is the refusal, naming the check that
    /// failed and nothing a caller could use to forge the next attempt.
    pub(crate) fn verify(&self, token: &str, audience: &str, host: &str) -> Result<Bearer, String> {
        if token.starts_with(EDGE_TOKEN_PREFIX) {
            return verify_edge_token(&self.home, token, host);
        }
        self.verify_access_token(token, audience)
    }

    /// Fetch the issuer's keys now and replace the cache with what it
    /// publishes, so a key the issuer withdrew stops verifying within one
    /// interval rather than at the next unknown key id. A fetch that fails
    /// leaves the cache as it was: the keys held are still the ones last
    /// published, and refusing every token on a hub outage would make the
    /// hub a dependency of each crossing's ruling.
    pub(crate) fn refresh_keys(&self) -> Result<usize, String> {
        let mut cache = self
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fetched = fetch_keys(&self.issuer)?;
        cache.last_fetch = Some(Instant::now());
        self.hold_keys(&mut cache, fetched);
        Ok(cache.keys.len())
    }

    /// Verify under `fetched` from now on, and write them to the disk cache
    /// for the next process.
    ///
    /// A cache that cannot be written does not fail a verification, but it is
    /// logged: after the issuer withdraws a key, the earlier cache still
    /// holds it, and the next process verifies under it until it refetches.
    fn hold_keys(&self, cache: &mut KeyCache, fetched: HashMap<String, [u8; 32]>) {
        if let Err(error) = self.store_keys(&fetched) {
            log_fault(format!(
                "issuer keys held in memory only; the key cache was not written: {error}"
            ));
        }
        cache.keys = fetched;
    }

    /// Replace the key cache beside the home whole, so a failed write leaves
    /// the earlier cache rather than a torn one. The keys are public, so the
    /// file is written under the umask like the rest of the home;
    /// [`declaration::replace_private`] would add nothing.
    fn store_keys(&self, keys: &HashMap<String, [u8; 32]>) -> Result<(), String> {
        let stored = StoredKeys {
            issuer: self.issuer.clone(),
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            keys: keys
                .iter()
                .map(|(kid, key)| StoredKey {
                    kid: kid.clone(),
                    x: URL_SAFE_NO_PAD.encode(key),
                })
                .collect(),
        };
        let encoded = serde_json::to_vec_pretty(&stored)
            .map_err(|error| format!("serialise {KEY_CACHE_FILE}: {error}"))?;
        declaration::replace(&self.home.join(KEY_CACHE_FILE), &encoded)
    }

    /// The checks in the order that spends least on a token that is not
    /// ours: structure and algorithm, then the issuer claim before any key
    /// is looked up (a token from another issuer must not cost a fetch of
    /// ours), then the signature, then the claims the signature covers.
    fn verify_access_token(&self, token: &str, audience: &str) -> Result<Bearer, String> {
        let mut segments = token.split('.');
        let (Some(header), Some(claims), Some(signature), None) = (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) else {
            return Err("the token is not a JWT: three base64url segments are expected".to_owned());
        };
        let header_value = decode_json(header).ok_or("the token header is not base64url JSON")?;
        let claims_value = decode_json(claims).ok_or("the token claims are not base64url JSON")?;
        let algorithm = header_value["alg"].as_str().unwrap_or("none");
        if algorithm != ACCEPTED_ALGORITHM {
            return Err(format!(
                "the token's algorithm is not accepted; the hub signs {ACCEPTED_ALGORITHM}"
            ));
        }
        let issuer = claims_value["iss"].as_str().unwrap_or("");
        if issuer.trim_end_matches('/') != self.issuer {
            return Err(format!(
                "the token's issuer is not the issuer pinned at enrolment, {}",
                self.issuer
            ));
        }
        let kid = header_value["kid"]
            .as_str()
            .filter(|kid| !kid.is_empty())
            .ok_or("the token names no signing key (kid)")?;
        let key = self.key(kid)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| "the token signature is not base64url".to_owned())?;
        let signed = &token[..header.len() + 1 + claims.len()];
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(signed.as_bytes(), &signature)
            .map_err(|_| "the token signature does not verify under the key it names".to_owned())?;

        let now = Utc::now().timestamp();
        let expiry = claims_value["exp"]
            .as_i64()
            .ok_or("the token has no expiry (exp)")?;
        if expiry + CLOCK_LEEWAY_SECS <= now {
            return Err(format!("the token expired at {}", timestamp(expiry)));
        }
        if let Some(not_before) = claims_value["nbf"].as_i64()
            && not_before - CLOCK_LEEWAY_SECS > now
        {
            return Err(format!(
                "the token is not valid before {}",
                timestamp(not_before)
            ));
        }
        if !names_audience(&claims_value["aud"], audience) {
            return Err(format!(
                "token audience {} is not this endpoint, {audience}",
                claims_value["aud"]
            ));
        }
        match claims_value[ORGANISATION_CLAIM].as_str() {
            Some(organisation) if organisation == self.organisation => {}
            Some(organisation) => {
                return Err(format!(
                    "token organisation {organisation:?} is not the enrolled organisation {}",
                    self.organisation
                ));
            }
            None => {
                return Err(format!(
                    "the token names no organisation ({ORGANISATION_CLAIM})"
                ));
            }
        }
        let subject = claims_value["sub"]
            .as_str()
            .filter(|subject| !subject.is_empty())
            .ok_or("the token has an empty or missing subject (sub)")?;
        Ok(Bearer::Subject(subject.to_owned()))
    }

    /// The public key for `kid`: from the cache, or fetched from the issuer
    /// when the cache does not hold it and no fetch was made within
    /// [`KEY_REFETCH_INTERVAL`]. The lock is held across the fetch, so two
    /// requests arriving under a new key together cost one fetch and both
    /// find it; every verification waits for that fetch, at most
    /// [`KEY_FETCH_BUDGET`] once per interval.
    fn key(&self, kid: &str) -> Result<[u8; 32], String> {
        let mut cache = self
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(key) = cache.keys.get(kid) {
            return Ok(*key);
        }
        if let Some(at) = cache.last_fetch
            && at.elapsed() < KEY_REFETCH_INTERVAL
        {
            return Err(format!(
                "the token's signing key is not one the issuer {} published when last asked, {}s \
                 ago; it is asked again after {}s",
                self.issuer,
                at.elapsed().as_secs(),
                KEY_REFETCH_INTERVAL.as_secs()
            ));
        }
        cache.last_fetch = Some(Instant::now());
        let fetched = fetch_keys(&self.issuer)?;
        self.hold_keys(&mut cache, fetched);
        cache.keys.get(kid).copied().ok_or_else(|| {
            format!(
                "the token's signing key is not one the issuer {} publishes",
                self.issuer
            )
        })
    }
}

/// Say a fault on standard error, where the hosted service says its others
/// and where its service log receives them.
fn log_fault(line: String) {
    #[cfg(test)]
    tests::LOGGED.with(|logged| logged.borrow_mut().push(line.clone()));
    eprintln!("commonmeasure: {line}");
}

/// Fetch the issuer's Ed25519 keys: its authorisation-server metadata
/// (RFC 8414) names `jwks_uri`, and that document carries the keys. Keys of
/// any other type are skipped; a set holding none is an error.
fn fetch_keys(issuer: &str) -> Result<HashMap<String, [u8; 32]>, String> {
    let metadata_url = format!("{issuer}/.well-known/oauth-authorization-server");
    let metadata = fetch_json(&metadata_url)?;
    let jwks_uri = metadata["jwks_uri"]
        .as_str()
        .ok_or_else(|| format!("{metadata_url} names no jwks_uri"))?;
    let jwks = fetch_json(jwks_uri)?;
    let keys: HashMap<String, [u8; 32]> = jwks["keys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter(|key| key["kty"] == "OKP" && key["crv"] == "Ed25519")
                .filter_map(|key| {
                    let kid = key["kid"].as_str()?.to_owned();
                    let bytes = decode_public_key(key["x"].as_str()?)?;
                    Some((kid, bytes))
                })
                .collect()
        })
        .unwrap_or_default();
    if keys.is_empty() {
        return Err(format!("{jwks_uri} publishes no Ed25519 key"));
    }
    Ok(keys)
}

fn fetch_json(url: &str) -> Result<Value, String> {
    let mut request = commonmeasure_http::Request::get(url);
    request.headers.set("Accept", "application/json");
    request.headers.set(
        "User-Agent",
        &format!("commonmeasure-hosted/{}", env!("CARGO_PKG_VERSION")),
    );
    let response = commonmeasure_http::send_with_timeout(url, request, KEY_FETCH_BUDGET)
        .map_err(|error| format!("could not fetch {url}: {error:#}"))?;
    if response.status != 200 {
        return Err(format!("{url} answered {}", response.status));
    }
    serde_json::from_slice(&response.body).map_err(|error| format!("{url} is not JSON: {error}"))
}

fn decode_json(segment: &str) -> Option<Value> {
    let bytes = URL_SAFE_NO_PAD.decode(segment).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn decode_public_key(x: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(x).ok()?.try_into().ok()
}

/// RFC 7519 lets `aud` be one string or an array of them.
fn names_audience(claim: &Value, audience: &str) -> bool {
    match claim {
        Value::String(one) => one == audience,
        Value::Array(many) => many.iter().any(|one| one == audience),
        _ => false,
    }
}

fn timestamp(seconds: i64) -> String {
    chrono::DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| seconds.to_string())
}

/// One token this edge issued. The secret itself is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EdgeToken {
    pub(crate) label: String,
    /// `sha256:<hex>` over the secret's bytes.
    pub(crate) sha256: String,
    pub(crate) issued_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) revoked_at: Option<String>,
    /// The one host word the token is accepted on, when it was issued with
    /// `--host`. An unbound token is accepted on every endpoint; the record
    /// tells the two apart by nothing but the basis, so an operator who
    /// wants the weaker basis confined to the host without OAuth binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EdgeTokens {
    tokens: Vec<EdgeToken>,
}

/// Where the hashes live.
pub(crate) fn path(home: &Path) -> PathBuf {
    home.join(EDGE_TOKENS_FILE)
}

/// Hold the token file against every other writer, in this process or
/// another, until the guard drops. Without it an `issue` that read the file
/// before a concurrent `revoke` wrote it back would write its older copy over
/// the revocation, and the revoked token would verify again. Readers take no
/// lock: `store` replaces the file by rename, so a reader sees one whole
/// version or the other.
///
/// A busy refusal is worded here rather than taken from [`declaration::lock`],
/// whose text speaks of an edit and would say what did not happen twice.
fn lock(home: &Path, change: &str) -> Result<declaration::Lock, String> {
    let path = home.join(EDGE_TOKENS_LOCK);
    declaration::lock(&path).map_err(|refused| match refused {
        LockRefused::Busy(_) => format!(
            "another process has held {} for {}s; no token was {change}",
            path.display(),
            declaration::LOCK_DEADLINE.as_secs()
        ),
        refused @ (LockRefused::LockFile(_) | LockRefused::Failed(_)) => {
            format!("no token was {change}: {refused}")
        }
    })
}

fn load(home: &Path) -> Result<EdgeTokens, String> {
    let source = path(home);
    match std::fs::read(&source) {
        Ok(encoded) => serde_json::from_slice(&encoded)
            .map_err(|error| format!("{} is not a valid token file: {error}", source.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EdgeTokens::default()),
        Err(error) => Err(format!("cannot read {}: {error}", source.display())),
    }
}

/// Replace the file owner-readable: it holds hashes, but a hash of a bearer
/// secret is still a fact about who may call. [`declaration::replace_private`]
/// writes a temporary of its own at 0600, removes it if the write fails, and
/// syncs the directory after the rename, so a revocation that returned is
/// still there after a crash. Without that sync, ext4 and XFS on Linux make
/// the rename durable only at their next journal commit, seconds later; on
/// Windows there is no directory sync and the rename is as durable as NTFS
/// makes it. The caller holds [`lock`].
fn store(home: &Path, tokens: &EdgeTokens) -> Result<(), String> {
    let encoded =
        serde_json::to_vec_pretty(tokens).map_err(|error| format!("serialise tokens: {error}"))?;
    declaration::replace_private(&path(home), &encoded)
}

/// Mint a token for `label`, bound to the endpoint `host` when given, and
/// return the secret, the one time it exists in the clear. A label is issued
/// once; revoking it does not free it, because the record's `token:<label>`
/// must keep naming one principal.
pub(crate) fn issue(home: &Path, label: &str, host: Option<&str>) -> Result<String, String> {
    if let Some(host) = host
        && !crate::hosted::HOST_WORDS.contains(&host)
    {
        return Err(format!(
            "--host {host:?} is not an endpoint; the endpoints are {}",
            crate::hosted::HOST_WORDS.join(", ")
        ));
    }
    if label.is_empty()
        || label.len() > 128
        || !label
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
    {
        return Err(
            "a token label is 1 to 128 printable ASCII characters with no space, quote or \
             backslash"
                .to_owned(),
        );
    }
    let _lock = lock(home, "issued")?;
    let mut tokens = load(home)?;
    #[cfg(test)]
    tests::after_issue_loaded(home);
    if tokens.tokens.iter().any(|token| token.label == label) {
        return Err(format!(
            "an edge token for {label:?} was already issued; revoke it and choose another label"
        ));
    }
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret)
        .map_err(|error| format!("draw token material from the operating system: {error}"))?;
    let token = format!("{EDGE_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(secret));
    tokens.tokens.push(EdgeToken {
        label: label.to_owned(),
        sha256: sha256_digest(token.as_bytes()),
        issued_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        revoked_at: None,
        host: host.map(str::to_owned),
    });
    store(home, &tokens)?;
    Ok(token)
}

/// Revoke `label`'s token. The file is read on every request, so the next
/// request carrying it is refused without a restart.
pub(crate) fn revoke(home: &Path, label: &str) -> Result<(), String> {
    let _lock = lock(home, "revoked")?;
    let mut tokens = load(home)?;
    #[cfg(test)]
    tests::after_revoke_loaded(home);
    let token = tokens
        .tokens
        .iter_mut()
        .find(|token| token.label == label)
        .ok_or_else(|| format!("no edge token was issued for {label:?}"))?;
    if let Some(at) = &token.revoked_at {
        return Err(format!(
            "the edge token for {label:?} was already revoked at {at}"
        ));
    }
    token.revoked_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
    store(home, &tokens)
}

pub(crate) fn list(home: &Path) -> Result<Vec<EdgeToken>, String> {
    Ok(load(home)?.tokens)
}

fn verify_edge_token(home: &Path, token: &str, host: &str) -> Result<Bearer, String> {
    let digest = sha256_digest(token.as_bytes());
    // The refusal reaches a caller no token has authenticated yet, so it
    // names the file and not where the operator keeps it; the service log
    // keeps the path.
    let tokens = load(home).map_err(|reason| {
        log_fault(format!("edge tokens not verified: {reason}"));
        reason.replace(&path(home).display().to_string(), EDGE_TOKENS_FILE)
    })?;
    let held = tokens
        .tokens
        .iter()
        .find(|held| held.sha256 == digest)
        .ok_or("the edge token is not one this edge issued")?;
    if let Some(at) = &held.revoked_at {
        return Err(format!("edge token {:?} was revoked at {at}", held.label));
    }
    if let Some(bound) = &held.host
        && bound != host
    {
        return Err(format!(
            "edge token {:?} is bound to the endpoint /mcp/{bound} and was presented on /mcp/{host}",
            held.label
        ));
    }
    Ok(Bearer::EdgeToken(held.label.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;

    type Pause = (PathBuf, Arc<dyn Fn() + Send + Sync>);

    thread_local! {
        /// What [`log_fault`] said on this thread.
        pub(super) static LOGGED: std::cell::RefCell<Vec<String>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn take_logged() -> Vec<String> {
        LOGGED.with(|logged| std::mem::take(&mut *logged.borrow_mut()))
    }

    /// A pause `issue` takes after reading the file, for the one home a test
    /// names, so a test can place another writer between that read and the
    /// write. Keyed by home because the module's other tests run in parallel.
    static ISSUE_PAUSE: Mutex<Option<Pause>> = Mutex::new(None);

    /// The same pause in `revoke`.
    static REVOKE_PAUSE: Mutex<Option<Pause>> = Mutex::new(None);

    pub(super) fn after_issue_loaded(home: &Path) {
        take_pause(&ISSUE_PAUSE, home);
    }

    pub(super) fn after_revoke_loaded(home: &Path) {
        take_pause(&REVOKE_PAUSE, home);
    }

    fn take_pause(slot: &Mutex<Option<Pause>>, home: &Path) {
        let pause = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|(held, _)| held == home)
            .map(|(_, pause)| Arc::clone(pause));
        if let Some(pause) = pause {
            pause();
        }
    }

    /// EGR-60. `issue` reads the file, then a revoke from another writer
    /// lands, then `issue` writes. Unlocked, the issue writes back the
    /// snapshot it read and the revoked token stands again.
    ///
    /// Two threads stand in for two processes: the lock is `flock` (or
    /// `LockFileEx`), held per open file description, and each call opens
    /// the lock file itself, so the threads contend as two processes do.
    /// The paused issue waits for the revoke to finish, or for a second:
    /// with the lock held the revoke cannot finish, and a second is inside
    /// the lock's two-second deadline, so the revoke then takes the lock
    /// after the issue releases it rather than being refused.
    #[test]
    fn a_revoke_between_an_issues_read_and_write_is_kept() {
        let home = tempfile::tempdir().expect("tempdir");
        let revoked = issue(home.path(), "a", None).expect("issued a");
        let (loaded_tx, loaded_rx) = mpsc::channel::<()>();
        let (revoked_tx, revoked_rx) = mpsc::channel::<()>();
        let revoked_rx = Mutex::new(revoked_rx);
        *ISSUE_PAUSE.lock().expect("pause") = Some((
            home.path().to_owned(),
            Arc::new(move || {
                loaded_tx.send(()).expect("loaded");
                let _ = revoked_rx
                    .lock()
                    .expect("receiver")
                    .recv_timeout(Duration::from_secs(1));
            }),
        ));
        let revoker = {
            let home = home.path().to_owned();
            std::thread::spawn(move || {
                loaded_rx.recv().expect("issue read the file");
                let outcome = revoke(&home, "a");
                let _ = revoked_tx.send(());
                outcome
            })
        };
        let issued = issue(home.path(), "b", None);
        ISSUE_PAUSE.lock().expect("pause").take();
        issued.expect("issued b");
        revoker.join().expect("revoker").expect("revoked a");

        let refused = verify_edge_token(home.path(), &revoked, "chatgpt")
            .expect_err("a revoked token stays revoked");
        assert!(refused.contains("revoked"), "{refused}");
        let labels: Vec<String> = list(home.path())
            .expect("list")
            .into_iter()
            .map(|token| token.label)
            .collect();
        assert_eq!(labels, ["a", "b"]);
    }

    /// The mirror of the test above: `revoke` reads the file, then an issue
    /// from another writer runs. An `issue` that read the file before taking
    /// the lock would wait for the revoke's lock with that read in hand and
    /// then write it back, and the revoked token would stand again.
    ///
    /// The paused revoke waits a second for the issue to finish, which it
    /// cannot while the lock is held; the issue then takes the lock within
    /// its two-second deadline, after the revoke releases it.
    #[test]
    fn an_issue_between_a_revokes_read_and_write_keeps_the_revocation() {
        let home = tempfile::tempdir().expect("tempdir");
        let revoked = issue(home.path(), "a", None).expect("issued a");
        let (loaded_tx, loaded_rx) = mpsc::channel::<()>();
        let (issued_tx, issued_rx) = mpsc::channel::<()>();
        let issued_rx = Mutex::new(issued_rx);
        *REVOKE_PAUSE.lock().expect("pause") = Some((
            home.path().to_owned(),
            Arc::new(move || {
                loaded_tx.send(()).expect("loaded");
                let _ = issued_rx
                    .lock()
                    .expect("receiver")
                    .recv_timeout(Duration::from_secs(1));
            }),
        ));
        let issuer = {
            let home = home.path().to_owned();
            std::thread::spawn(move || {
                loaded_rx.recv().expect("revoke read the file");
                let outcome = issue(&home, "b", None);
                let _ = issued_tx.send(());
                outcome
            })
        };
        let outcome = revoke(home.path(), "a");
        REVOKE_PAUSE.lock().expect("pause").take();
        outcome.expect("revoked a");
        issuer.join().expect("issuer").expect("issued b");

        let refused = verify_edge_token(home.path(), &revoked, "chatgpt")
            .expect_err("a revoked token stays revoked");
        assert!(refused.contains("revoked"), "{refused}");
        let labels: Vec<String> = list(home.path())
            .expect("list")
            .into_iter()
            .map(|token| token.label)
            .collect();
        assert_eq!(labels, ["a", "b"]);
    }

    #[test]
    fn an_issued_token_verifies_until_revoked_and_a_label_is_issued_once() {
        let home = tempfile::tempdir().expect("tempdir");
        let token = issue(home.path(), "ci:owner/repo", None).expect("issued");
        assert!(token.starts_with(EDGE_TOKEN_PREFIX));
        assert_eq!(
            verify_edge_token(home.path(), &token, "chatgpt"),
            Ok(Bearer::EdgeToken("ci:owner/repo".to_owned()))
        );
        assert!(
            !std::fs::read_to_string(path(home.path()))
                .expect("file")
                .contains(&token),
            "the secret is not stored"
        );
        assert!(issue(home.path(), "ci:owner/repo", None).is_err());
        revoke(home.path(), "ci:owner/repo").expect("revoked");
        let refused = verify_edge_token(home.path(), &token, "chatgpt").expect_err("refused");
        assert!(refused.contains("revoked"), "{refused}");
        assert!(revoke(home.path(), "ci:owner/repo").is_err());
        let unknown =
            verify_edge_token(home.path(), "cmet_unknown", "chatgpt").expect_err("refused");
        assert!(unknown.contains("not one this edge issued"), "{unknown}");
    }

    #[test]
    fn a_change_refused_behind_another_writer_says_nothing_changed() {
        let home = tempfile::tempdir().expect("tempdir");
        issue(home.path(), "a", None).expect("issued a");
        let before = std::fs::read(path(home.path())).expect("file");
        let held = declaration::lock(&home.path().join(EDGE_TOKENS_LOCK)).expect("held");
        let refused = revoke(home.path(), "a").expect_err("busy");
        assert!(
            refused.contains(EDGE_TOKENS_LOCK) && refused.ends_with("no token was revoked"),
            "{refused}"
        );
        assert!(
            !refused.contains("no edit was made"),
            "said once: {refused}"
        );
        drop(held);
        assert_eq!(std::fs::read(path(home.path())).expect("file"), before);
    }

    fn leftovers(home: &Path) -> Vec<String> {
        std::fs::read_dir(home)
            .expect("home")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    /// EGR-123. A write that fails leaves the token file as it was and no
    /// temporary beside it: the directory refuses the temporary, or the
    /// rename fails after the temporary was written.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_leaves_no_temporary_and_the_file_as_it_was() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().expect("tempdir");
        issue(home.path(), "a", None).expect("issued a");
        let before = std::fs::read(path(home.path())).expect("file");
        // SAFETY: `geteuid` has no arguments or memory preconditions. Root
        // writes into a 0555 directory, so the refusal half needs another
        // user.
        if unsafe { libc::geteuid() } != 0 {
            std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o555))
                .expect("read-only home");
            let refused = revoke(home.path(), "a");
            std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700))
                .expect("writable home");
            let refused = refused.expect_err("a read-only home refuses the write");
            // The directory's own error, not a search for a free temporary
            // name that sends the operator after leftover files that do not
            // exist.
            assert!(
                refused.contains("Permission denied") && !refused.contains("tries"),
                "{refused}"
            );
            assert_eq!(std::fs::read(path(home.path())).expect("file"), before);
            assert_eq!(leftovers(home.path()), Vec::<String>::new());
        }

        let blocked = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(path(blocked.path())).expect("a directory where the file goes");
        std::fs::write(path(blocked.path()).join("keep"), b"").expect("non-empty");
        store(blocked.path(), &EdgeTokens::default()).expect_err("the rename is refused");
        assert!(path(blocked.path()).join("keep").exists());
        assert_eq!(leftovers(blocked.path()), Vec::<String>::new());
    }

    /// A temporary an earlier failed write left behind, under a wider mode,
    /// does not pass that mode on to the token file.
    #[cfg(unix)]
    #[test]
    fn a_leftover_temporary_does_not_widen_the_token_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().expect("tempdir");
        let leftover = path(home.path()).with_extension("json.tmp");
        std::fs::write(&leftover, b"").expect("leftover");
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o644)).expect("wide");
        issue(home.path(), "a", None).expect("issued a");
        let mode = std::fs::metadata(path(home.path()))
            .expect("file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// EGR-124. A key cache that cannot be written is said where the service
    /// says its other faults, the earlier cache stays whole, and the fetched
    /// keys are still the ones verified under.
    #[cfg(unix)]
    #[test]
    fn a_key_cache_that_cannot_be_written_is_logged_and_the_earlier_one_kept() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().expect("tempdir");
        let verifier = Verifier::new(home.path(), "https://hub.test", "org-1");
        let mut cache = KeyCache::default();
        verifier.hold_keys(&mut cache, HashMap::from([("old".to_owned(), [1u8; 32])]));
        let cached = home.path().join(KEY_CACHE_FILE);
        let before = std::fs::read(&cached).expect("the first cache is written");
        let _ = take_logged();

        std::fs::set_permissions(&cached, std::fs::Permissions::from_mode(0o444))
            .expect("read-only cache");
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o555))
            .expect("read-only home");
        verifier.hold_keys(&mut cache, HashMap::from([("new".to_owned(), [2u8; 32])]));
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700))
            .expect("writable home");

        assert_eq!(std::fs::read(&cached).expect("cache"), before);
        let logged = take_logged();
        assert!(
            logged.len() == 1 && logged[0].contains(KEY_CACHE_FILE),
            "{logged:?}"
        );
        assert!(cache.keys.contains_key("new") && !cache.keys.contains_key("old"));
    }

    #[test]
    fn a_bound_token_is_accepted_on_its_endpoint_alone() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(issue(home.path(), "ci:x", Some("claude-code")).is_err());
        let token = issue(home.path(), "ci:x", Some("copilot-cloud-agent")).expect("issued");
        assert_eq!(
            verify_edge_token(home.path(), &token, "copilot-cloud-agent"),
            Ok(Bearer::EdgeToken("ci:x".to_owned()))
        );
        let refused = verify_edge_token(home.path(), &token, "chatgpt").expect_err("refused");
        assert!(
            refused.contains("bound to the endpoint /mcp/copilot-cloud-agent"),
            "{refused}"
        );
        assert_eq!(
            list(home.path()).expect("list")[0].host.as_deref(),
            Some("copilot-cloud-agent")
        );
    }

    #[test]
    fn a_token_file_that_does_not_parse_is_named_to_the_caller_and_logged_in_full() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(path(home.path()), "{").expect("token file");
        let _ = take_logged();
        let refused =
            verify_edge_token(home.path(), "cmet_anything", "chatgpt").expect_err("refused");
        assert!(
            refused.starts_with("hosted-tokens.json is not a valid token file: "),
            "{refused}"
        );
        assert!(
            !refused.contains(&home.path().display().to_string()),
            "{refused}"
        );
        let logged = take_logged();
        assert!(
            logged.iter().any(|line| line.contains(&format!(
                "{} is not a valid token file",
                path(home.path()).display()
            ))),
            "the service log keeps the path: {logged:?}"
        );
    }

    #[test]
    fn a_label_with_a_space_or_quote_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(issue(home.path(), "", None).is_err());
        assert!(issue(home.path(), "a b", None).is_err());
        assert!(issue(home.path(), "a\"b", None).is_err());
    }

    #[test]
    fn a_token_that_is_not_a_jwt_or_not_eddsa_is_refused_before_any_key_is_looked_up() {
        let home = tempfile::tempdir().expect("tempdir");
        let verifier = Verifier::new(home.path(), "https://hub.test/", "org-1");
        assert_eq!(verifier.issuer(), "https://hub.test");
        let refused = verifier
            .verify("not-a-jwt", "https://edge.test/mcp/chatgpt", "chatgpt")
            .expect_err("refused");
        assert!(refused.contains("three base64url segments"), "{refused}");
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","kid":"k"}"#);
        let claims = URL_SAFE_NO_PAD.encode(br#"{"iss":"https://hub.test"}"#);
        let refused = verifier
            .verify(
                &format!("{header}.{claims}.sig"),
                "https://edge.test/mcp/chatgpt",
                "chatgpt",
            )
            .expect_err("refused");
        assert!(refused.contains("algorithm is not accepted"), "{refused}");
        assert!(!refused.contains("HS256"), "{refused}");
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"EdDSA","kid":"k"}"#);
        let claims = URL_SAFE_NO_PAD.encode(br#"{"iss":"https://other.test"}"#);
        let refused = verifier
            .verify(
                &format!("{header}.{claims}.sig"),
                "https://edge.test/mcp/chatgpt",
                "chatgpt",
            )
            .expect_err("refused");
        assert!(refused.contains("not the issuer pinned"), "{refused}");
        assert!(!refused.contains("other.test"), "{refused}");
        assert!(
            verifier.keys.lock().expect("lock").last_fetch.is_none(),
            "no fetch was made for a token that is not ours"
        );
    }
}
