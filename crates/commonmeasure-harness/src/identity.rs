//! The edge's network identity: the Ed25519 key minted at enrolment and the
//! HTTP message signature the requests it decides for itself carry.
//!
//! One signing implementation serves both directions. Towards a publisher the
//! mediated fetch and the `robots.txt`, licence and manifest probes beside it
//! sign, so the site can verify who fetched; towards the hub the policy fetch
//! signs, so the hub can authenticate the edge with no second credential.
//! Both are RFC 9421 signatures under the Web Bot Auth profile, over the same
//! components with the same parameters, so a publisher and the hub verify by
//! one rule.
//! The supplier credential release request is the one exception: it also
//! covers `@method` and `@path`, because its answer carries a secret and a
//! signature good at any route could be presented there
//! (`docs/contracts/supplier-credentials.md` §Signature components of the
//! release request).
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
///
/// Also one half of a second bound: the hub's release route refuses a
/// signature whose `expires` is more than 360 seconds after `created`
/// (`docs/contracts/supplier-credentials.md` §Release). A lifetime above that
/// turns every release into a `401`, which clears the released set.
pub const SIGNATURE_LIFETIME_SECS: i64 = 300;

/// How many random bytes a nonce is drawn from. In unpadded base64url that is
/// 22 characters, which is the shortest nonce the hub's release route accepts
/// (`docs/contracts/supplier-credentials.md` §Signature components of the
/// release request). Fewer bytes, or an encoding that writes them shorter,
/// turns every release into a `401`.
pub const NONCE_BYTES: usize = 16;

/// A derived component of RFC 9421 §2.2 that a caller may ask
/// [`SigningIdentity::sign_request_covering`] to cover. `@authority` is not
/// listed because every request signature covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Derived {
    /// `@method`: the request's method as sent.
    Method,
    /// `@path`: the path of the URL requested, without its query.
    Path,
}

impl Derived {
    fn name(self) -> &'static str {
        match self {
            Self::Method => "@method",
            Self::Path => "@path",
        }
    }
}

/// How far before its own clock an edge dates `created` on a lifecycle
/// request. The hub's registration profile refuses a `created` later than its
/// own clock and allows no grace, so an edge dating `created` at its own time
/// would fail every request whenever its clock ran a second ahead. RFC 9421
/// does not require `created` to be the sending time, and the hub checks only
/// that it is not in the future and that `expires` is at most
/// [`SIGNATURE_LIFETIME_SECS`] after it. `expires` stays that far after
/// `created`, so a signature is good for 270 seconds from sending.
pub const REGISTRATION_SKEW_ALLOWANCE_SECS: i64 = 30;

/// What kind of signature an instance registration request carries
/// (`docs/contracts/session-evidence.md` §Instance registration). A separate
/// tag from [`TAG`], so a signature made for a publisher can never be
/// replayed at the hub's lifecycle routes or the reverse.
pub const REGISTRATION_TAG: &str = "commonmeasure-registration";
pub const CONTENT_DIGEST: &str = "Content-Digest";
pub const IDEMPOTENCY_KEY: &str = "Idempotency-Key";

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

    /// Write `<home>/edge-key.json` atomically, readable by the owner only,
    /// through a temporary file of this writer's own.
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
        crate::declaration::replace_private(&path, &encoded)
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
        Self::from_record(home, EnrolmentRecord::load(home)?)
    }

    /// [`Identity::load`] over an enrolment record the caller has already
    /// read, `None` where `home` holds none. A caller that acts on the record
    /// as well as the identity reads it once and gives it here, so the two
    /// cannot come from different writes of the file.
    pub fn from_record(home: &Path, record: Option<EnrolmentRecord>) -> Result<Self, String> {
        let Some(record) = record else {
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
        // has withdrawn.
        if record.is_revoked() {
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
            enrolment_home: Some(home.to_owned()),
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
    /// The home whose enrolment record is read again before each signature
    /// ([`Self::still_enrolled`]). `None` only for the fixed signing vectors
    /// in this module's tests, which have no home.
    enrolment_home: Option<PathBuf>,
}

impl SigningIdentity {
    /// Whether the enrolment this identity was loaded from still stands.
    /// A process holds its identity for its life (an MCP server for its host
    /// session, a hosted service session until it ends), and a revocation
    /// learnt by another process in the meantime must stop it signing too.
    /// One read of `enrolment.json` per signature, so a refusal lasts only
    /// while the record says revoked, names another key, is gone or cannot
    /// be read: one failed read refuses one signature, and a withdrawn
    /// 401-only revocation lets this identity sign again
    /// (`docs/contracts/bot-identity.md` §Revocation).
    fn still_enrolled(&self) -> Result<(), String> {
        let Some(home) = &self.enrolment_home else {
            return Ok(());
        };
        let record = EnrolmentRecord::load(home).map_err(|reason| {
            format!("{reason}, so this signature was refused; the next one reads it again")
        })?;
        match record {
            None => Err(format!(
                "the enrolment record at {} is gone, so this session stops signing as key {}; a \
                 new session runs unsigned, and `commonmeasure connect` enrols a new key",
                EnrolmentRecord::path(home).display(),
                self.key_id
            )),
            Some(record) if record.key_id != self.key_id => Err(format!(
                "this edge is now enrolled as key {}, not key {}, which this session loaded; a \
                 new session signs under the new key",
                record.key_id, self.key_id
            )),
            Some(record) if record.is_revoked() => Err(format!(
                "the enrolled key {} was revoked by the {} after this session loaded it, so this \
                 session stops signing with it; a new session runs unsigned, and `commonmeasure \
                 connect` enrols a new key",
                self.key_id,
                record.revocation.as_deref().unwrap_or("hub")
            )),
            Some(_) => Ok(()),
        }
    }

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
    ///
    /// This covers no derived component but `@authority`. A hub before the
    /// custody release signature reads any other covered name as a header
    /// and refuses the signature, and what a publisher's verifier does with
    /// one is not the edge's or the hub's to change, so the desired-policy
    /// request and every request signed for a publisher go through here
    /// (`docs/contracts/supplier-credentials.md` §Signature components of the
    /// release request).
    pub fn sign_request(
        &self,
        url: &str,
        request: &mut Request,
        cover: &[&str],
        created: i64,
    ) -> Result<(), String> {
        self.sign_request_covering(url, request, &[], cover, created)
    }

    /// [`Self::sign_request`], also covering each derived component in
    /// `derived`, in the order given, after `signature-agent` and before the
    /// headers of `cover`. With `derived` empty the signature base is the one
    /// [`Self::sign_request`] has always made.
    ///
    /// `@path` is read from `url` by the parser the HTTP client takes the
    /// request target from, and `@method` from `request` as it stands at the
    /// call. What is signed is what is sent only if the caller then sends
    /// `request` to the same `url` without changing its method; nothing here
    /// enforces that. `request.target` is not read: the client overwrites it
    /// from `url`.
    pub fn sign_request_covering(
        &self,
        url: &str,
        request: &mut Request,
        derived: &[Derived],
        cover: &[&str],
        created: i64,
    ) -> Result<(), String> {
        self.sign_with_nonce(url, request, derived, cover, created, &nonce()?)
    }

    fn sign_with_nonce(
        &self,
        url: &str,
        request: &mut Request,
        derived: &[Derived],
        cover: &[&str],
        created: i64,
        nonce: &str,
    ) -> Result<(), String> {
        self.still_enrolled()?;
        let agent = quote(&self.origin);
        request.headers.set(SIGNATURE_AGENT, &agent);

        let mut covered = vec![
            (component("@authority"), authority_of(url)?),
            (component("signature-agent"), agent),
        ];
        for (position, asked) in derived.iter().enumerate() {
            // RFC 9421 §2.5: a component identifier occurs once.
            if derived[..position].contains(asked) {
                return Err(format!("{} was asked for twice", asked.name()));
            }
            let value = match asked {
                Derived::Method => request.method.clone(),
                Derived::Path => path_of(url)?,
            };
            covered.push((component(asked.name()), value));
        }
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
            nonce,
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
        self.still_enrolled()?;
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

/// What a registration signature covers of one lifecycle request.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleRequest<'a> {
    pub method: &'a str,
    /// The absolute public target, as the hub reconstructs it from its own
    /// origin and the request path.
    pub target_uri: &'a str,
    /// The exact bytes sent; empty for a read.
    pub body: &'a [u8],
    pub idempotency_key: &'a str,
}

/// One signed instance registration request: the four headers the hub's
/// lifecycle routes read and the signature base they were made over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationSignature {
    /// `sha-256=:<base64>:` over the exact body bytes; of no bytes for a read.
    pub content_digest: String,
    pub idempotency_key: String,
    pub signature_input: String,
    pub signature: String,
    /// The RFC 9421 base the signature covers, kept so a test can hold it to
    /// the pinned vector byte for byte.
    pub base: String,
}

impl RegistrationSignature {
    /// What the hub binds an idempotency key to: the method, the absolute
    /// target and the body digest, without the nonce and times, so a retry
    /// that signs a fresh nonce reproduces it.
    #[must_use]
    pub fn request_digest(method: &str, target_uri: &str, content_digest: &str) -> String {
        format!(
            "sha256:{:x}",
            Sha256::digest(format!("{method}\n{target_uri}\n{content_digest}"))
        )
    }

    pub fn attach(&self, request: &mut Request) {
        request.headers.set(CONTENT_DIGEST, &self.content_digest);
        request.headers.set(IDEMPOTENCY_KEY, &self.idempotency_key);
        request.headers.set(SIGNATURE_INPUT, &self.signature_input);
        request.headers.set(SIGNATURE, &self.signature);
    }
}

impl EdgeKey {
    /// Sign one lifecycle request under the registration profile: `@method`,
    /// the absolute public `@target-uri`, `content-digest` over `body` and
    /// `idempotency-key`, with `created`, `expires`, `keyid`, `alg`, `nonce`
    /// and the registration tag as parameters. The nonce is the caller's so
    /// the pinned interoperability vector can be reproduced; every other
    /// caller goes through [`SigningIdentity::sign_registration`].
    #[must_use]
    pub fn sign_registration(
        &self,
        key_id: &str,
        request: &LifecycleRequest<'_>,
        nonce: &str,
        created: i64,
    ) -> RegistrationSignature {
        let content_digest = format!(
            "sha-256=:{}:",
            STANDARD.encode(Sha256::digest(request.body))
        );
        let covered = [
            (component("@method"), request.method.to_owned()),
            (component("@target-uri"), request.target_uri.to_owned()),
            (component("content-digest"), content_digest.clone()),
            (
                component("idempotency-key"),
                request.idempotency_key.to_owned(),
            ),
        ];
        let params = signature_params(
            &covered
                .iter()
                .map(|(identifier, _)| identifier.as_str())
                .collect::<Vec<_>>(),
            created,
            created + SIGNATURE_LIFETIME_SECS,
            key_id,
            nonce,
            REGISTRATION_TAG,
        );
        let base = signature_base(&covered, &params);
        let signature = self.sign_raw(base.as_bytes());
        RegistrationSignature {
            content_digest,
            idempotency_key: request.idempotency_key.to_owned(),
            signature_input: format!("{SIGNATURE_LABEL}={params}"),
            signature: format!("{SIGNATURE_LABEL}=:{}:", STANDARD.encode(signature)),
            base,
        }
    }
}

impl SigningIdentity {
    /// [`EdgeKey::sign_registration`] under this edge's enrolled key id with
    /// a fresh nonce, which is what lets a retry keep its idempotency key.
    pub fn sign_registration(
        &self,
        request: &LifecycleRequest<'_>,
        created: i64,
    ) -> Result<RegistrationSignature, String> {
        self.still_enrolled()?;
        Ok(self
            .key
            .sign_registration(&self.key_id, request, &nonce()?, created))
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

/// The `@path` of a URL: its path without the query, `/` where it has none.
/// Read with the parser `commonmeasure_http` takes the request target from,
/// so the path signed is the path sent.
pub fn path_of(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("parse url {url}: {error}"))?;
    Ok(parsed.path().to_owned())
}

/// A structured-field string, the form `Signature-Agent` takes.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A fresh nonce, so two identical requests do not produce the same signature
/// and a verifier that keeps them can refuse a replay.
fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; NONCE_BYTES];
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

    /// RFC 8037 appendix A: the Ed25519 key of A.1 and A.2, whose thumbprint
    /// A.3 states. The key id this edge computes must equal what the hub
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

    /// Two `connect` runs at once each store a key. Each writes through its
    /// own temporary file, so neither renames the other's away and the key
    /// that lands is one of theirs, whole.
    #[test]
    fn concurrent_key_stores_each_land_whole() {
        let home = tempfile::tempdir().expect("home");
        let keys: Vec<EdgeKey> = (0..4)
            .map(|_| EdgeKey::generate().expect("generate"))
            .collect();
        let failures = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for key in &keys {
                let (home, failures) = (home.path(), &failures);
                scope.spawn(move || {
                    for _ in 0..40 {
                        if let Err(error) = key.store(home) {
                            failures.lock().unwrap().push(error);
                        }
                    }
                });
            }
        });
        assert_eq!(failures.into_inner().unwrap(), Vec::<String>::new());
        let landed = EdgeKey::load(home.path()).expect("load").expect("a key");
        assert!(
            keys.iter()
                .any(|key| key.thumbprint() == landed.thumbprint())
        );
        let leftovers: Vec<String> = std::fs::read_dir(home.path())
            .expect("list")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
    /// The key is the one RFC 8037 appendix A publishes (A.1 and A.2), so the
    /// vector can be recomputed with any Ed25519 implementation. This one was
    /// checked against `openssl pkeyutl -sign -rawin` over the same base.
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
            enrolment_home: None,
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
    /// carries one, so the id a publisher logs is one the signature protects.
    /// A request without one covers two components, not three: an absent
    /// header is never signed as empty.
    #[test]
    fn the_telemetry_id_is_covered_when_the_request_carries_one() {
        let identity = SigningIdentity {
            key: EdgeKey::generate().expect("generate"),
            key_id: "key-1".to_owned(),
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: Some("mailto:bot@hub.example".to_owned()),
            enrolment_home: None,
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

    fn vector_identity() -> SigningIdentity {
        SigningIdentity {
            key: vector_key(),
            key_id: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k".to_owned(),
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
            enrolment_home: None,
        }
    }

    const VECTOR_NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    /// The signature headers `request` was given, in a fixed order.
    fn signature_headers(request: &Request) -> [String; 3] {
        [SIGNATURE_AGENT, SIGNATURE_INPUT, SIGNATURE]
            .map(|name| request.headers.get(name).expect("signed").to_owned())
    }

    /// The desired-policy client and the mediated crossing ask for no derived
    /// component, and what they sign is what they signed before the signer
    /// could cover one. Ed25519 is deterministic, so a signature equal to one
    /// made over a base written out here shows the base is that one, byte for
    /// byte. The first expectation is the vector pinned above, which predates
    /// the change; the second is the crossing's shape with its telemetry id.
    ///
    /// A released edge signs exactly this, and a hub before the custody
    /// release signature reads a covered `@method` as a header it cannot
    /// find and refuses. That `sign_request` itself asks for nothing is shown
    /// by `the_telemetry_id_is_covered_when_the_request_carries_one`, and
    /// that `managed.rs` and `mcp.rs` call it by the CLI tests' verifier,
    /// which refuses a derived component on those routes.
    #[test]
    fn a_request_that_asks_for_no_derived_component_is_signed_as_before() {
        let identity = vector_identity();
        let params = "(\"@authority\" \"signature-agent\");created=1757160000;expires=1757160300;\
                      keyid=\"kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k\";alg=\"ed25519\";\
                      nonce=\"AAAAAAAAAAAAAAAAAAAAAA\";tag=\"web-bot-auth\"";

        // As `managed.rs` signs the policy request: no header to cover.
        let mut policy = Request::get("/");
        identity
            .sign_with_nonce(
                "https://www.example.org/api/v1/policy/desired",
                &mut policy,
                &[],
                &[],
                1_757_160_000,
                VECTOR_NONCE,
            )
            .expect("sign");
        assert_eq!(
            signature_headers(&policy),
            [
                "\"https://hub.example\"".to_owned(),
                format!("sig1={params}"),
                "sig1=:JWCbgf/sAf+KKYQ+ov+hggk34carkv/eJf3ZQNrGK0CUc0qvdTt/qWx0TdvrT7orffYMT+hkOK3GRsI5p7I9CA==:"
                    .to_owned(),
            ]
        );

        // As `mcp.rs` signs a crossing: the telemetry id, absent then present.
        let mut bare = Request::get("/");
        identity
            .sign_with_nonce(
                "https://www.example.org/page?q=1",
                &mut bare,
                &[],
                &["Content-Telemetry-Id"],
                1_757_160_000,
                VECTOR_NONCE,
            )
            .expect("sign");
        assert_eq!(signature_headers(&bare), signature_headers(&policy));

        let mut carrying = Request::get("/");
        carrying.headers.set("Content-Telemetry-Id", "abc");
        identity
            .sign_with_nonce(
                "https://www.example.org/page?q=1",
                &mut carrying,
                &[],
                &["Content-Telemetry-Id"],
                1_757_160_000,
                VECTOR_NONCE,
            )
            .expect("sign");
        let params = params.replacen(
            "\"signature-agent\")",
            "\"signature-agent\" \"content-telemetry-id\")",
            1,
        );
        let base = format!(
            "\"@authority\": www.example.org\n\"signature-agent\": \"https://hub.example\"\n\
             \"content-telemetry-id\": abc\n\"@signature-params\": {params}"
        );
        assert_eq!(
            signature_headers(&carrying)[1..],
            [
                format!("sig1={params}"),
                format!(
                    "sig1=:{}:",
                    STANDARD.encode(vector_key().sign_raw(base.as_bytes()))
                ),
            ]
        );

        // A derived component goes after `signature-agent` and before a
        // covered header, as `sign_request_covering` documents.
        let mut ordered = Request::get("/");
        ordered.headers.set("Content-Telemetry-Id", "abc");
        identity
            .sign_with_nonce(
                "https://www.example.org/page?q=1",
                &mut ordered,
                &[Derived::Path],
                &["Content-Telemetry-Id"],
                1_757_160_000,
                VECTOR_NONCE,
            )
            .expect("sign");
        let params = params.replacen(
            "\"content-telemetry-id\"",
            "\"@path\" \"content-telemetry-id\"",
            1,
        );
        let base = format!(
            "\"@authority\": www.example.org\n\"signature-agent\": \"https://hub.example\"\n\
             \"@path\": /page\n\"content-telemetry-id\": abc\n\"@signature-params\": {params}"
        );
        assert_eq!(
            signature_headers(&ordered)[1..],
            [
                format!("sig1={params}"),
                format!(
                    "sig1=:{}:",
                    STANDARD.encode(vector_key().sign_raw(base.as_bytes()))
                ),
            ]
        );

        // The public entry point with nothing asked for is the same signer.
        let mut public = Request::get("/");
        identity
            .sign_request_covering("https://www.example.org/", &mut public, &[], &[], 1)
            .expect("sign");
        assert!(
            signature_headers(&public)[1]
                .starts_with("sig1=(\"@authority\" \"signature-agent\");created=1;expires=301;"),
            "{:?}",
            signature_headers(&public)
        );
    }

    /// The release request's pinned vector
    /// (`docs/contracts/supplier-credentials-release-vector.json`), which the
    /// hub's verifier test reads a copy of. The file's signature was made
    /// with `openssl`, not this signer, so the test fails on a change to the
    /// component order, the value either derived component takes, or the
    /// file. The file's base is tied to its headers by verifying the
    /// signature over it.
    #[test]
    fn the_release_request_matches_the_pinned_vector() {
        use ed25519_dalek::{Signature, VerifyingKey};

        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/contracts/supplier-credentials-release-vector.json"
        ))
        .expect("the vector is JSON");
        let text = |pointer: &str| {
            vector
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("the vector has no {pointer}"))
                .to_owned()
        };
        let secret: [u8; 32] = URL_SAFE_NO_PAD
            .decode(text("/key/d"))
            .expect("base64url")
            .try_into()
            .expect("32 bytes");
        let identity = SigningIdentity {
            key: EdgeKey {
                signing: SigningKey::from_bytes(&secret),
            },
            key_id: text("/key_id"),
            origin: text("/signature_agent_origin"),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
            enrolment_home: None,
        };
        assert_eq!(identity.key.jwk_x(), text("/key/x"));
        assert_eq!(identity.key.thumbprint(), text("/key_id"));

        let created = vector["created"].as_i64().expect("created");
        assert_eq!(
            vector["expires"].as_i64().expect("expires"),
            created + SIGNATURE_LIFETIME_SECS
        );
        let url = text("/request/url");
        assert_eq!(authority_of(&url).unwrap(), text("/request/authority"));
        assert_eq!(path_of(&url).unwrap(), text("/request/path"));

        let mut request = Request::get("/");
        assert_eq!(request.method, text("/request/method"));
        identity
            .sign_with_nonce(
                &url,
                &mut request,
                &[Derived::Method, Derived::Path],
                &[],
                created,
                &text("/nonce"),
            )
            .expect("sign");
        assert_eq!(
            signature_headers(&request),
            [
                text("/headers/Signature-Agent"),
                text("/headers/Signature-Input"),
                text("/headers/Signature"),
            ]
        );

        let signature: [u8; 64] = STANDARD
            .decode(
                text("/headers/Signature")
                    .trim_start_matches("sig1=:")
                    .trim_end_matches(':'),
            )
            .expect("base64")
            .try_into()
            .expect("64 bytes");
        VerifyingKey::from(&identity.key.signing)
            .verify_strict(
                text("/signature_base").as_bytes(),
                &Signature::from_bytes(&signature),
            )
            .expect("the vector's signature is over the vector's base");
    }

    /// `@path` is the path the client sends, never the query, and a component
    /// asked for twice is refused because RFC 9421 lists each once.
    #[test]
    fn the_path_excludes_the_query_and_a_repeated_component_is_refused() {
        assert_eq!(path_of("https://hub.example/a/b?c=d#e").unwrap(), "/a/b");
        assert_eq!(path_of("https://hub.example").unwrap(), "/");
        assert!(path_of("not a url").is_err());

        let mut request = Request::get("/");
        let error = vector_identity()
            .sign_request_covering(
                "https://hub.example/a",
                &mut request,
                &[Derived::Path, Derived::Path],
                &[],
                1,
            )
            .expect_err("twice");
        assert_eq!(error, "@path was asked for twice");
        assert!(request.headers.get(SIGNATURE).is_none());
    }

    /// EGR-18. The hub's release route refuses a nonce shorter than 22
    /// characters and a signature window longer than 360 seconds
    /// (`docs/contracts/supplier-credentials.md` §Release and §Signature
    /// components of the release request), and a refusal there clears the
    /// released set. The hub's test pins its two limits against the edge's
    /// numbers; this one fails if the edge's numbers pass the hub's limits.
    #[test]
    fn the_nonce_and_the_window_stay_inside_the_hubs_stated_limits() {
        const HUB_NONCE_MINIMUM_CHARS: usize = 22;
        const HUB_WINDOW_CAP_SECS: i64 = 360;

        let drawn = nonce().expect("nonce");
        assert_eq!(
            drawn.len(),
            URL_SAFE_NO_PAD.encode([0u8; NONCE_BYTES]).len()
        );
        assert!(
            drawn.len() >= HUB_NONCE_MINIMUM_CHARS,
            "a {}-character nonce is shorter than the hub accepts",
            drawn.len()
        );
        assert!(
            drawn
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "unpadded base64url: {drawn}"
        );

        // The window is read from a signed request: what the hub sees, which
        // is [`SIGNATURE_LIFETIME_SECS`] unless the signer stops using it.
        let mut request = Request::get("/");
        vector_identity()
            .sign_request_covering(
                "https://hub.example/api/v1/supplier-credentials",
                &mut request,
                &[Derived::Method, Derived::Path],
                &[],
                1_000,
            )
            .expect("sign");
        let input = request.headers.get(SIGNATURE_INPUT).expect("signed");
        let parameter = |name: &str| {
            let rest = &input[input.find(name).expect(name) + name.len()..];
            rest[..rest.find(';').unwrap_or(rest.len())]
                .trim_matches('"')
                .to_owned()
        };
        assert!(
            parameter(";nonce=").len() >= HUB_NONCE_MINIMUM_CHARS,
            "{input}"
        );
        let window = parameter(";expires=").parse::<i64>().unwrap()
            - parameter(";created=").parse::<i64>().unwrap();
        assert_eq!(window, SIGNATURE_LIFETIME_SECS);
        assert!(window <= HUB_WINDOW_CAP_SECS, "{input}");
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
            enrolment_home: None,
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

    /// An identity already loaded reads the enrolment record again before
    /// each signature, so a revocation, a disconnect or a new enrolment
    /// recorded by another process stops it signing requests, lifecycle
    /// calls and directory proofs, for as long as the record says so.
    #[test]
    fn a_loaded_identity_stops_signing_when_the_record_changes_under_it() {
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
        let identity = Identity::load(home.path()).expect("load");
        let signer = identity.signer().expect("an enrolled key signs");
        let attempts = |signer: &SigningIdentity| {
            let mut request = Request::get("/");
            [
                signer.sign_request("https://example.org/", &mut request, &[], 1_000),
                signer
                    .sign_registration(
                        &LifecycleRequest {
                            method: "GET",
                            target_uri: "https://hub.example/api/v1/instances/i",
                            body: &[],
                            idempotency_key: "check-1",
                        },
                        1_000,
                    )
                    .map(|_| ()),
                signer
                    .directory_proof("hub.example", 1_000, 604_800)
                    .map(|_| ()),
            ]
        };
        assert!(attempts(signer).iter().all(Result::is_ok));

        let learnt = |expected: &str| {
            for attempt in attempts(signer) {
                let reason = attempt.expect_err("no signature");
                assert!(reason.contains(expected), "{reason}");
            }
        };
        record
            .record_refusal(home.path(), "the member was removed")
            .expect("store");
        learnt("revoked by the hub: the member was removed after this session loaded it");

        // A refusal lasts only while the record says so: a withdrawn 401 and
        // a read that failed once leave the same identity signing again.
        assert!(
            record
                .withdraw_refusal(
                    home.path(),
                    chrono::Utc::now() + chrono::Duration::seconds(1)
                )
                .expect("store")
        );
        assert!(attempts(signer).iter().all(Result::is_ok));
        let path = crate::enrolment::EnrolmentRecord::path(home.path());
        let whole = std::fs::read(&path).expect("read");
        std::fs::write(&path, "{").expect("write");
        learnt("not a valid enrolment record");
        learnt("the next one reads it again");
        std::fs::write(&path, &whole).expect("write");
        assert!(attempts(signer).iter().all(Result::is_ok));

        // A revocation the hub stated after the 401 is not withdrawn.
        record
            .record_refusal(home.path(), "the member was removed")
            .expect("store");
        record
            .record_revocation(home.path(), "2026-09-25T09:00:00Z", "owner")
            .expect("store");
        assert!(
            !record
                .withdraw_refusal(
                    home.path(),
                    chrono::Utc::now() + chrono::Duration::seconds(1)
                )
                .expect("store")
        );
        learnt("revoked by the owner after this session loaded it");

        record.revoked_at = None;
        record.revocation = None;
        record.revocation_learnt_at = None;
        record.key_id = "another-key".to_owned();
        record.store(home.path()).expect("store");
        learnt("now enrolled as key another-key");
        std::fs::remove_file(crate::enrolment::EnrolmentRecord::path(home.path())).expect("remove");
        learnt("is gone");
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
