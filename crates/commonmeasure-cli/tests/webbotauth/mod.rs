//! Verifying a signed request the way the party on the other end does.
//!
//! This is the verifier's half of `commonmeasure_harness::identity`, written
//! from the drafts rather than from that module, so a loopback publisher and a
//! loopback hub in these tests admit a request on the same terms Cloudflare
//! and the Common Measure Hub do: resolve `keyid` in the directory the
//! `Signature-Agent` header names, rebuild the RFC 9421 signature base from
//! the components the request says are covered, and check the Ed25519
//! signature over it.
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

/// The public halves a verifier holds, by key id, and the URL this directory
/// is served at: what a verifier fetches and what it finds there.
pub struct Directory(HashMap<String, VerifyingKey>, String);

impl Default for Directory {
    fn default() -> Self {
        Self(
            HashMap::new(),
            format!("https://hub.example{DIRECTORY_PATH}"),
        )
    }
}

impl Directory {
    /// Publish a key under its RFC 7638 thumbprint, computed here rather than
    /// taken on trust, so a signature can only verify under the id the
    /// directory would really list it as.
    pub fn publish(&mut self, public_key: &[u8; 32]) -> String {
        let x = URL_SAFE_NO_PAD.encode(public_key);
        let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
        let key_id =
            URL_SAFE_NO_PAD.encode(<sha2::Sha256 as sha2::Digest>::digest(canonical.as_bytes()));
        self.0.insert(
            key_id.clone(),
            VerifyingKey::from_bytes(public_key).expect("a valid Ed25519 point"),
        );
        key_id
    }

    pub fn forget(&mut self, key_id: &str) {
        self.0.remove(key_id);
    }
}

/// Verify `request`, which reached `authority`.
///
/// Every refusal names what failed, because a test asserting "the request was
/// refused" without knowing why would pass on a signature that was never sent.
pub fn verify(
    request: &Request,
    authority: &str,
    directory: &Directory,
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
    if resolved != directory.1 {
        return Err(format!(
            "Signature-Agent resolves to {resolved}, which is not this directory ({})",
            directory.1
        ));
    }

    let key = directory.0.get(&key_id).ok_or_else(|| {
        format!("keyid {key_id} is not in the directory; a revoked key is not published")
    })?;

    let mut base = String::new();
    for name in &covered {
        let value = if name == "@authority" {
            authority.to_owned()
        } else {
            request
                .headers
                .get(name)
                .ok_or_else(|| format!("the signature covers {name}, which the request omits"))?
                .to_owned()
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

/// Enrol `home` as `commonmeasure connect` does: mint the key, store it, and
/// write the enrolment record naming where the hub publishes it. Publishes the
/// public half in `directory` and returns the key id.
///
/// The files are written through the runtime's own types, so a test can never
/// enrol an edge into a shape the binary would not itself produce.
pub fn enrol(home: &std::path::Path, hub: &str, origin: &str, published: &mut Directory) -> String {
    use commonmeasure_harness::enrolment::{
        EnrolledIdentity, EnrolledOrganization, EnrolmentRecord,
    };
    use commonmeasure_harness::identity::EdgeKey;

    let key = EdgeKey::generate().expect("a key");
    key.store(home).expect("store the key");
    let public: [u8; 32] = URL_SAFE_NO_PAD
        .decode(key.jwk_x())
        .expect("the public key")
        .try_into()
        .expect("32 bytes");
    let key_id = published.publish(&public);
    assert_eq!(key_id, key.thumbprint(), "the key id is the thumbprint");

    EnrolmentRecord {
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
    }
    .store(home)
    .expect("store the enrolment record");
    key_id
}

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
