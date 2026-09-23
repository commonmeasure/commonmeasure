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
        self.store_keys(&fetched);
        cache.keys = fetched;
        Ok(cache.keys.len())
    }

    /// Write the fetched keys beside the home. A cache that cannot be
    /// written costs the next process a fetch; it does not fail a
    /// verification.
    fn store_keys(&self, keys: &HashMap<String, [u8; 32]>) {
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
        if let Ok(encoded) = serde_json::to_vec_pretty(&stored) {
            let _ = std::fs::write(self.home.join(KEY_CACHE_FILE), encoded);
        }
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
                "token algorithm {algorithm:?} is not accepted; the hub signs {ACCEPTED_ALGORITHM}"
            ));
        }
        let issuer = claims_value["iss"].as_str().unwrap_or("");
        if issuer.trim_end_matches('/') != self.issuer {
            return Err(format!(
                "token issuer {issuer:?} is not the issuer pinned at enrolment, {}",
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
            .map_err(|_| format!("the token signature does not verify under key {kid}"))?;

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
                "signing key {kid} is not one the issuer {} published when last asked, {}s \
                 ago; it is asked again after {}s",
                self.issuer,
                at.elapsed().as_secs(),
                KEY_REFETCH_INTERVAL.as_secs()
            ));
        }
        cache.last_fetch = Some(Instant::now());
        let fetched = fetch_keys(&self.issuer)?;
        self.store_keys(&fetched);
        cache.keys = fetched;
        cache.keys.get(kid).copied().ok_or_else(|| {
            format!(
                "signing key {kid} is not one the issuer {} publishes",
                self.issuer
            )
        })
    }
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

fn load(home: &Path) -> Result<EdgeTokens, String> {
    let source = path(home);
    match std::fs::read(&source) {
        Ok(encoded) => serde_json::from_slice(&encoded)
            .map_err(|error| format!("{} is not a valid token file: {error}", source.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EdgeTokens::default()),
        Err(error) => Err(format!("cannot read {}: {error}", source.display())),
    }
}

/// Write-then-rename, owner-readable: the file holds hashes, but a hash of a
/// bearer secret is still a fact about who may call.
fn store(home: &Path, tokens: &EdgeTokens) -> Result<(), String> {
    let target = path(home);
    let tmp = target.with_extension("json.tmp");
    let encoded =
        serde_json::to_vec_pretty(tokens).map_err(|error| format!("serialise tokens: {error}"))?;
    commonmeasure_harness::identity::write_private(&tmp, &encoded)?;
    std::fs::rename(&tmp, &target)
        .map_err(|error| format!("cannot write {}: {error}", target.display()))
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
    let mut tokens = load(home)?;
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
    let mut tokens = load(home)?;
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
    let tokens = load(home)?;
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
        assert!(refused.contains("HS256"), "{refused}");
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
        assert!(
            verifier.keys.lock().expect("lock").last_fetch.is_none(),
            "no fetch was made for a token that is not ours"
        );
    }
}
