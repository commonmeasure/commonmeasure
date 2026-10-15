//! Anonymous passes: the edge's wallet and its client for an Anchor's
//! issuance.
//!
//! Under the anonymous access profile (product decision "Anonymous access
//! profile"), where a source asks for an anonymous pass the edge may present
//! one instead of its named Web Bot Auth signature. The pass comes from an
//! **Anchor**, in the sense of the Anonymous Bot Authentication draft, that
//! the source accepts. It says only that the Anchor found the agent meets a
//! policy the Anchor publishes, such as public interest news requirements
//! (integrity, citation, provenance, reporting); it identifies neither the
//! operator, the reader nor the session. The requirements are kept outside
//! Common Measure, and Common Measure may be one Anchor among several, never
//! the only one. The mechanism is an RFC 9578 publicly verifiable token
//! (blind RSA) until the draft settles. The operator names the Anchor per
//! host in the source policy's terms ([`crate::policy::AnchorTerm`]), so the
//! profile is never ambient.
//!
//! This module is the client side: the Anchor's issuer directory, issuance
//! ahead of any fetch, the wallet the passes are held in, and reading a
//! source's `PrivateToken` challenge. The fetch path (`crate::mcp`) presents
//! a held pass on that challenge; it never asks the Anchor for one then,
//! since an issuance just before the request would let the Anchor, which
//! knows the credential, and the source match the two by time.
//!
//! What keeps a pass from linking the edge to its requests:
//!
//! - **Blinding.** The Anchor signs a blinded request and cannot recognise
//!   the pass it signed when a source later sees it.
//! - **The transport.** Anchor traffic goes through `commonmeasure-http`
//!   unsigned: no Web Bot Auth signature and no `CommonMeasureBot` user
//!   agent, so the Anchor never learns the edge key. It does learn the
//!   credential the edge registered under, which is what it rations by.
//!
//! No async runtime enters the product here. The `privacypass` client is
//! synchronous (`TokenRequest::new`, `TokenResponse::issue_token`); only the
//! crate's issuer and origin key stores are async, and the edge holds
//! neither.

use std::fs::File;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
pub use blind_rsa_signatures::reexports::rand::CryptoRng;
use commonmeasure_http::{Request, Response, send};
use commonmeasure_runtime::policy::normalised_host;
use privacypass::auth::authenticate::{TokenChallenge, parse_www_authenticate_header};
use privacypass::auth::authorize::build_authorization_header;
use privacypass::public_tokens::server::serialize_public_key;
pub use privacypass::public_tokens::{PublicKey, PublicToken};
use privacypass::public_tokens::{TokenRequest, TokenResponse};
use privacypass::{Deserialize as _, Serialize as _, TokenType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Where an issuer publishes its directory (RFC 9578 §4).
pub const DIRECTORY_PATH: &str = "/.well-known/private-token-issuer-directory";
/// RFC 9578 media types.
pub const DIRECTORY_TYPE: &str = "application/private-token-issuer-directory";
pub const TOKEN_REQUEST_TYPE: &str = "application/private-token-request";
pub const TOKEN_RESPONSE_TYPE: &str = "application/private-token-response";

/// How much of an Anchor's refusal is kept, as it stated it. Enough for a
/// ration and a reason; an Anchor that answers with a page is cut short.
const STATED_LIMIT: usize = 2048;

/// The Anchor's current token key, as its issuer directory states it.
pub struct AnchorKey {
    public_key: PublicKey,
    /// The key as the directory served it.
    token_key: Vec<u8>,
    /// The `token_key_id` every pass under this key carries: SHA-256 of the
    /// key's serialisation.
    key_id: [u8; 32],
    request_uri: String,
}

impl AnchorKey {
    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// The key id, lowercase hex, as records carry it.
    pub fn key_id(&self) -> String {
        hex(&self.key_id)
    }
}

/// Read the Anchor's issuer directory: its request URI and its first
/// publicly verifiable (type 2) token key.
///
/// The request URI must be on the Anchor's own origin: the credential is
/// sent there, and a directory cannot redirect it elsewhere.
pub fn directory(origin: &str) -> Result<AnchorKey, String> {
    let url = format!("{origin}{DIRECTORY_PATH}");
    let mut request = Request::get(&url);
    request.headers.set("Accept", DIRECTORY_TYPE);
    let response = send(&url, request)
        .map_err(|error| format!("the anchor's issuer directory at {url}: {error:#}"))?;
    if response.status != 200 {
        return Err(format!(
            "the anchor's issuer directory at {url} answered {}",
            response.status
        ));
    }
    let directory: Value = serde_json::from_slice(&response.body)
        .map_err(|error| format!("the anchor's issuer directory at {url} is not JSON: {error}"))?;
    let encoded = directory["token-keys"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|key| key["token-type"] == json!(TokenType::Public as u16))
        .and_then(|key| key["token-key"].as_str())
        .ok_or_else(|| {
            format!("the anchor's issuer directory at {url} lists no publicly verifiable token key")
        })?;
    let token_key = URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .map_err(|error| format!("the anchor's token key is not base64url: {error}"))?;
    let public_key = PublicKey::from_spki(&token_key)
        .map_err(|error| format!("the anchor's token key is not an RSA key: {error:?}"))?;
    let key_id = key_id_of(&public_key)?;

    let named = directory["issuer-request-uri"].as_str().ok_or_else(|| {
        format!("the anchor's issuer directory at {url} names no issuer-request-uri")
    })?;
    let request_uri = url::Url::parse(&url)
        .and_then(|base| base.join(named))
        .map_err(|error| format!("the anchor's issuer-request-uri {named:?}: {error}"))?;
    if request_uri.origin().ascii_serialization() != origin {
        return Err(format!(
            "the anchor's issuer directory sends token requests to {request_uri}, outside the \
             anchor's origin {origin}; the credential is sent to the anchor alone"
        ));
    }
    Ok(AnchorKey {
        public_key,
        token_key,
        key_id,
        request_uri: request_uri.into(),
    })
}

fn key_id_of(public_key: &PublicKey) -> Result<[u8; 32], String> {
    let serialised = serialize_public_key(public_key)
        .map_err(|error| format!("the anchor's token key: {error:?}"))?;
    Ok(Sha256::digest(serialised).into())
}

/// The challenge a source names `host` in, answered ahead of any fetch:
/// publicly verifiable, issued by the Anchor under its host name, with no
/// redemption context so that a pass can be held until a request needs it,
/// and the source host as the origin information so that it is spent there
/// alone. A source whose challenge differs is answered from its own
/// challenge at fetch time.
pub fn challenge_for(anchor_origin: &str, host: &str) -> Result<TokenChallenge, String> {
    let issuer = url::Url::parse(anchor_origin)
        .ok()
        .and_then(|origin| origin.host_str().map(str::to_owned))
        .ok_or_else(|| format!("{anchor_origin:?} names no anchor host"))?;
    let host = normalised_host(host);
    if host.is_empty() {
        return Err("a pass is held for a named host".to_owned());
    }
    Ok(TokenChallenge::new(
        TokenType::Public,
        &issuer,
        None,
        &[host],
    ))
}

/// One `PrivateToken` challenge a source sent (RFC 9577 §2.1): the token
/// challenge, and the id of the issuer key it names in `token-key`.
pub struct SourceChallenge {
    challenge: TokenChallenge,
    key_id: [u8; 32],
}

impl SourceChallenge {
    pub fn challenge(&self) -> &TokenChallenge {
        &self.challenge
    }

    /// Whether this challenge asks for publicly verifiable passes from the
    /// Anchor at `anchor_origin`, named by its host as [`challenge_for`]
    /// names it.
    pub fn names_anchor(&self, anchor_origin: &str) -> bool {
        self.challenge.token_type() == TokenType::Public
            && url::Url::parse(anchor_origin)
                .ok()
                .and_then(|origin| origin.host_str().map(str::to_owned))
                .is_some_and(|host| host == self.challenge.issuer_name())
    }

    /// Whether the challenge binds the pass to `host`. A challenge that
    /// names no origin would take a pass any origin could also redeem, and
    /// one naming other hosts alone would spend here a pass meant for them,
    /// so neither is answered.
    pub fn names_host(&self, host: &str) -> bool {
        let host = normalised_host(host);
        self.challenge
            .origin_info()
            .iter()
            .any(|named| normalised_host(named) == host)
    }

    /// The RFC 9578 §6.1 truncated token key id of the key the challenge
    /// names: the last byte of its id.
    pub fn truncated_token_key_id(&self) -> u8 {
        self.key_id[31]
    }

    /// How this challenge differs from the one a pass is held for ahead of
    /// a fetch ([`challenge_for`]), in a phrase, or `None` where it is that
    /// challenge.
    pub fn differs_from_held(&self, anchor_origin: &str, host: &str) -> Option<String> {
        let held = challenge_for(anchor_origin, host).ok()?;
        if digest_of(&held).ok() == digest_of(&self.challenge).ok() {
            return None;
        }
        Some(if self.challenge.redemption_context().is_some() {
            "it carries a redemption context of its own, which a pass obtained ahead of the              request cannot answer"
                .to_owned()
        } else if self.challenge.origin_info() != held.origin_info() {
            format!(
                "it binds the pass to {}, not to {} alone",
                self.challenge.origin_info().join(", "),
                held.origin_info().join(", ")
            )
        } else {
            "its encoding differs from the challenge passes are held for".to_owned()
        })
    }
}

/// The `PrivateToken` challenges in a `WWW-Authenticate` value. `Ok` with
/// none where the value names another scheme; `Err` where it names
/// `PrivateToken` and does not parse, or names a key that is not an RSA
/// key.
pub fn source_challenges(value: &str) -> Result<Vec<SourceChallenge>, String> {
    let scheme = value
        .trim_start()
        .split([' ', ','])
        .next()
        .unwrap_or_default();
    if !scheme.eq_ignore_ascii_case("PrivateToken") {
        return Ok(Vec::new());
    }
    let header = http::HeaderValue::from_str(value).map_err(|error| error.to_string())?;
    parse_www_authenticate_header(&header)
        .map_err(|error| format!("the PrivateToken challenge does not parse: {error}"))?
        .into_iter()
        .map(|challenge| {
            let public_key = PublicKey::from_spki(challenge.token_key()).map_err(|error| {
                format!("the challenge's token-key is not an RSA key: {error:?}")
            })?;
            Ok(SourceChallenge {
                challenge: challenge.token_challenge().clone(),
                key_id: key_id_of(&public_key)?,
            })
        })
        .collect()
}

/// The challenge in a source's `WWW-Authenticate` value.
pub fn challenge_from_header(value: &str) -> Result<TokenChallenge, String> {
    let value = http::HeaderValue::from_str(value).map_err(|error| error.to_string())?;
    let challenges = parse_www_authenticate_header(&value).map_err(|error| format!("{error:?}"))?;
    challenges
        .into_iter()
        .next()
        .map(|challenge| challenge.token_challenge().clone())
        .ok_or_else(|| "no PrivateToken challenge in WWW-Authenticate".to_owned())
}

/// The `Authorization` value presenting one pass.
pub fn authorization_for(token: &PublicToken) -> Result<String, String> {
    let (_, value) = build_authorization_header(token).map_err(|error| format!("{error:?}"))?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|error| error.to_string())
}

/// The wallet for one Anchor: `<home>/passes/<anchor>.json`, owner-only,
/// holding the passes obtained from it and its key as last read.
///
/// A stolen wallet file yields passes for that Anchor's keys and no
/// identity: a pass names no operator, reader or session, and the file
/// holds no credential. Each pass is bound to one source's challenge.
///
/// The wallet holds an exclusive lock beside the file for its lifetime, so
/// two refills cannot each write back a wallet missing the other's passes,
/// and one pass cannot be taken twice.
pub struct Wallet {
    path: PathBuf,
    held: WalletFile,
    _lock: File,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletFile {
    anchor: String,
    /// The Anchor's key as the last refill read it, so a pass can be
    /// verified offline, and its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_key: Option<String>,
    #[serde(default)]
    passes: Vec<HeldPass>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeldPass {
    /// The origin information of the challenge the pass answers: the
    /// source hosts it may be spent at.
    origin_info: Vec<String>,
    /// The token (RFC 9577 §2.2), base64url.
    token: String,
}

/// The wallet file's name for an Anchor origin: the origin without
/// `https://`, every character that is not a letter, digit, `.` or `-`
/// written as `_`. The file also records its Anchor, which is checked at
/// open, so two origins that spell one name cannot share passes.
pub fn wallet_path(home: &Path, anchor_origin: &str) -> PathBuf {
    let stem: String = anchor_origin
        .strip_prefix("https://")
        .unwrap_or(anchor_origin)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    home.join("passes").join(format!("{stem}.json"))
}

impl Wallet {
    /// Open the Anchor's wallet, creating `<home>/passes` owner-only where
    /// it is not there, and hold its lock until the wallet is dropped.
    pub fn open(home: &Path, anchor_origin: &str) -> Result<Self, String> {
        let path = wallet_path(home, anchor_origin);
        let directory = path.parent().expect("a wallet path has a directory");
        crate::fetched_file::private_directory(directory)?;
        let lock_path = path.with_extension("lock");
        let lock = {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(false).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            options
                .open(&lock_path)
                .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?
        };
        lock.lock()
            .map_err(|error| format!("cannot lock {}: {error}", lock_path.display()))?;
        let held = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<WalletFile>(&bytes)
                .map_err(|error| format!("{} is not a pass wallet: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => WalletFile {
                anchor: anchor_origin.to_owned(),
                key_id: None,
                token_key: None,
                passes: Vec::new(),
            },
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        if held.anchor != anchor_origin {
            return Err(format!(
                "{} holds passes from {}, not {anchor_origin}",
                path.display(),
                held.anchor
            ));
        }
        Ok(Self {
            path,
            held,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The Anchor's key id as the last refill read it.
    pub fn key_id(&self) -> Option<&str> {
        self.held.key_id.as_deref()
    }

    /// How many passes answer `challenge`.
    pub fn held(&self, challenge: &TokenChallenge) -> Result<usize, String> {
        let digest = digest_of(challenge)?;
        Ok(self
            .tokens()?
            .iter()
            .filter(|(_, token)| token.challenge_digest() == &digest)
            .count())
    }

    /// Remove and return one pass answering `challenge`, the wallet written
    /// back before it is returned so that it is never presented twice.
    pub fn take(&mut self, challenge: &TokenChallenge) -> Result<Option<PublicToken>, String> {
        let digest = digest_of(challenge)?;
        self.take_where(|token| token.challenge_digest() == &digest)
    }

    /// Remove and return one pass answering a source's challenge: issued
    /// for its challenge and under the key it names. A pass under another
    /// of the Anchor's keys would not verify there, so it is not spent on
    /// it.
    pub fn take_answering(
        &mut self,
        source: &SourceChallenge,
    ) -> Result<Option<PublicToken>, String> {
        let digest = digest_of(&source.challenge)?;
        self.take_where(|token| {
            token.challenge_digest() == &digest && token.token_key_id() == &source.key_id
        })
    }

    /// How many passes are held for a source's challenge under a key other
    /// than the one it names: passes from before the Anchor rotated its
    /// key, which the next refill discards.
    pub fn held_under_other_keys(&self, source: &SourceChallenge) -> Result<usize, String> {
        let digest = digest_of(&source.challenge)?;
        Ok(self
            .tokens()?
            .iter()
            .filter(|(_, token)| {
                token.challenge_digest() == &digest && token.token_key_id() != &source.key_id
            })
            .count())
    }

    fn take_where(
        &mut self,
        answers: impl Fn(&PublicToken) -> bool,
    ) -> Result<Option<PublicToken>, String> {
        let Some(index) = self.tokens()?.iter().position(|(_, token)| answers(token)) else {
            return Ok(None);
        };
        let held = self.held.passes.remove(index);
        self.save()?;
        decode_token(&held.token).map(Some)
    }

    /// Obtain up to `count` passes answering `challenge` from the Anchor,
    /// presenting `credential`, and keep each in the wallet.
    ///
    /// Stops at the first request the Anchor does not answer with a pass.
    /// A refusal is kept as the Anchor stated it, because the ration is the
    /// Anchor's to state and this edge has no ration of its own to compare
    /// it with. Every pass obtained before the stop is kept. An error is
    /// returned only where nothing was asked of the Anchor's issuance: the
    /// directory could not be read.
    pub fn refill<R: CryptoRng>(
        &mut self,
        credential: &str,
        challenge: &TokenChallenge,
        count: usize,
        rng: &mut R,
    ) -> Result<Refill, String> {
        let key = directory(&self.held.anchor)?;
        // Passes under a key the Anchor's directory does not list verify nowhere
        // that has followed the rotation, and cannot be checked against
        // the key the wallet holds, so they are not kept beside it.
        let before = self.held.passes.len();
        let current = key.key_id;
        let mut kept = Vec::with_capacity(before);
        for held in std::mem::take(&mut self.held.passes) {
            if decode_token(&held.token)?.token_key_id() == &current {
                kept.push(held);
            }
        }
        self.held.passes = kept;
        let discarded = before - self.held.passes.len();
        self.held.key_id = Some(key.key_id());
        self.held.token_key = Some(URL_SAFE_NO_PAD.encode(&key.token_key));
        let mut obtained = 0;
        let mut stopped = None;
        for _ in 0..count {
            match issue(&key, credential, challenge, rng) {
                Ok(token) => {
                    let mut encoded = Vec::new();
                    token
                        .tls_serialize(&mut encoded)
                        .map_err(|error| format!("{error:?}"))?;
                    self.held.passes.push(HeldPass {
                        origin_info: challenge.origin_info(),
                        token: URL_SAFE_NO_PAD.encode(encoded),
                    });
                    obtained += 1;
                }
                Err(stop) => {
                    stopped = Some(stop.redacting(credential));
                    break;
                }
            }
        }
        self.save()?;
        Ok(Refill {
            anchor: self.held.anchor.clone(),
            origin_info: challenge.origin_info(),
            requested: count,
            obtained,
            key_id: key.key_id(),
            held: self.held(challenge)?,
            discarded,
            stopped,
        })
    }

    fn tokens(&self) -> Result<Vec<(usize, PublicToken)>, String> {
        self.held
            .passes
            .iter()
            .enumerate()
            .map(|(index, held)| decode_token(&held.token).map(|token| (index, token)))
            .collect()
    }

    fn save(&self) -> Result<(), String> {
        let encoded = serde_json::to_vec_pretty(&self.held)
            .map_err(|error| format!("cannot serialise the wallet: {error}"))?;
        crate::declaration::replace_private(&self.path, &encoded)
    }
}

/// The random source a production refill blinds with: the operating
/// system's, through `rand`'s thread-local generator.
pub fn rng() -> impl CryptoRng {
    blind_rsa_signatures::reexports::rand::rng()
}

fn digest_of(challenge: &TokenChallenge) -> Result<[u8; 32], String> {
    challenge.digest().map_err(|error| format!("{error:?}"))
}

fn decode_token(encoded: &str) -> Result<PublicToken, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("a held pass is not base64url: {error}"))?;
    PublicToken::tls_deserialize(&mut bytes.as_slice())
        .map_err(|error| format!("a held pass is not a token: {error:?}"))
}

/// One token request: blind, ask, finalise. Finalising verifies the
/// Anchor's signature with its key, so a pass that would not verify is
/// never held.
fn issue<R: CryptoRng>(
    key: &AnchorKey,
    credential: &str,
    challenge: &TokenChallenge,
    rng: &mut R,
) -> Result<PublicToken, Stopped> {
    let (token_request, state) = TokenRequest::new(rng, key.public_key.clone(), challenge)
        .map_err(|error| Stopped::Failed(format!("blinding the request: {error:?}")))?;
    let mut body = Vec::new();
    token_request
        .tls_serialize(&mut body)
        .map_err(|error| Stopped::Failed(format!("{error:?}")))?;
    let mut request = Request::post(&key.request_uri, body, TOKEN_REQUEST_TYPE);
    request.headers.set("Accept", TOKEN_RESPONSE_TYPE);
    request
        .headers
        .set("Authorization", &format!("Bearer {credential}"));
    let response =
        send(&key.request_uri, request).map_err(|error| Stopped::Failed(format!("{error:#}")))?;
    if response.status != 200 {
        return Err(Stopped::refused(&response));
    }
    let content_type = response.headers.get("content-type").unwrap_or_default();
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case(TOKEN_RESPONSE_TYPE) {
        // Named by its top-level type, not quoted: the value is the
        // Anchor's and the reason is kept.
        return Err(Stopped::Failed(format!(
            "the anchor answered 200 with {}, not {TOKEN_RESPONSE_TYPE}",
            commonmeasure_http::media_type_named(content_type)
        )));
    }
    let token_response = TokenResponse::tls_deserialize(&mut response.body.as_slice())
        .map_err(|error| Stopped::Failed(format!("the token response: {error:?}")))?;
    token_response
        .issue_token(&state)
        .map_err(|error| Stopped::Failed(format!("the anchor's signature: {error:?}")))
}

/// Why a refill stopped short of the count asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Stopped {
    /// The Anchor answered a token request with something other than a
    /// pass: its status, `Retry-After` as sent, and its body as it stated
    /// it (JSON where it is JSON, otherwise text), cut at 2 KiB.
    Refused {
        status: u16,
        retry_after: Option<String>,
        stated: Value,
    },
    /// The exchange failed, or the Anchor's answer was not a pass that
    /// verifies.
    Failed(String),
}

impl Stopped {
    fn refused(response: &Response) -> Self {
        let body = &response.body[..response.body.len().min(STATED_LIMIT)];
        let stated = serde_json::from_slice::<Value>(body)
            .unwrap_or_else(|_| json!(String::from_utf8_lossy(body)));
        Stopped::Refused {
            status: response.status,
            retry_after: response.headers.get("retry-after").map(str::to_owned),
            stated,
        }
    }

    /// The same, with any echo of the credential replaced, before it is
    /// kept anywhere.
    fn redacting(self, credential: &str) -> Self {
        if credential.is_empty() {
            return self;
        }
        let redact = |text: &str| text.replace(credential, "[credential]");
        match self {
            Stopped::Refused {
                status,
                retry_after,
                stated,
            } => {
                let stated = serde_json::from_str(&redact(&stated.to_string())).unwrap_or(stated);
                Stopped::Refused {
                    status,
                    retry_after: retry_after.as_deref().map(redact),
                    stated,
                }
            }
            Stopped::Failed(reason) => Stopped::Failed(redact(&reason)),
        }
    }

    fn record(&self) -> Value {
        match self {
            Stopped::Refused {
                status,
                retry_after,
                stated,
            } => json!({"status": status, "retry_after": retry_after, "stated": stated}),
            Stopped::Failed(reason) => json!({"failed": reason}),
        }
    }
}

/// What one refill did, for the `passes_refilled` record and the operator.
#[derive(Debug, Clone, PartialEq)]
pub struct Refill {
    pub anchor: String,
    pub origin_info: Vec<String>,
    pub requested: usize,
    pub obtained: usize,
    pub key_id: String,
    /// Passes the wallet holds for the challenge afterwards.
    pub held: usize,
    /// Passes dropped from the wallet, for any host, because they were
    /// issued under a key the Anchor's directory does not list.
    pub discarded: usize,
    pub stopped: Option<Stopped>,
}

impl Refill {
    /// The `passes_refilled` payload: the Anchor by origin, the counts and
    /// the key id. Never a pass, a blinded request or the credential.
    pub fn record(&self) -> Value {
        json!({
            "anchor": self.anchor,
            "origin_info": self.origin_info,
            "requested": self.requested,
            "obtained": self.obtained,
            "held": self.held,
            "discarded": self.discarded,
            "key_id": self.key_id,
            "stopped": self.stopped.as_ref().map(Stopped::record),
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wallet_is_named_after_its_anchor() {
        let home = Path::new("/h");
        assert_eq!(
            wallet_path(home, "https://anchor.example"),
            Path::new("/h/passes/anchor.example.json")
        );
        assert_eq!(
            wallet_path(home, "https://anchor.example:8443"),
            Path::new("/h/passes/anchor.example_8443.json")
        );
        assert_eq!(
            wallet_path(home, "http://127.0.0.1:41234"),
            Path::new("/h/passes/http___127.0.0.1_41234.json")
        );
    }

    #[test]
    fn a_pass_is_held_for_the_host_it_names_under_the_anchor_host() {
        let challenge = challenge_for("https://anchor.example:8443", "News.Example.").unwrap();
        assert_eq!(challenge.issuer_name(), "anchor.example");
        assert_eq!(challenge.origin_info(), ["news.example"]);
        assert!(challenge.redemption_context().is_none());
        assert!(challenge_for("https://anchor.example", "").is_err());
    }

    /// An Anchor's refusal is kept as stated, and an echoed credential is
    /// not kept at all.
    #[test]
    fn a_refusal_is_kept_as_stated_without_the_credential() {
        let mut response = Response::new(
            429,
            br#"{"error":"ration exhausted for secret-1","ration_per_window":3}"#.to_vec(),
        );
        response.headers.set("Retry-After", "3600");
        let stopped = Stopped::refused(&response).redacting("secret-1");
        assert_eq!(
            stopped.record(),
            json!({"status": 429, "retry_after": "3600", "stated": {
                "error": "ration exhausted for [credential]", "ration_per_window": 3}})
        );
        let text = Stopped::refused(&Response::new(401, b"no".to_vec()));
        assert_eq!(text.record()["stated"], "no");
    }

    /// Another scheme is no challenge to answer; a `PrivateToken` value
    /// that does not parse is an error the refusal names, never a pass.
    #[test]
    fn only_a_private_token_challenge_is_read_and_one_that_does_not_parse_is_an_error() {
        assert!(
            source_challenges(r#"Bearer realm="news""#)
                .unwrap()
                .is_empty()
        );
        assert!(source_challenges("PrivateToken challenge=!!, token-key=??").is_err());
        assert!(source_challenges("privatetoken token-key=AAAA").is_err());
    }

    #[test]
    fn a_wallet_refuses_a_file_held_for_another_anchor() {
        let home = tempfile::tempdir().unwrap();
        let path = wallet_path(home.path(), "https://a.example");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"anchor":"https://b.example","passes":[]}"#).unwrap();
        let error = Wallet::open(home.path(), "https://a.example")
            .map(|_| ())
            .unwrap_err();
        assert!(
            error.contains("holds passes from https://b.example"),
            "{error}"
        );
    }
}
