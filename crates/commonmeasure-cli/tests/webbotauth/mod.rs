//! Verifying a signed request the way the party on the other end does.
//!
//! This is the verifier's half of `commonmeasure_harness::identity`, written
//! from the drafts rather than from that module, so a loopback publisher and a
//! loopback hub in these tests admit a request on the same terms Cloudflare
//! and the Common Measure Hub do: resolve `keyid` in the directory the
//! `Signature-Agent` header names, use the key only when the directory
//! response carries a current signature by that key over its own authority
//! (Cloudflare's per-key rule), rebuild the RFC 9421 signature base from the
//! components the request says are covered, and check the Ed25519 signature
//! over it.
//!
//! It lives in the tests because nothing the edge ships verifies its own
//! signatures. The hub's verifier is the other implementation, and the pinned
//! vector in `commonmeasure_harness::identity` is what keeps the two agreeing
//! without ever running in one process.

// Each integration test binary compiles this module separately, so anything
// only one of them uses is dead code in the others.
#![allow(dead_code)]

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use commonmeasure_http::Request;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// What a verified request establishes: who signed it, and over what.
pub struct Verified {
    pub key_id: String,
    /// The covered component identifiers, in the order they were signed.
    pub covered: Vec<String>,
}

impl Verified {
    pub fn covers(&self, component: &str) -> bool {
        self.covered.iter().any(|name| name == component)
    }
}

/// Where a key directory is served under the origin `Signature-Agent` names.
pub const DIRECTORY_PATH: &str = "/.well-known/http-message-signatures-directory";

/// The tag a signature on a key directory response carries.
pub const DIRECTORY_TAG: &str = "http-message-signatures-directory";

/// A key directory as a hub serves it: the listed keys in order, each with
/// the proof its holder signed, and the URL it is served at. What a verifier
/// fetches is [`Directory::response`], and what it may use from that is
/// [`usable_keys`], so a key that is published without a proof is in the
/// body and still verifies nothing.
pub struct Directory {
    listed: Vec<Listed>,
    url: String,
}

struct Listed {
    key_id: String,
    x: String,
    /// The `@signature-params` its holder signed, verbatim, and the
    /// signature, standard base64. `None` for a key listed with no proof.
    proof: Option<(String, String)>,
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            listed: Vec::new(),
            url: format!("https://hub.example{DIRECTORY_PATH}"),
        }
    }
}

/// The RFC 7638 thumbprint of an Ed25519 public key, computed here rather
/// than taken on trust, so a signature can only verify under the id the
/// directory would really list it as.
pub fn thumbprint(public_key: &[u8; 32]) -> String {
    let x = URL_SAFE_NO_PAD.encode(public_key);
    let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
    URL_SAFE_NO_PAD.encode(<sha2::Sha256 as sha2::Digest>::digest(canonical.as_bytes()))
}

impl Directory {
    /// List a key with the directory proof its holder signed: the
    /// `Signature-Input` member's parameters verbatim and the signature.
    /// Returns the key id.
    pub fn publish(
        &mut self,
        public_key: &[u8; 32],
        signature_input: &str,
        signature: &str,
    ) -> String {
        self.list(
            public_key,
            Some((signature_input.to_owned(), signature.to_owned())),
        )
    }

    /// List a key with no proof, as a directory that predates the per-key
    /// rule does.
    pub fn publish_unsigned(&mut self, public_key: &[u8; 32]) -> String {
        self.list(public_key, None)
    }

    fn list(&mut self, public_key: &[u8; 32], proof: Option<(String, String)>) -> String {
        let key_id = thumbprint(public_key);
        self.forget(&key_id);
        self.listed.push(Listed {
            key_id: key_id.clone(),
            x: URL_SAFE_NO_PAD.encode(public_key),
            proof,
        });
        key_id
    }

    pub fn forget(&mut self, key_id: &str) {
        self.listed.retain(|listed| listed.key_id != key_id);
    }

    /// The authority the directory is served from.
    pub fn authority(&self) -> String {
        self.url
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .expect("an absolute URL")
            .to_owned()
    }

    /// The response a verifier fetching this directory receives: the JWK set
    /// and, when any key carries a proof, `Signature-Input` and `Signature`
    /// with one member per such key, labelled at serving time.
    pub fn response(&self) -> (String, Option<String>, Option<String>) {
        let keys = self
            .listed
            .iter()
            .map(|listed| {
                format!(
                    r#"{{"kty":"OKP","crv":"Ed25519","x":"{}","kid":"{}"}}"#,
                    listed.x, listed.key_id
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(r#"{{"keys":[{keys}]}}"#);
        let members: Vec<(String, String)> = self
            .listed
            .iter()
            .filter_map(|listed| listed.proof.clone())
            .collect();
        if members.is_empty() {
            return (body, None, None);
        }
        let input = members
            .iter()
            .enumerate()
            .map(|(index, (params, _))| format!("binding{index}={params}"))
            .collect::<Vec<_>>()
            .join(", ");
        let signature = members
            .iter()
            .enumerate()
            .map(|(index, (_, signature))| format!("binding{index}=:{signature}:"))
            .collect::<Vec<_>>()
            .join(", ");
        (body, Some(input), Some(signature))
    }
}

/// The keys a verifier applying Cloudflare's per-key rule may use from one
/// directory response served from `authority`: a listed key is usable only
/// when a `Signature-Input` member names it as `keyid`, covers exactly
/// `("@authority";req)`, carries the directory tag and Ed25519, is valid at
/// `now`, and its signature verifies under that key over the base rebuilt
/// from the served parameters. Every listed key that is not usable is
/// returned with the reason, so a test can say why a key was refused.
pub fn usable_keys(
    body: &str,
    signature_input: Option<&str>,
    signature: Option<&str>,
    authority: &str,
    now: i64,
) -> Result<UsableKeys, String> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("the directory is not JSON: {error}"))?;
    let mut listed = HashMap::new();
    for key in parsed["keys"]
        .as_array()
        .ok_or("the directory has no keys array")?
    {
        let (Some(kid), Some(x)) = (key["kid"].as_str(), key["x"].as_str()) else {
            return Err(format!("a listed key has no kid or x: {key}"));
        };
        let raw: [u8; 32] = URL_SAFE_NO_PAD
            .decode(x)
            .map_err(|error| format!("x is not base64url: {error}"))?
            .try_into()
            .map_err(|_| "x is not 32 bytes".to_owned())?;
        if thumbprint(&raw) != kid {
            return Err(format!("kid {kid} is not the thumbprint of its key"));
        }
        listed.insert(
            kid.to_owned(),
            VerifyingKey::from_bytes(&raw).map_err(|_| format!("{kid} is not an Ed25519 point"))?,
        );
    }

    let members = |header: Option<&str>| -> Result<HashMap<String, String>, String> {
        let Some(header) = header else {
            return Ok(HashMap::new());
        };
        header
            .split(", ")
            .map(|member| {
                member
                    .split_once('=')
                    .map(|(label, value)| (label.to_owned(), value.to_owned()))
                    .ok_or_else(|| format!("a member is not label=value: {member}"))
            })
            .collect()
    };
    let inputs = members(signature_input)?;
    let signatures = members(signature)?;
    if inputs.len() != signatures.len() {
        return Err(format!(
            "Signature-Input has {} members and Signature has {}",
            inputs.len(),
            signatures.len()
        ));
    }

    let mut usable = HashMap::new();
    let mut refused: HashMap<String, String> = listed
        .keys()
        .map(|kid| {
            (
                kid.clone(),
                "no Signature-Input member names this key".to_owned(),
            )
        })
        .collect();
    for (label, params) in &inputs {
        let Some(kid) = parameter(params, "keyid") else {
            continue;
        };
        let Some(key) = listed.get(&kid) else {
            continue;
        };
        let checked = (|| -> Result<(), String> {
            if !params.starts_with(r#"("@authority";req);"#) {
                return Err(format!(
                    "the member does not cover exactly (\"@authority\";req): {params}"
                ));
            }
            if !params.contains(&format!(r#";tag="{DIRECTORY_TAG}""#)) {
                return Err(format!(
                    "the member is not tagged {DIRECTORY_TAG}: {params}"
                ));
            }
            if !params.contains(r#";alg="ed25519""#) {
                return Err(format!("the member is not Ed25519: {params}"));
            }
            let created = integer(params, "created").ok_or("the member has no created")?;
            let expires = integer(params, "expires").ok_or("the member has no expires")?;
            if created > now || expires <= now {
                return Err(format!(
                    "the member is not valid now ({created} to {expires})"
                ));
            }
            let encoded = signatures
                .get(label)
                .ok_or_else(|| format!("Signature has no member {label}"))?;
            let bytes: [u8; 64] = STANDARD
                .decode(encoded.trim_matches(':'))
                .map_err(|error| format!("the signature is not base64: {error}"))?
                .try_into()
                .map_err(|_| "an Ed25519 signature is 64 bytes".to_owned())?;
            let base = format!("\"@authority\";req: {authority}\n\"@signature-params\": {params}");
            key.verify(base.as_bytes(), &Signature::from_bytes(&bytes))
                .map_err(|_| format!("the signature does not verify over the base:\n{base}"))
        })();
        match checked {
            Ok(()) => {
                refused.remove(&kid);
                usable.insert(kid, *key);
            }
            Err(reason) if !usable.contains_key(&kid) => {
                refused.insert(kid, reason);
            }
            Err(_) => {}
        }
    }
    Ok(UsableKeys {
        keys: usable,
        refused,
    })
}

/// What one directory response lets a verifier use.
pub struct UsableKeys {
    /// The keys whose holders signed their place, by key id.
    pub keys: HashMap<String, VerifyingKey>,
    /// Every other listed key, by key id, with why it is not usable.
    pub refused: HashMap<String, String>,
}

/// One `;name=integer` signature parameter.
fn integer(params: &str, name: &str) -> Option<i64> {
    let at = params.find(&format!(";{name}="))? + name.len() + 2;
    let rest = &params[at..];
    rest[..rest.find(';').unwrap_or(rest.len())].parse().ok()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs() as i64
}

/// Verify `request`, which reached `authority`.
///
/// Every refusal names what failed, because a test asserting "the request was
/// refused" without knowing why would pass on a signature that was never sent.
///
/// `@authority` is the one derived component this resolves. Any other covered
/// name is looked up as a header and refused when absent, which is how a hub
/// before the custody release signature reads `@method` and `@path`. A
/// publisher's verifier may resolve them; the contract forbids the edge to
/// cover them for one all the same. The policy route and the loopback
/// publishers verify through here, so a policy or crossing request that
/// covered either fails these tests.
pub fn verify(
    request: &Request,
    authority: &str,
    directory: &Directory,
) -> Result<Verified, String> {
    verify_resolving(request, authority, directory, false)
}

/// [`verify`] for the supplier credential release route
/// (`docs/contracts/supplier-credentials.md` §Signature components of the
/// release request): the signature must cover `@method` and `@path`, both
/// resolved from the request as it arrived; its window is at most 360 seconds
/// and its nonce at least 22 characters.
pub fn verify_release(
    request: &Request,
    authority: &str,
    directory: &Directory,
) -> Result<Verified, String> {
    verify_resolving(request, authority, directory, true)
}

fn verify_resolving(
    request: &Request,
    authority: &str,
    directory: &Directory,
    release: bool,
) -> Result<Verified, String> {
    let input = request
        .headers
        .get("Signature-Input")
        .ok_or("the request carries no Signature-Input")?;
    let signature = request
        .headers
        .get("Signature")
        .ok_or("the request carries no Signature")?;

    // `<label>=<params>`, where the params text is what was signed verbatim:
    // rebuilding it from parsed pieces would verify this parser rather than
    // the sender's bytes.
    let (label, params) = input
        .split_once('=')
        .ok_or_else(|| format!("Signature-Input is not label=params: {input}"))?;
    let (signature_label, encoded) = signature
        .split_once('=')
        .ok_or_else(|| format!("Signature is not label=value: {signature}"))?;
    if label != signature_label {
        return Err(format!(
            "Signature-Input names {label} and Signature names {signature_label}"
        ));
    }

    let listed = params
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .map(|(inside, _)| inside)
        .ok_or_else(|| format!("no component list in {params}"))?;
    let covered: Vec<String> = listed
        .split_whitespace()
        .map(|name| name.trim_matches('"').to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    if !covered.iter().any(|name| name == "@authority") {
        return Err("the signature does not cover @authority".to_owned());
    }
    if release {
        for required in ["@method", "@path"] {
            if !covered.iter().any(|name| name == required) {
                return Err(format!(
                    "components_missing: the signature does not cover {required}"
                ));
            }
        }
    }

    if !params.contains(r#"tag="web-bot-auth""#) {
        return Err(format!(
            "the signature is not tagged web-bot-auth: {params}"
        ));
    }
    if !params.contains(r#"alg="ed25519""#) {
        return Err(format!("the signature is not Ed25519: {params}"));
    }
    let key_id = parameter(params, "keyid")
        .ok_or_else(|| format!("the signature names no keyid: {params}"))?;
    if release {
        // The numbers the contract pins for this route, as the relay's
        // release double holds them.
        let (created, expires) = integer(params, "created")
            .zip(integer(params, "expires"))
            .ok_or_else(|| format!("the signature names no window: {params}"))?;
        if expires - created > 360 {
            return Err(format!(
                "window_too_long: the window is {} seconds and the cap is 360",
                expires - created
            ));
        }
        let nonce = parameter(params, "nonce")
            .ok_or_else(|| format!("the signature names no nonce: {params}"))?;
        if nonce.len() < 22 {
            return Err(format!(
                "the nonce is {} characters and the minimum is 22",
                nonce.len()
            ));
        }
    }

    // Resolve the directory the way a verifier does: `Signature-Agent` names
    // an origin, and the well-known path is appended to it
    // (draft-meunier-webbotauth-httpsig-protocol §4). A header carrying a
    // full directory URL would send the verifier to that path twice.
    let agent = request
        .headers
        .get("Signature-Agent")
        .ok_or("the request carries no Signature-Agent")?
        .trim_matches('"');
    if agent.trim_end_matches('/').contains("/.well-known/") {
        return Err(format!(
            "Signature-Agent must name an origin, not a directory URL: {agent}"
        ));
    }
    let resolved = format!("{}{DIRECTORY_PATH}", agent.trim_end_matches('/'));
    if resolved != directory.url {
        return Err(format!(
            "Signature-Agent resolves to {resolved}, which is not this directory ({})",
            directory.url
        ));
    }

    // Fetch the directory and keep only the keys whose holders signed their
    // place in it, as Cloudflare does: a listed key without a proof is not
    // used.
    let (body, served_input, served_signature) = directory.response();
    let UsableKeys {
        keys: usable,
        refused,
    } = usable_keys(
        &body,
        served_input.as_deref(),
        served_signature.as_deref(),
        &directory.authority(),
        unix_now(),
    )?;
    let key = usable
        .get(&key_id)
        .ok_or_else(|| match refused.get(&key_id) {
            Some(reason) => format!("keyid {key_id} is listed but not usable: {reason}"),
            None => {
                format!("keyid {key_id} is not in the directory; a revoked key is not published")
            }
        })?;

    let mut base = String::new();
    for name in &covered {
        let value = match name.as_str() {
            "@authority" => authority.to_owned(),
            "@method" if release => request.method.clone(),
            "@path" if release => request
                .target
                .split_once('?')
                .map_or(request.target.as_str(), |(path, _)| path)
                .to_owned(),
            header => request
                .headers
                .get(header)
                .ok_or_else(|| format!("the signature covers {name}, which the request omits"))?
                .to_owned(),
        };
        base.push_str(&format!("\"{name}\": {value}\n"));
    }
    base.push_str(&format!("\"@signature-params\": {params}"));

    let raw = encoded.trim().trim_matches(':');
    let bytes: [u8; 64] = STANDARD
        .decode(raw)
        .map_err(|error| format!("the signature is not base64: {error}"))?
        .try_into()
        .map_err(|_| "an Ed25519 signature is 64 bytes".to_owned())?;
    key.verify(base.as_bytes(), &Signature::from_bytes(&bytes))
        .map_err(|_| format!("the signature does not verify over the base:\n{base}"))?;

    Ok(Verified { key_id, covered })
}

/// One `;name="value"` signature parameter.
fn parameter(params: &str, name: &str) -> Option<String> {
    let at = params.find(&format!(";{name}=\""))? + name.len() + 3;
    let rest = &params[at..];
    Some(rest[..rest.find('"')?].to_owned())
}

/// Enrol `home` as `commonmeasure connect` does: mint the key, store it,
/// write the enrolment record naming where the hub publishes it, and sign the
/// directory proof. Publishes the public half with its proof in `directory`
/// and returns the key id.
///
/// The files are written through the runtime's own types, so a test can never
/// enrol an edge into a shape the binary would not itself produce.
pub fn enrol(home: &std::path::Path, hub: &str, origin: &str, published: &mut Directory) -> String {
    use commonmeasure_harness::enrolment::{
        DirectoryListing, EnrolledIdentity, EnrolledOrganization, EnrolmentRecord, ProofStatement,
        timestamp,
    };
    use commonmeasure_harness::identity::{EdgeKey, Identity};

    let key = EdgeKey::generate().expect("a key");
    key.store(home).expect("store the key");
    let public: [u8; 32] = URL_SAFE_NO_PAD
        .decode(key.jwk_x())
        .expect("the public key")
        .try_into()
        .expect("32 bytes");
    let key_id = key.thumbprint();
    assert_eq!(key_id, thumbprint(&public), "the key id is the thumbprint");

    let record = EnrolmentRecord {
        hub: hub.to_owned(),
        organization: EnrolledOrganization {
            id: "org-1".to_owned(),
            name: "Org A Media".to_owned(),
        },
        name: "laptop-7".to_owned(),
        key_id: key_id.clone(),
        identity: EnrolledIdentity {
            origin: origin.to_owned(),
            bot_page: format!("{origin}/bot"),
            contact: Some("mailto:bot@hub.example".to_owned()),
        },
        enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    };
    record.store(home).expect("store the enrolment record");

    // The proof the edge uploads at `connect`, made by the edge's own signer,
    // and the listing `connect` leaves once the hub holds it.
    let identity = Identity::load(home).expect("the enrolled identity");
    let signer = identity.signer().expect("an enrolled key signs");
    let authority = published.authority();
    let proof = signer
        .directory_proof(&authority, unix_now(), LIFETIME_SECS)
        .expect("sign the directory proof");
    published.publish(&public, &proof.signature_input, &proof.signature);
    DirectoryListing {
        key_id: key_id.clone(),
        last_uploaded_release: Some(commonmeasure_harness::enrolment::RELEASE.to_owned()),
        checked_at: timestamp(chrono::Utc::now()),
        stated: Some(ProofStatement {
            authority,
            lifetime_secs: LIFETIME_SECS,
            expires_at: chrono::DateTime::from_timestamp(proof.expires, 0).map(timestamp),
            listed: None,
            unlisted_reason: None,
        }),
        failure: None,
    }
    .store(home)
    .expect("store the directory listing");
    key_id
}

/// The proof lifetime a hub states by default: seven days.
pub const LIFETIME_SECS: i64 = 604_800;

/// Record the revocation the edge learns on its next relay run, and take the
/// key out of the directory as the hub does.
pub fn revoke(home: &std::path::Path, key_id: &str, published: &mut Directory) {
    use commonmeasure_harness::enrolment::EnrolmentRecord;
    let mut record = EnrolmentRecord::load(home)
        .expect("readable")
        .expect("enrolled");
    record.revoked_at = Some("2026-09-06T01:00:00.000Z".to_owned());
    record.revocation = Some("owner".to_owned());
    record.revocation_learnt_at = Some("2026-09-06T01:00:05.000Z".to_owned());
    record.store(home).expect("store");
    published.forget(key_id);
}
