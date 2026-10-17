//! A signed release: text with a keep-intact sidecar, as a source's
//! gatekeeper serves it.
//!
//! This is a working format, not a standard and not Common Measure's to
//! define. The trusted news demo's stand-in gatekeeper defined it; it goes to
//! the open standards bodies as a proposal, and the edge reads it as it
//! stands. The text is the source's words; the sidecar names the passages by
//! id with their byte spans, the keep-intact markers, the terms and the
//! attribution, and carries an Ed25519 signature over the text's hash and the
//! sidecar's signed fields ([`signed_message`]). Verification lives with the
//! processor that performs it (`commonmeasure_runtime::processor::release`).
//!
//! Two optional additions are read where a release carries them: a
//! structured [`Usage`] block among the signed fields, beside the free-text
//! terms, and a C2PA manifest store in the sidecar ([`Sidecar::c2pa`]),
//! signed by the source with its own certificate and bound to the text. A
//! release without them reads, verifies and signs exactly as before
//! (`docs/contracts/display-provenance.md`).

use serde::{Deserialize, Serialize};

use crate::canonical::canonical_json;

/// The media type a gatekeeper serves a release under. Unregistered, in the
/// `x.` tree of RFC 6838 §3.4, because the format is a working one.
pub const MEDIA_TYPE: &str = "application/x.signed-release+json";

/// The one signature scheme the format names.
pub const SCHEME: &str = "ed25519-over-sha256(text)-and-canonical-signed-json";

/// The display modes a release may ask for: what is not confirmed from the
/// text is shown marked as not from the source (`labelled`), or only
/// confirmed text and the source's own words are shown (`strict`).
pub const LABELLED: &str = "labelled";
pub const STRICT: &str = "strict";

/// A passage of the signed text: a byte span of it, named by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passage {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

/// A keep-intact marker in the signed text, with the passage it lies in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptSpan {
    pub start: usize,
    pub end: usize,
    pub label: String,
    pub passage: String,
}

/// The fields the signature covers. None of them carries the text itself.
///
/// A field outside these is refused rather than dropped: the canonical
/// form the signature is checked over is rebuilt from the fields read, so a
/// dropped field would be neither verified nor kept on the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub source: String,
    pub title: String,
    pub revid: u64,
    pub url: String,
    pub licence: String,
    pub attribution: String,
    /// The display terms the source states, in words.
    pub terms: Vec<String>,
    /// [`LABELLED`] or [`STRICT`]. Strictness belongs to the source, so it
    /// is a signed term.
    #[serde(default = "default_display_mode")]
    pub display_mode: String,
    /// Lower-case hex SHA-256 of the text's UTF-8 bytes.
    pub text_sha256: String,
    pub passages: Vec<Passage>,
    pub markers: Vec<KeptSpan>,
    pub released_at: String,
    /// What the source allows of the text in words a reader can rule on,
    /// beside the free-text `terms`. Absent in a release that states none,
    /// and then absent from the canonical form the signature covers, so a
    /// release signed before the field existed verifies unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A source's statement of use, named in RSL 1.0 terms where RSL has the
/// word (`docs/contracts/display-provenance.md` §Terms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    /// The text as input to a model: grounding, summaries, answers. RSL
    /// `ai-input`.
    pub ai_input: UsageTerm,
    /// The text, or a product derived from it, as part of a new product
    /// sold onwards. RSL names commercial use as a user type; there is no
    /// usage token for it.
    pub onward_commercial_product: UsageTerm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting: Option<UsageReporting>,
}

/// One term of a [`Usage`] block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTerm {
    Allow,
    Disallow,
}

/// Whether the source requires use of the text to be reported, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageReporting {
    pub required: bool,
    /// The reporting destination the source names: a URL or its own
    /// reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

/// The signed fields and the signature over them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub signed: Signed,
    /// Ed25519 over [`signed_message`], standard base64. Empty where the
    /// release carries none.
    #[serde(default)]
    pub signature: String,
    /// The signer's Ed25519 public key, standard base64. Empty where the
    /// release names none.
    #[serde(default)]
    pub signer_public_key: String,
    pub signature_scheme: String,
    /// A C2PA manifest store, standard base64, whose hard binding is
    /// `c2pa.hash.data` over the NFC-normalised text, signed by the source
    /// with its own certificate. Outside the Ed25519-signed fields: the
    /// manifest carries its own signature, and a release whose store does
    /// not verify is still the release its Ed25519 signature admits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c2pa: Option<String>,
}

/// A release as served: the text and its sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub text: String,
    pub sidecar: Sidecar,
}

/// What a verifier established about a release's signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    /// The text matches its signed hash and the signature verifies under
    /// the key the sidecar names.
    Verified,
    /// A signature and key are present and the check failed: the text does
    /// not match its hash, the scheme is not the one named here, a value is
    /// malformed or the signature does not verify.
    Failed,
    /// The sidecar carries no signature or names no key.
    Missing,
}

/// A release as a crossing record keeps it: the sidecar whole, which holds
/// the text's hash and never the text, and what verification established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub media_type: String,
    pub verification: VerificationState,
    /// Why verification did not establish `verified`, in the verifier's
    /// words; absent where it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub sidecar: Sidecar,
    /// What reading the sidecar's C2PA store established, where the
    /// sidecar carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c2pa: Option<C2paRecord>,
}

/// What a C2PA store a release carries was found to be, read offline
/// against the release text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum C2paState {
    /// The store does not decode, parse or verify against the text.
    Invalid,
    /// The signature and the binding verify; the signer chains to no
    /// configured trust anchor.
    Untrusted,
    /// The signature and the binding verify and the signer chains to a
    /// configured trust anchor.
    Trusted,
}

/// The C2PA reading of a release's store as the crossing record keeps it:
/// the state, the binding and who signed, never any text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C2paRecord {
    pub state: C2paState,
    /// Why the store is `invalid`, in the reader's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// SHA-256 of the decoded store bytes, `sha256:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_label: Option<String>,
    /// Whether the store's data hash matches the NFC text, and whether
    /// that text is the text `text_sha256` names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<C2paBinding>,
    /// The claim signer's certificate subject and issuer, as the reader
    /// reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<serde_json::Value>,
    /// Each `cawg.identity` assertion, summarised: the credential kind and
    /// the subject it names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<serde_json::Value>,
    /// The `cawg.training-mining` assertion as the source signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub training_mining: Option<serde_json::Value>,
    /// Every validation status code the reader reported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,
    pub verifier: String,
}

/// The binding of a release's C2PA store to its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C2paBinding {
    /// The data hash matches the NFC-normalised release text.
    pub matches: bool,
    /// The NFC-normalised text is byte for byte the text `text_sha256`
    /// names, so the binding and the Ed25519 signature are over one text.
    pub same_text_as_signed: bool,
}

fn default_display_mode() -> String {
    LABELLED.to_owned()
}

/// The bytes the signature covers: the text's hex SHA-256, a newline, and
/// the canonical JSON of the signed fields.
pub fn signed_message(signed: &Signed) -> Vec<u8> {
    let mut message = signed.text_sha256.as_bytes().to_vec();
    message.push(b'\n');
    let value = serde_json::to_value(signed).expect("the signed fields serialise");
    message.extend(canonical_json(&value).into_bytes());
    message
}

impl Release {
    /// A passage by id, with its text, where its span lies in the text.
    pub fn passage(&self, id: &str) -> Option<(&Passage, &str)> {
        let passage = self.sidecar.signed.passages.iter().find(|p| p.id == id)?;
        Some((passage, self.text.get(passage.start..passage.end)?))
    }
}

/// Whether a `Content-Type` value names [`MEDIA_TYPE`]: the media type
/// before any parameter, case-folded.
pub fn is_release(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(MEDIA_TYPE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_type_is_matched_without_its_parameters_and_case() {
        assert!(is_release("application/x.signed-release+json"));
        assert!(is_release(
            "Application/X.Signed-Release+JSON; charset=utf-8"
        ));
        assert!(!is_release("application/json"));
    }

    #[test]
    fn a_missing_display_mode_reads_as_labelled_and_a_missing_signature_as_empty() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "text": "t",
            "sidecar": {
                "signed": {
                    "source": "s", "title": "t", "revid": 1, "url": "u", "licence": "l",
                    "attribution": "a", "terms": [], "text_sha256": "h",
                    "passages": [], "markers": [], "released_at": "r"
                },
                "signature_scheme": SCHEME
            }
        }))
        .unwrap();
        assert_eq!(release.sidecar.signed.display_mode, LABELLED);
        assert!(release.sidecar.signature.is_empty());
        assert!(release.sidecar.signer_public_key.is_empty());
    }
}
