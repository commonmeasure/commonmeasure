//! The edge's network identity: the Ed25519 key minted at enrolment and the
//! HTTP message signature the requests it decides for itself carry.
//!
//! One signing implementation serves both directions. Towards a publisher the
//! mediated fetch and the `robots.txt`, licence and manifest probes beside it
//! sign, so the site can verify who fetched; towards the hub the policy fetch
//! signs, so the hub can authenticate the edge with no second credential
//! (`DECISIONS.md` §Delegated authority and fleet management). Both are RFC
//! 9421 signatures under the Web Bot Auth profile, over the same components
//! with the same parameters, so a publisher and the hub verify by one rule.
//!
//! Three request paths do not sign, because none of them is a fetch a
//! publisher rules on: telemetry delivery and enrolment reach the operator's
//! own hub under its ingest key, and a supply adapter reaches a supplier's API
//! under a credential the operator holds with that supplier.
//!
//! The private key never leaves `<home>/edge-key.json`. What a verifier needs
//! is public: the key id, which is the key's RFC 7638 thumbprint, and the
//! origin serving the directory the hub publishes it in. Both are named on
//! the enrolment record, and the edge never invents the origin: the hub
//! returns it at enrolment, so an edge cannot sign under an origin it was not
//! enrolled on.
//!
//! An edge that is not enrolled, or whose key the hub has revoked, signs
//! nothing and says so on every record it writes. It still sends the product
//! token, which is the honest claim that this software made the request; the
//! signature is what makes it an identity.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use commonmeasure_http::Request;

use crate::declarations::PRODUCT_TOKEN;
use crate::enrolment::EnrolmentRecord;

/// The signature algorithm, as the `alg` parameter names it.
pub const ALGORITHM: &str = "ed25519";

/// The tag every Web Bot Auth request signature carries, which is how a
/// verifier tells this signature from any other on the same message.
pub const TAG: &str = "web-bot-auth";

/// The tag a directory proof carries: the tag a key directory response's
/// signatures carry (`draft-meunier-http-message-signatures-directory`).
pub const DIRECTORY_TAG: &str = "http-message-signatures-directory";

/// How far ahead of now a directory proof's expiry must be for the hub to
/// serve it: two of the directory's one-hour cache ages, so a copy cached
/// for its full hour still verifies for a further hour. The hub applies the
/// same margin when it lists keys.
pub const DIRECTORY_PROOF_MARGIN_SECS: i64 = 7_200;

/// The label the signature is published under. One signature per request, so
/// the label is fixed.
pub const SIGNATURE_LABEL: &str = "sig1";

/// The header naming the origin a verifier resolves `keyid` at.
pub const SIGNATURE_AGENT: &str = "Signature-Agent";

/// Where a key directory is served under the origin `Signature-Agent` names
/// (`draft-meunier-webbotauth-httpsig-protocol` §4). The header carries the
/// origin and the verifier appends this, so sending a full directory URL
/// would have a verifier request the path twice and find nothing.
pub const DIRECTORY_PATH: &str = "/.well-known/http-message-signatures-directory";
pub const SIGNATURE_INPUT: &str = "Signature-Input";
pub const SIGNATURE: &str = "Signature";

/// How long a request signature stays acceptable, in seconds. A request is
/// answered in seconds and a signature outliving its request is only a replay
/// window; five minutes covers a slow origin and a clock a little out.
///
/// One half of a bound stated in `docs/contracts/policy-envelope.md` §The hub
/// side: the hub allows a further minute either side for clock skew, and a
/// lifetime shorter than that allowance would accept a signature that had
/// never been valid. The two move together.
pub const SIGNATURE_LIFETIME_SECS: i64 = 300;

/// The private key file: an OKP JWK with `d`. Mode 0600 where the platform
/// has modes.
#[derive(Debug, Serialize, Deserialize)]
struct PrivateJwk {
    kty: String,
    crv: String,
    x: String,
    d: String,
}

/// The edge's signing key. Generated at `connect`, stored under `<home>`,
/// and never serialised anywhere else.
pub struct EdgeKey {
    signing: SigningKey,
}

// Derived, this would print the private key wherever a caller formats a value
// holding one. The thumbprint is the public name for the same key.
impl std::fmt::Debug for EdgeKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EdgeKey")
            .field("key_id", &self.thumbprint())
            .finish_non_exhaustive()
    }
}

impl EdgeKey {
    pub fn path(home: &Path) -> PathBuf {
        home.join("edge-key.json")
    }

    /// A fresh key from the operating system's randomness.
    pub fn generate() -> Result<Self, String> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|error| format!("draw key material from the operating system: {error}"))?;
        Ok(Self {
            signing: SigningKey::from_bytes(&secret),
        })
    }

    /// Read `<home>/edge-key.json`. `None` when there is no key file; an
    /// error when there is one this runtime cannot use, because an edge that
    /// holds an unreadable key is not the same thing as an edge that holds
    /// none.
    pub fn load(home: &Path) -> Result<Option<Self>, String> {
        let source = Self::path(home);
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let jwk: PrivateJwk = serde_json::from_slice(&encoded)
            .map_err(|error| format!("{} is not a valid key: {error}", source.display()))?;
        if jwk.kty != "OKP" || jwk.crv != "Ed25519" {
            return Err(format!(
                "{} is a {} key on curve {}; only OKP on Ed25519 signs here",
                source.display(),
                jwk.kty,
                jwk.crv
            ));
        }
        let secret: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&jwk.d)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| {
                format!(
                    "{}: d is not the 32-byte private key, base64url unpadded",
                    source.display()
                )
            })?;
        Ok(Some(Self {
            signing: SigningKey::from_bytes(&secret),
        }))
    }

    /// Write `<home>/edge-key.json`, readable by the owner only.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let path = Self::path(home);
        let jwk = PrivateJwk {
            kty: "OKP".to_owned(),
            crv: "Ed25519".to_owned(),
            x: self.jwk_x(),
            d: URL_SAFE_NO_PAD.encode(self.signing.to_bytes()),
        };
        let encoded =
            serde_json::to_vec_pretty(&jwk).map_err(|error| format!("serialise key: {error}"))?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &encoded)?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(())
    }

    /// The JWK `x` member: the public key, base64url unpadded.
    pub fn jwk_x(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.verifying_key().to_bytes())
    }

    /// The RFC 7638 thumbprint the hub assigns as key id, computed here too
    /// so the answer can be checked rather than trusted.
    ///
    /// RFC 7638 hashes the key's required members, in lexicographic order,
    /// with no whitespace. That is what RFC 8785 produces for this object, so
    /// the pre-image is built by the one canonicaliser
    /// (`docs/contracts/canonical-json.md`) rather than by a second hand
    /// spelling the same rule.
    pub fn thumbprint(&self) -> String {
        let members = json!({"crv": "Ed25519", "kty": "OKP", "x": self.jwk_x()});
        let canonical = commonmeasure_types::canonical::canonical_json(&members);
        URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
    }

    /// Sign `message`; base64url unpadded, the form the hub's exchange takes
    /// the proof of possession in.
    pub fn sign(&self, message: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.sign(message).to_bytes())
    }

    /// Sign `message` as an RFC 9421 signature value: raw bytes, which the
    /// caller wraps in the structured field's byte-sequence delimiters.
    fn sign_raw(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }
}

/// Write a file that holds a credential: created with mode 0600 where the
/// platform has modes, fsynced before the caller renames it into place.
#[cfg(unix)]
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot fsync {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// What this edge can present on a request.
///
/// The two states are different facts, not a value and its absence: a signing
/// edge is the network identity a publisher can verify, and an unsigned one
/// carries a product token that authenticates nothing. Every record says which
/// it was, and an unsigned request says why.
#[derive(Debug)]
pub enum Identity {
    /// Boxed because the key and its published URLs dwarf the other variant,
    /// and this enum is carried by value in the server that holds it for a
    /// whole session.
    Signing(Box<SigningIdentity>),
    Unsigned {
        reason: String,
    },
}

impl Identity {
    /// Read the identity this edge holds. `Err` is a state this runtime
    /// cannot act on — an enrolment record or key file it cannot read — never
    /// an edge that is merely not enrolled.
    pub fn load(home: &Path) -> Result<Self, String> {
        let Some(record) = EnrolmentRecord::load(home)? else {
            return Ok(Self::Unsigned {
                reason: format!(
                    "this edge is not enrolled with a hub, so it holds no key a publisher can \
                     verify: no enrolment record at {}",
                    EnrolmentRecord::path(home).display()
                ),
            });
        };
        // A revoked key is out of the directory, so a signature under it
        // verifies nowhere. Signing with it would claim an identity the hub
        // has withdrawn (`ROADMAP.md` §One-command hub enrolment).
        if record.revoked_at.is_some() {
            return Ok(Self::Unsigned {
                reason: format!(
                    "the enrolled key {} was revoked by the {}, so this edge stopped signing",
                    record.key_id,
                    record.revocation.as_deref().unwrap_or("hub")
                ),
            });
        }
        let Some(key) = EdgeKey::load(home)? else {
            return Err(format!(
                "enrolled as key {} but {} does not exist, so nothing can be signed; run \
                 `commonmeasure disconnect` and enrol again",
                record.key_id,
                EdgeKey::path(home).display()
            ));
        };
        if key.thumbprint() != record.key_id {
            return Err(format!(
                "the key in {} has thumbprint {} but the enrolment record names {}; nothing was \
                 signed",
                EdgeKey::path(home).display(),
                key.thumbprint(),
                record.key_id
            ));
        }
        Ok(Self::Signing(Box::new(SigningIdentity {
            key,
            key_id: record.key_id,
            origin: record.identity.origin,
            bot_page: record.identity.bot_page,
            contact: record.identity.contact,
        })))
    }

    /// The signer, or nothing when this edge does not sign.
    pub fn signer(&self) -> Option<&SigningIdentity> {
        match self {
            Self::Signing(signing) => Some(signing),
            Self::Unsigned { .. } => None,
        }
    }

    /// The `User-Agent` every signed request carries. A signing edge names where its
    /// documentation and its contact are, so a publisher reading a log can
    /// reach the party answering for the fleet without knowing the product; an
    /// unsigned edge names the product and version alone, because it has no
    /// published identity to point at.
    pub fn user_agent(&self) -> String {
        match self {
            Self::Signing(signing) => signing.user_agent(),
            Self::Unsigned { .. } => format!("{PRODUCT_TOKEN}/{}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// What the record says this request presented.
    pub fn presented(&self) -> PresentedIdentity {
        match self {
            Self::Signing(signing) => PresentedIdentity {
                user_agent: signing.user_agent(),
                key_id: Some(signing.key_id.clone()),
                signature_agent: Some(signing.origin.clone()),
                unsigned: None,
            },
            Self::Unsigned { reason } => PresentedIdentity {
                user_agent: self.user_agent(),
                key_id: None,
                signature_agent: None,
                unsigned: Some(reason.clone()),
            },
        }
    }
}

/// An enrolled key that stands, with the public facts a verifier needs.
#[derive(Debug)]
pub struct SigningIdentity {
    key: EdgeKey,
    key_id: String,
    /// The origin `Signature-Agent` names, as the hub returned it.
    origin: String,
    bot_page: String,
    contact: Option<String>,
}

impl SigningIdentity {
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// The origin this edge's `Signature-Agent` names.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The key directory a verifier reaches from that origin.
    pub fn directory(&self) -> String {
        format!("{}{DIRECTORY_PATH}", self.origin)
    }

    fn user_agent(&self) -> String {
        let contact = match &self.contact {
            Some(contact) => format!("; {contact}"),
            None => String::new(),
        };
        format!(
            "{PRODUCT_TOKEN}/{} (+{}{contact})",
            env!("CARGO_PKG_VERSION"),
            self.bot_page
        )
    }

    /// Attach `Signature-Agent`, `Signature-Input` and `Signature` to
    /// `request`, covering the authority of `url`, the signature agent, and
    /// each header named in `cover` that the request carries.
    ///
    /// Signed per request rather than once per fetch: `@authority` is the
    /// origin this hop reaches, so a redirect to another host must be signed
    /// again or the signature would attest to the host that redirected.
    pub fn sign_request(
        &self,
        url: &str,
        request: &mut Request,
        cover: &[&str],
        created: i64,
    ) -> Result<(), String> {
        let agent = quote(&self.origin);
        request.headers.set(SIGNATURE_AGENT, &agent);

        let mut covered = vec![
            (component("@authority"), authority_of(url)?),
            (component("signature-agent"), agent),
        ];
        for name in cover {
            if let Some(value) = request.headers.get(name) {
                covered.push((component(&name.to_ascii_lowercase()), value.to_owned()));
            }
        }

        let params = signature_params(
            &covered
                .iter()
                .map(|(identifier, _)| identifier.as_str())
                .collect::<Vec<_>>(),
            created,
            created + SIGNATURE_LIFETIME_SECS,
            &self.key_id,
            &nonce()?,
            TAG,
        );
        let base = signature_base(&covered, &params);
        let signature = self.key.sign_raw(base.as_bytes());

        request
            .headers
            .set(SIGNATURE_INPUT, &format!("{SIGNATURE_LABEL}={params}"));
        request.headers.set(
            SIGNATURE,
            &format!("{SIGNATURE_LABEL}=:{}:", STANDARD.encode(signature)),
        );
        Ok(())
    }

    /// Sign this key's agreement to be listed in the key directory served
    /// from `authority`, valid from `created` for `lifetime_secs`.
    ///
    /// A verifier that applies the per-key rule uses a listed key only when
    /// the directory response carries a signature by that key. The hub
    /// cannot make one, because the private key never leaves the edge, so
    /// the edge makes it in advance and uploads it. It covers the authority
    /// alone, never the body: the body is the whole key set, which changes on
    /// every enrolment and revocation, and a proof over it could not be made
    /// before the hub serves it.
    pub fn directory_proof(
        &self,
        authority: &str,
        created: i64,
        lifetime_secs: i64,
    ) -> Result<DirectoryProof, String> {
        Ok(self.key.directory_proof(
            &self.key_id,
            authority,
            created,
            created + lifetime_secs,
            &nonce()?,
        ))
    }
}

/// A signed directory proof as the hub's upload takes it: the parameters
/// verbatim, which the hub serves without rebuilding, and the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryProof {
    /// The `@signature-params` value, served as the key's
    /// `Signature-Input` member.
    pub signature_input: String,
    /// The 64-byte Ed25519 signature, standard base64.
    pub signature: String,
    pub created: i64,
    pub expires: i64,
}

impl EdgeKey {
    fn directory_proof(
        &self,
        key_id: &str,
        authority: &str,
        created: i64,
        expires: i64,
        nonce: &str,
    ) -> DirectoryProof {
        let covered = [(component_with_req("@authority"), authority.to_owned())];
        let params = signature_params(
            &[covered[0].0.as_str()],
            created,
            expires,
            key_id,
            nonce,
            DIRECTORY_TAG,
        );
        let base = signature_base(&covered, &params);
        DirectoryProof {
            signature_input: params,
            signature: STANDARD.encode(self.sign_raw(base.as_bytes())),
            created,
            expires,
        }
    }
}

/// A covered component identifier as RFC 9421 serialises it: the name as a
/// quoted string.
#[must_use]
pub fn component(name: &str) -> String {
    format!("\"{name}\"")
}

/// A component identifier with the `req` flag, which names the component of
/// the request a response answers: `@authority` on a directory response is
/// the authority the request for the directory reached.
#[must_use]
pub fn component_with_req(name: &str) -> String {
    format!("\"{name}\";req")
}

/// The signature parameters of the Web Bot Auth profile: the covered
/// component identifiers in order, then the parameters a verifier checks,
/// ending with `tag`, which says what kind of signature this is.
#[must_use]
pub fn signature_params(
    components: &[&str],
    created: i64,
    expires: i64,
    key_id: &str,
    nonce: &str,
    tag: &str,
) -> String {
    let listed = components.join(" ");
    format!(
        "({listed});created={created};expires={expires};keyid=\"{key_id}\";alg=\"{ALGORITHM}\";\
         nonce=\"{nonce}\";tag=\"{tag}\""
    )
}

/// The RFC 9421 signature base: one line per covered component, by its
/// serialised identifier, then the signature parameters. The one
/// construction both sides of every signature this product makes are
/// checked against.
#[must_use]
pub fn signature_base(covered: &[(String, String)], params: &str) -> String {
    let mut base = String::new();
    for (identifier, value) in covered {
        base.push_str(&format!("{identifier}: {value}\n"));
    }
    base.push_str(&format!("\"@signature-params\": {params}"));
    base
}

/// The `@authority` of a URL: host and any non-default port, lowercased, with
/// no scheme. What the origin sees in `Host`, which is what binds a signature
/// to the origin it was made for.
pub fn authority_of(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("parse url {url}: {error}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("{url} has no host"))?
        .to_ascii_lowercase();
    let default_port = match parsed.scheme() {
        "https" => 443,
        "http" => 80,
        other => return Err(format!("{other} is not a scheme this signs for")),
    };
    Ok(match parsed.port() {
        Some(port) if port != default_port => format!("{host}:{port}"),
        _ => host,
    })
}

/// A structured-field string, the form `Signature-Agent` takes.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A fresh nonce, so two identical requests do not produce the same signature
/// and a verifier that keeps them can refuse a replay.
fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("draw a nonce from the operating system: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The network identity one request presented, as the evidence record carries
/// it. Written on every mediated crossing, signed or not: a reader has to be
/// able to tell a request a publisher could verify from one it could not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedIdentity {
    pub user_agent: String,
    /// The enrolled key the request was signed with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// The origin a verifier resolves `key_id` at, as `Signature-Agent`
    /// carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_agent: Option<String>,
    /// Why the request carried no signature. Absent when it was signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsigned: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8037 appendix A.3: the Ed25519 key whose thumbprint the RFC
    /// states. The key id this edge computes must equal what the hub
    /// assigns, and both must equal the RFC.
    #[test]
    fn the_thumbprint_matches_the_rfc_8037_vector() {
        let key = vector_key();
        assert_eq!(key.jwk_x(), "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo");
        assert_eq!(
            key.thumbprint(),
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"
        );
    }

    fn vector_key() -> EdgeKey {
        let secret: [u8; 32] = URL_SAFE_NO_PAD
            .decode("nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A")
            .expect("the RFC's private key")
            .try_into()
            .expect("32 bytes");
        EdgeKey {
            signing: SigningKey::from_bytes(&secret),
        }
    }

    #[test]
    fn a_stored_key_round_trips_and_is_owner_readable() {
        let home = tempfile::tempdir().expect("home");
        let key = EdgeKey::generate().expect("generate");
        key.store(home.path()).expect("store");
        let read = EdgeKey::load(home.path())
            .expect("load")
            .expect("the key is there");
        assert_eq!(read.thumbprint(), key.thumbprint());
        assert_eq!(read.jwk_x(), key.jwk_x());
        assert!(
            EdgeKey::load(tempfile::tempdir().expect("empty").path())
                .expect("no key is not an error")
                .is_none()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(EdgeKey::path(home.path()))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the private key is owner-readable only"
            );
        }
    }

    #[test]
    fn a_key_file_that_cannot_be_used_is_an_error_not_an_unenrolled_edge() {
        let home = tempfile::tempdir().expect("home");
        std::fs::write(
            EdgeKey::path(home.path()),
            r#"{"kty":"EC","crv":"P-256","x":"a","d":"b"}"#,
        )
        .expect("write");
        let error = EdgeKey::load(home.path()).expect_err("an EC key does not sign here");
        assert!(error.contains("OKP"), "{error}");
    }

    #[test]
    fn the_authority_is_the_host_and_any_non_default_port() {
        assert_eq!(
            authority_of("https://Example.ORG/a?b=c").unwrap(),
            "example.org"
        );
        assert_eq!(
            authority_of("https://example.org:443/a").unwrap(),
            "example.org"
        );
        assert_eq!(
            authority_of("http://127.0.0.1:8080/policy").unwrap(),
            "127.0.0.1:8080"
        );
        assert!(authority_of("ftp://example.org/a").is_err());
    }

    /// The pinned cross-implementation vector. The hub verifies these
    /// signatures, and the two implementations never run in one process, so
    /// the base construction is fixed here by value: a change to the
    /// component order, the parameter order or the line format shows up as a
    /// different base and a different signature, and the same vector in the
    /// hub's test suite fails against the other half.
    ///
    /// The key is the one RFC 8037 appendix A.3 publishes, so the vector can
    /// be recomputed with any Ed25519 implementation. This one was checked
    /// against `openssl pkeyutl -sign -rawin` over the same base.
    #[test]
    fn the_signature_base_and_signature_match_the_pinned_vector() {
        let params = signature_params(
            &["\"@authority\"", "\"signature-agent\""],
            1_757_160_000,
            1_757_160_300,
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
            "AAAAAAAAAAAAAAAAAAAAAA",
            TAG,
        );
        assert_eq!(
            params,
            "(\"@authority\" \"signature-agent\");created=1757160000;expires=1757160300;\
             keyid=\"kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k\";alg=\"ed25519\";\
             nonce=\"AAAAAAAAAAAAAAAAAAAAAA\";tag=\"web-bot-auth\""
        );
        let covered = vec![
            (component("@authority"), "www.example.org".to_owned()),
            (
                component("signature-agent"),
                "\"https://hub.example\"".to_owned(),
            ),
        ];
        let base = signature_base(&covered, &params);
        assert_eq!(
            base,
            format!(
                "\"@authority\": www.example.org\n\"signature-agent\": \"https://hub.example\"\n\
                 \"@signature-params\": {params}"
            )
        );
        assert_eq!(
            STANDARD.encode(vector_key().sign_raw(base.as_bytes())),
            "JWCbgf/sAf+KKYQ+ov+hggk34carkv/eJf3ZQNrGK0CUc0qvdTt/qWx0TdvrT7orffYMT+hkOK3GRsI5p7I9CA=="
        );
    }

    /// The pinned directory-proof vector, shared with the hub's upload
    /// check. Same key and method as the request vector above: the base was
    /// signed with `openssl pkeyutl -sign -rawin` and verified with
    /// `openssl pkeyutl -verify`. The component list, the `req` flag, the
    /// parameter order and the tag are all fixed by value here, so a change
    /// to any of them fails this test and the hub's copy of it.
    #[test]
    fn the_directory_proof_matches_the_pinned_vector() {
        let proof = vector_key().directory_proof(
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
            "hub.example",
            1_757_160_000,
            1_757_764_800,
            "AAAAAAAAAAAAAAAAAAAAAA",
        );
        let params = "(\"@authority\";req);created=1757160000;expires=1757764800;\
                      keyid=\"kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k\";alg=\"ed25519\";\
                      nonce=\"AAAAAAAAAAAAAAAAAAAAAA\";tag=\"http-message-signatures-directory\"";
        assert_eq!(proof.signature_input, params);
        assert_eq!(
            signature_base(
                &[(component_with_req("@authority"), "hub.example".to_owned())],
                &proof.signature_input
            ),
            format!("\"@authority\";req: hub.example\n\"@signature-params\": {params}")
        );
        assert_eq!(
            proof.signature,
            "DPpR6ACaG9KIFdTCoJ6LPfcAwjHNZ7VTy4BNjPWWjTWujpDFXGzy2UaGRoDza60NBXMlSMUBetggbWYsCLolBA=="
        );
        assert_eq!(
            (proof.created, proof.expires),
            (1_757_160_000, 1_757_764_800)
        );
    }

    /// A proof made through the signing identity carries a fresh nonce and
    /// the expiry its lifetime implies, and is signed under the enrolled key
    /// id.
    #[test]
    fn a_directory_proof_is_signed_under_the_key_id_with_a_fresh_nonce() {
        let identity = SigningIdentity {
            key: vector_key(),
            key_id: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k".to_owned(),
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
        };
        let first = identity
            .directory_proof("hub.example", 1_000, 604_800)
            .expect("sign");
        let second = identity
            .directory_proof("hub.example", 1_000, 604_800)
            .expect("sign");
        assert_eq!(first.expires, 605_800);
        assert!(
            first
                .signature_input
                .starts_with("(\"@authority\";req);created=1000;expires=605800;keyid=\"kPrK_"),
            "{}",
            first.signature_input
        );
        assert_ne!(first.signature_input, second.signature_input, "nonce");
        assert_ne!(first.signature, second.signature);
    }

    /// The signature covers the telemetry correlation id when the request
    /// carries one, so the id a publisher logs is one the signature protects
    /// (`ROADMAP.md` §Verified fetcher identity). A request without one covers two components,
    /// not three: an absent header is never signed as empty.
    #[test]
    fn the_telemetry_id_is_covered_when_the_request_carries_one() {
        let identity = SigningIdentity {
            key: EdgeKey::generate().expect("generate"),
            key_id: "key-1".to_owned(),
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: Some("mailto:bot@hub.example".to_owned()),
        };

        let mut bare = Request::get("/");
        identity
            .sign_request("https://www.example.org/page", &mut bare, &["X-Test-Id"], 1)
            .expect("sign");
        let input = bare.headers.get(SIGNATURE_INPUT).expect("signed");
        assert!(
            input.starts_with("sig1=(\"@authority\" \"signature-agent\");"),
            "{input}"
        );
        assert!(input.contains("tag=\"web-bot-auth\""), "{input}");

        let mut carrying = Request::get("/");
        carrying.headers.set("X-Test-Id", "abc");
        identity
            .sign_request(
                "https://www.example.org/page",
                &mut carrying,
                &["X-Test-Id"],
                1,
            )
            .expect("sign");
        let input = carrying.headers.get(SIGNATURE_INPUT).expect("signed");
        assert!(
            input.starts_with("sig1=(\"@authority\" \"signature-agent\" \"x-test-id\");"),
            "{input}"
        );
        assert_eq!(
            carrying.headers.get(SIGNATURE_AGENT),
            Some("\"https://hub.example\""),
            "the header carries the origin; the verifier appends the well-known path"
        );
        assert_eq!(
            identity.directory(),
            "https://hub.example/.well-known/http-message-signatures-directory"
        );
        assert_ne!(
            bare.headers.get(SIGNATURE),
            carrying.headers.get(SIGNATURE),
            "a covered id changes the signature"
        );
    }

    /// The user agent points a publisher at the page that explains the bot
    /// and at a contact, and an unenrolled edge points at neither because it
    /// has no published identity.
    #[test]
    fn the_user_agent_names_the_documentation_and_the_contact_when_enrolled() {
        let unsigned = Identity::Unsigned {
            reason: "not enrolled".to_owned(),
        };
        assert_eq!(
            unsigned.user_agent(),
            format!("CommonMeasureBot/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            unsigned.presented().unsigned.as_deref(),
            Some("not enrolled")
        );

        let signing = Identity::Signing(Box::new(SigningIdentity {
            key: EdgeKey::generate().expect("generate"),
            key_id: "key-1".to_owned(),
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: Some("mailto:bot@hub.example".to_owned()),
        }));
        assert_eq!(
            signing.user_agent(),
            format!(
                "CommonMeasureBot/{} (+https://hub.example/bot; mailto:bot@hub.example)",
                env!("CARGO_PKG_VERSION")
            )
        );
        let presented = signing.presented();
        assert_eq!(presented.key_id.as_deref(), Some("key-1"));
        assert!(presented.unsigned.is_none());
    }

    /// A revoked key is out of the directory, so signing with it would claim
    /// an identity the hub withdrew. The reason is on the record.
    #[test]
    fn a_revoked_key_stops_signing_and_the_record_says_why() {
        let home = tempfile::tempdir().expect("home");
        let key = EdgeKey::generate().expect("generate");
        key.store(home.path()).expect("store");
        let mut record = crate::enrolment::EnrolmentRecord {
            hub: "https://hub.example".to_owned(),
            organization: crate::enrolment::EnrolledOrganization {
                id: "org-1".to_owned(),
                name: "Org".to_owned(),
            },
            name: "laptop".to_owned(),
            key_id: key.thumbprint(),
            identity: crate::enrolment::EnrolledIdentity {
                origin: "https://hub.example".to_owned(),
                bot_page: "https://hub.example/bot".to_owned(),
                contact: None,
            },
            enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
            revoked_at: None,
            revocation: None,
            revocation_learnt_at: None,
        };
        record.store(home.path()).expect("store");
        assert!(
            Identity::load(home.path())
                .expect("load")
                .signer()
                .is_some(),
            "an enrolled key signs"
        );

        record.revoked_at = Some("2026-09-06T01:00:00.000Z".to_owned());
        record.revocation = Some("owner".to_owned());
        record.store(home.path()).expect("store");
        let identity = Identity::load(home.path()).expect("load");
        assert!(identity.signer().is_none());
        let reason = identity.presented().unsigned.expect("a reason");
        assert!(reason.contains("revoked by the owner"), "{reason}");
    }

    /// An enrolment record with no key beside it is a broken edge, not an
    /// unenrolled one: it must fail loudly rather than fetch unsigned under a
    /// key id its records still name.
    #[test]
    fn an_enrolment_without_its_key_is_an_error() {
        let home = tempfile::tempdir().expect("home");
        crate::enrolment::EnrolmentRecord {
            hub: "https://hub.example".to_owned(),
            organization: crate::enrolment::EnrolledOrganization {
                id: "org-1".to_owned(),
                name: "Org".to_owned(),
            },
            name: "laptop".to_owned(),
            key_id: "key-1".to_owned(),
            identity: crate::enrolment::EnrolledIdentity {
                origin: "https://hub.example".to_owned(),
                bot_page: "https://hub.example/bot".to_owned(),
                contact: None,
            },
            enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
            revoked_at: None,
            revocation: None,
            revocation_learnt_at: None,
        }
        .store(home.path())
        .expect("store");
        let error = Identity::load(home.path()).expect_err("no key file");
        assert!(error.contains("does not exist"), "{error}");
    }
}
