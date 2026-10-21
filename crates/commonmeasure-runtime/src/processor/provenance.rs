//! Output provenance: an `attest`-stage processor that builds a C2PA manifest
//! for a plan's answer from the sealed source record, signs it with the
//! operator's certificate and embeds it in the answer text under C2PA 2.4
//! Annex A.8 (invisible Unicode variation selectors appended after the text,
//! which survive copy and paste). A second process can read the manifest
//! back from the pasted output with [`read_back`].
//!
//! The manifest carries hashes and identifiers only. Each source that entered
//! the model's window is an ingredient referenced by the SHA-256 of the text
//! that entered, with the grade its crossing was recorded at, so a mediated
//! ruling and an observed witness are never one claim. The answer is bound by
//! a data hash and named by its hash in the source-record assertion; no
//! prompt, context or answer text is inside the manifest.
//!
//! The default local signing identity is the operator's. Without a certificate the
//! invocation is recorded as unavailable and the answer is left unlabelled;
//! the manifest that would have been signed is still in the record, so the
//! claim is inspectable even when it could not be attested. Local signing readback
//! configures no trust list, so a well-formed signature validates as valid but
//! untrusted. Independent readers may supply anchors with [`read_back_with_trust`].
//! The optional [`encypher`] adapter requires explicit disclosure and trusted
//! readback, and identifies its service as the claim signer.

use commonmeasure_types::canonical::sha256_digest;
use std::io::Cursor;
use std::sync::LazyLock;

use c2pa::assertions::DataHash;
use c2pa::{BoxedSigner, Builder, Context, HashRange, Reader, Signer, SigningAlg};
use chrono::Utc;
use commonmeasure_types::{Decision, Gap, GapReason};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

pub mod encypher;

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, Permissions, ProcessorManifest, Stage,
};

pub const NAME: &str = "output-provenance";
pub const VERSION: &str = "1";

/// Names the PEM file holding the operator's signing certificate chain: the
/// end-entity certificate first, then its issuers.
pub const CERTIFICATE_VARIABLE: &str = "COMMONMEASURE_PROVENANCE_CERTIFICATE";
/// Names the PEM file holding the matching PKCS#8 private key (ECDSA P-256).
pub const KEY_VARIABLE: &str = "COMMONMEASURE_PROVENANCE_KEY";

/// Names a PEM bundle of trust anchors the operator supplies for reading
/// C2PA manifests back: a release's own store, and a display's manifest at
/// write time. Unset, a manifest that verifies is `valid` and untrusted.
pub const TRUST_ANCHORS_VARIABLE: &str = "COMMONMEASURE_PROVENANCE_TRUST_ANCHORS";

/// The trust anchors [`TRUST_ANCHORS_VARIABLE`] names: `Ok(None)` where it
/// is unset, the bundle where the file reads as UTF-8, and otherwise why
/// not. A configured bundle that cannot be read is never taken as no bundle
/// configured: the caller records the reason with
/// [`trust_anchors_unavailable`]. A bundle that reads but does not parse is
/// refused by the reader, which records the store as invalid with the
/// reason.
pub fn trust_anchors_from_environment() -> Result<Option<String>, String> {
    let Some(path) = std::env::var_os(TRUST_ANCHORS_VARIABLE) else {
        return Ok(None);
    };
    std::fs::read_to_string(&path).map(Some).map_err(|error| {
        format!(
            "{TRUST_ANCHORS_VARIABLE} names {}, which cannot be read as a PEM bundle ({error})",
            std::path::Path::new(&path).display()
        )
    })
}

/// Record on `invocation` that the operator configured trust anchors that
/// could not be read (`reason`, from [`trust_anchors_from_environment`]):
/// a gap naming them, and, where the invocation read a manifest back, its
/// trust list stated as unavailable rather than unset. The manifest was
/// read without anchors, so it is never `trusted`.
pub fn trust_anchors_unavailable(invocation: &mut Invocation, reason: &str) {
    if let Some(validation) = invocation
        .detail
        .get_mut("validation")
        .and_then(Value::as_object_mut)
    {
        validation.insert(
            "trust_list".to_owned(),
            json!("configured trust anchors unavailable"),
        );
    }
    invocation.gaps.push(Gap::new(
        GapReason::CapabilityUnavailable,
        format!(
            "trust anchors unavailable: {reason}; manifests were read without trust anchors, \
             so none is trusted."
        ),
    ));
}

/// The assertion carrying the source record's identifiers and hashes.
pub const SOURCE_RECORD_LABEL: &str = "ai.commonmeasure.source-record";
/// The CAWG training and data mining assertion (version 1.1).
pub const TRAINING_MINING_LABEL: &str = "cawg.training-mining";
/// The role the operator holds in the identity assertion: the party
/// accountable for producing the output.
pub const OPERATOR_ROLE: &str = "cawg.producer";
/// The digital source type of a model's answer.
pub const DIGITAL_SOURCE_TYPE: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia";
/// The embedding method the record names.
pub const EMBEDDING: &str = "c2pa-2.4-annex-a8-variation-selectors";

const TEXT_FORMAT: &str = "text/plain";
const DATA_HASH_NAME: &str = "commonmeasure text binding";
/// The SDK composes a raw manifest store under this format name; text has no
/// container of its own, and the wrapper is the container.
const RAW_MANIFEST_FORMAT: &str = "c2pa";
/// The wrapper length depends on the manifest length, which depends on the
/// data-hash exclusion that states the wrapper length. The fixed point is
/// reached in two rounds unless an integer changes width in CBOR.
const SIGNING_ROUNDS: usize = 4;
/// Headroom over the certificate chain for the COSE signature itself: the
/// ECDSA signature, the protected header and the CBOR framing. The SDK's own
/// reserve allows for a timestamp response that is never requested, and an
/// unrequested allowance would be padding in every labelled answer.
const SIGNATURE_HEADROOM: usize = 1024;

/// The rule set, stated canonically; the configuration digest covers it.
const RULES: &str = "\
output-provenance/1: builds one C2PA manifest per answered plan from the sealed source \
record, signs it with the operator's certificate and embeds it in the answer under C2PA \
2.4 Annex A.8.\n\
ingredients: one c2pa.ingredient per source that entered the model's window, related as \
inputTo, titled by the source reference, its instance id the SHA-256 of the text that \
entered the window, and its description the grade the crossing was \
recorded at (mediated, observed or reconstructed). A ruling and a witness are never one \
ingredient.\n\
actions: one c2pa.created action whose digital source type is trainedAlgorithmicMedia \
and whose software agent is this binary.\n\
source record: an ai.commonmeasure.source-record assertion carrying the run id, the plan \
id, the run manifest hash, the SHA-256 of the answer as recorded, and per source the \
reference, content hash and grade. No prompt, context or answer text enters the \
manifest.\n\
training and data mining: a cawg.training-mining assertion whose four entries are the \
operator's declaration in the suite, verbatim, with constraint_info where declared.\n\
identity: a cawg.identity assertion (cawg.x509.cose) signed with the operator's \
certificate under the role cawg.producer, referencing the actions and source record \
assertions.\n\
binding: a c2pa.hash.data assertion over the NFC-normalised answer bytes, excluding the \
appended wrapper; the wrapper is padded to the length the exclusion declares.\n\
signing: the certificate chain and PKCS#8 key the operator names in \
COMMONMEASURE_PROVENANCE_CERTIFICATE and COMMONMEASURE_PROVENANCE_KEY, ES256. Without \
them the invocation is unavailable, the answer is left unlabelled and the gap names the \
variables.\n\
validation: the labelled text is read back before it is recorded; the record carries the \
validation state and every status code. No trust list is configured, so a well-formed \
signature is valid but untrusted, and the record says so.\n";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Attest,
    capability: "output-provenance-label",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: Permissions {
        network: false,
        filesystem: false,
        model: false,
        // The operator's private key is handed to the processor as bytes;
        // it never reads the key from disk itself.
        credentials: true,
        // It hashes the answer text to bind it. The manifest it writes
        // carries the hash, not the text.
        raw_content: true,
    },
    // The assertions are a pure function of the inputs; the signed bytes are
    // not, because the manifest label is a fresh UUID and ECDSA signatures
    // are randomised. Two invocations over one record agree on every claim
    // and differ in every byte.
    determinism: Determinism::NonDeterministic,
    limits: "in-process and synchronous; one signing pass, no supervisor timeout",
    failure_by_mode: FailureByMode {
        // A label is a claim about the answer, not a gate on it. An answer
        // that could not be labelled stands, with a gap saying why.
        strict: FailBehaviour::FailOpen,
        prefer: FailBehaviour::FailOpen,
        observe: FailBehaviour::FailOpen,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "The signature proves the manifest was signed with the configured key. Whether that \
     key belongs to the operator is a trust-list question this edge does not answer, so \
     every label validates as untrusted until a reader holds the operator's certificate \
     on a trust list.",
    "The Annex A.8 wrapper survives copy and paste but not every editor: one that strips \
     variation selectors removes the label and leaves the visible text unchanged.",
    "Ingredients are referenced by hash. The source bytes are not carried and cannot be \
     recovered from the manifest; the run directory holds them.",
];

/// How a source's crossing was recorded (`docs/GLOSSARY.md` §Grade).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// Carried by the product under policy before the model saw it.
    Mediated,
    /// Seen by a hook after it had already happened.
    Observed,
    /// Read back from a transcript with nothing watching at the time.
    Reconstructed,
}

impl Grade {
    fn as_str(self) -> &'static str {
        match self {
            Grade::Mediated => "mediated",
            Grade::Observed => "observed",
            Grade::Reconstructed => "reconstructed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "mediated" => Some(Grade::Mediated),
            "observed" => Some(Grade::Observed),
            "reconstructed" => Some(Grade::Reconstructed),
            _ => None,
        }
    }
}

/// One source that entered the model's window, as the manifest names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub reference: String,
    /// SHA-256 of the text that entered the window, `sha256:<hex>`.
    pub content_hash: String,
    pub grade: Grade,
}

/// One entry of the training and data mining assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Use {
    Allowed,
    NotAllowed,
    /// Allowed under the terms `constraint_info` points at. A reader without
    /// more information treats it as not allowed.
    Constrained,
}

impl Use {
    /// The value as the CAWG assertion spells it.
    fn wire(self) -> &'static str {
        match self {
            Use::Allowed => "allowed",
            Use::NotAllowed => "notAllowed",
            Use::Constrained => "constrained",
        }
    }
}

/// The operator's declaration of what may be done with its outputs: the
/// four entries of the CAWG training and data mining assertion 1.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingMining {
    pub ai_inference: Use,
    pub ai_training: Use,
    pub ai_generative_training: Use,
    pub data_mining: Use,
    /// Where the terms of a `constrained` entry are stated; the
    /// specification allows a URL to a policy document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint_info: Option<String>,
}

impl TrainingMining {
    fn entries(&self) -> [(&'static str, Use); 4] {
        [
            ("cawg.ai_inference", self.ai_inference),
            ("cawg.ai_training", self.ai_training),
            ("cawg.ai_generative_training", self.ai_generative_training),
            ("cawg.data_mining", self.data_mining),
        ]
    }

    fn assertion(&self) -> Value {
        let mut entries = serde_json::Map::new();
        for (label, r#use) in self.entries() {
            let mut entry = json!({"use": r#use.wire()});
            if r#use == Use::Constrained
                && let Some(info) = &self.constraint_info
            {
                entry["constraint_info"] = json!(info);
            }
            entries.insert(label.to_owned(), entry);
        }
        json!({"entries": entries})
    }
}

/// The suite's opt-in: the operator's declaration for its own outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProvenance {
    pub training_mining: TrainingMining,
    /// Explicit permission for the named remote signer to receive this
    /// answer and its source references, hashes, grades and run identifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encypher: Option<encypher::Disclosure>,
}

impl OutputProvenance {
    /// A `constrained` entry with nothing to point at is read as not allowed
    /// by every conforming reader, so it is refused here rather than
    /// declared as something it is not.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(disclosure) = &self.encypher
            && !disclosure.send_answer_and_source_record
        {
            return Err("Encypher signing requires send_answer_and_source_record: true".to_owned());
        }
        let constrained = self
            .training_mining
            .entries()
            .iter()
            .any(|(_, r#use)| *r#use == Use::Constrained);
        if constrained && self.training_mining.constraint_info.is_none() {
            return Err(
                "output_provenance declares a constrained training-and-data-mining \
                        entry without constraint_info; a constraint with no stated terms \
                        is read as not allowed, so declare the terms or declare \
                        not_allowed"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// The operator's signing certificate chain and key, as bytes the caller
/// read. The references name where they came from and go into the record;
/// the bytes never do.
#[derive(Clone)]
pub struct SigningMaterial {
    pub certificate_chain: Vec<u8>,
    pub private_key: Vec<u8>,
    pub certificate_ref: String,
    pub key_ref: String,
}

/// Prints where the material came from and how long the key is. The key
/// bytes are never formatted, so no error path can print them.
impl std::fmt::Debug for SigningMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningMaterial")
            .field("certificate_ref", &self.certificate_ref)
            .field("certificate_chain_bytes", &self.certificate_chain.len())
            .field("key_ref", &self.key_ref)
            .field("private_key_bytes", &self.private_key.len())
            .finish()
    }
}

/// What the run holds by way of a signing identity.
#[derive(Debug, Clone)]
pub enum SigningIdentity {
    /// Neither variable is set. The ordinary state of a machine that has not
    /// been given a certificate.
    Unconfigured,
    Configured(SigningMaterial),
    /// Remote credentials are held privately and never formatted into evidence.
    Encypher(encypher::SigningConfig),
    /// A variable is set and the file it names cannot be used. Recorded as
    /// a gap naming the file, never treated as unconfigured.
    Unusable {
        reference: String,
        reason: String,
    },
}

impl SigningIdentity {
    /// Read the certificate chain and key the environment names.
    pub fn from_environment() -> Self {
        match std::env::var("COMMONMEASURE_PROVENANCE_SIGNER").as_deref() {
            Ok("encypher") => {
                return match encypher::SigningConfig::from_environment() {
                    Ok(config) => Self::Encypher(config),
                    Err(reason) => Self::Unusable {
                        reference: "Encypher signing configuration".to_owned(),
                        reason,
                    },
                };
            }
            Ok("local") | Err(std::env::VarError::NotPresent) => {}
            Err(std::env::VarError::NotUnicode(_)) => {
                return Self::Unusable {
                    reference: "COMMONMEASURE_PROVENANCE_SIGNER".to_owned(),
                    reason: "signer selection is not UTF-8".to_owned(),
                };
            }
            Ok(_) => {
                return Self::Unusable {
                    reference: "COMMONMEASURE_PROVENANCE_SIGNER".to_owned(),
                    reason: "expected local or encypher".to_owned(),
                };
            }
        }
        let certificate = std::env::var(CERTIFICATE_VARIABLE).ok();
        let key = std::env::var(KEY_VARIABLE).ok();
        match (certificate, key) {
            (None, None) => Self::Unconfigured,
            (Some(certificate), Some(key)) => {
                let certificate_chain = match std::fs::read(&certificate) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return Self::Unusable {
                            reference: certificate,
                            reason: format!("{CERTIFICATE_VARIABLE} cannot be read: {error}"),
                        };
                    }
                };
                let private_key = match std::fs::read(&key) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return Self::Unusable {
                            reference: key,
                            reason: format!("{KEY_VARIABLE} cannot be read: {error}"),
                        };
                    }
                };
                Self::Configured(SigningMaterial {
                    certificate_chain,
                    private_key,
                    certificate_ref: certificate,
                    key_ref: key,
                })
            }
            (Some(certificate), None) => Self::Unusable {
                reference: certificate,
                reason: format!("{CERTIFICATE_VARIABLE} is set but {KEY_VARIABLE} is not"),
            },
            (None, Some(key)) => Self::Unusable {
                reference: key,
                reason: format!("{KEY_VARIABLE} is set but {CERTIFICATE_VARIABLE} is not"),
            },
        }
    }
}

/// Everything one invocation reads.
#[derive(Debug)]
pub struct Input<'a> {
    pub run_id: &'a str,
    pub plan_id: &'a str,
    /// The run manifest's hash: the operator's own manifest reference.
    pub run_manifest_hash: &'a str,
    /// The answer as the plan records it.
    pub answer: &'a str,
    /// Where the answer is recorded, for the input artefact reference.
    pub answer_reference: &'a str,
    /// The sources that entered the model's window, in window order.
    pub sources: &'a [Source],
    pub policy: &'a OutputProvenance,
    pub signing: &'a SigningIdentity,
    /// Where the caller will publish the labelled text and the raw manifest
    /// store; the record names them before they are written.
    pub labelled_reference: &'a str,
    pub manifest_reference: &'a str,
}

/// The labelled output: the answer with the manifest embedded, and the raw
/// manifest store on its own.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    pub manifest: Vec<u8>,
}

/// The manifest definition the SDK signs: a pure function of the record, so
/// a reader can rebuild it from the plan and compare it with what the
/// labelled output carries.
pub fn definition(input: &Input) -> Value {
    let ingredients: Vec<Value> = input
        .sources
        .iter()
        .map(|source| {
            json!({
                "title": source.reference,
                "format": TEXT_FORMAT,
                "relationship": "inputTo",
                "instance_id": source.content_hash,
                "description": source.grade.as_str(),
            })
        })
        .collect();
    json!({
        "claim_generator_info": [{
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }],
        "title": format!("Common Measure run {} plan {}", input.run_id, input.plan_id),
        "format": TEXT_FORMAT,
        "ingredients": ingredients,
        "assertions": [
            {
                "label": "c2pa.actions",
                "data": {"actions": [{
                    "action": "c2pa.created",
                    "digitalSourceType": DIGITAL_SOURCE_TYPE,
                    "softwareAgent": {
                        "name": env!("CARGO_PKG_NAME"),
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }]},
            },
            {
                "label": TRAINING_MINING_LABEL,
                "data": input.policy.training_mining.assertion(),
            },
            {
                "label": SOURCE_RECORD_LABEL,
                "data": source_record(input),
            },
        ],
    })
}

/// The source-record assertion: identifiers and hashes, nothing else.
pub fn source_record(input: &Input) -> Value {
    json!({
        "run_id": input.run_id,
        "plan_id": input.plan_id,
        "run_manifest_hash": input.run_manifest_hash,
        "answer_hash": sha256_digest(input.answer.as_bytes()),
        "sources": input.sources,
    })
}

/// Build, sign, embed and read back one plan's label.
pub fn invoke(input: &Input) -> (Invocation, Option<Label>) {
    if input.policy.encypher.is_some() {
        return encypher::invoke(input);
    }
    let started_at = Utc::now();
    let definition = definition(input);
    let mut inputs = vec![ArtefactRef {
        reference: input.answer_reference.to_owned(),
        content_hash: Some(sha256_digest(input.answer.as_bytes())),
        tokens: None,
    }];
    inputs.extend(input.sources.iter().map(|source| ArtefactRef {
        reference: source.reference.clone(),
        content_hash: Some(source.content_hash.clone()),
        tokens: None,
    }));

    let material = match input.signing {
        SigningIdentity::Configured(material) => material,
        SigningIdentity::Encypher(_) => {
            return (
                unavailable(
                    started_at,
                    inputs,
                    definition,
                    "The suite has not authorised Encypher disclosure; no request was sent"
                        .to_owned(),
                    json!({"basis": "remote disclosure not authorised"}),
                ),
                None,
            );
        }
        SigningIdentity::Unconfigured => {
            return (
                unavailable(
                    started_at,
                    inputs,
                    definition,
                    format!(
                        "No signing identity is configured. Set {CERTIFICATE_VARIABLE} to \
                         the operator's certificate chain and {KEY_VARIABLE} to its \
                         PKCS#8 key; the answer stands unlabelled."
                    ),
                    json!({"basis": "unconfigured"}),
                ),
                None,
            );
        }
        SigningIdentity::Unusable { reference, reason } => {
            return (
                unavailable(
                    started_at,
                    inputs,
                    definition,
                    format!(
                        "The configured signing identity cannot be used ({reason}); the \
                         answer stands unlabelled."
                    ),
                    json!({"basis": "unusable", "reference": reference}),
                ),
                None,
            );
        }
    };
    let signing_detail = json!({
        "basis": "operator-supplied certificate chain and key",
        "certificate": material.certificate_ref,
        "key": material.key_ref,
        "alg": "ES256",
    });

    let signed = match sign(&definition, input.answer, material) {
        Ok(signed) => signed,
        Err(reason) => {
            return (
                unavailable(
                    started_at,
                    inputs,
                    definition,
                    format!(
                        "The manifest could not be signed ({reason}); the answer stands \
                             unlabelled."
                    ),
                    signing_detail,
                ),
                None,
            );
        }
    };

    // Reading the label back is the only check that the bytes this
    // invocation publishes carry the claim it made; the record carries what
    // the reader found, not what the writer intended.
    let read = match read_back(&signed.text) {
        Ok(read) => read,
        Err(error) => {
            return (
                unavailable(
                    started_at,
                    inputs,
                    definition,
                    format!(
                        "The label was written but could not be read back ({error}); \
                             the answer stands unlabelled."
                    ),
                    signing_detail,
                ),
                None,
            );
        }
    };
    let outputs = vec![
        ArtefactRef {
            reference: input.labelled_reference.to_owned(),
            content_hash: Some(sha256_digest(signed.text.as_bytes())),
            tokens: None,
        },
        ArtefactRef {
            reference: input.manifest_reference.to_owned(),
            content_hash: Some(sha256_digest(&signed.manifest)),
            tokens: None,
        },
    ];
    let detail = json!({
        "manifest_definition": definition,
        "manifest_label": read.manifest_label,
        "manifest_bytes": signed.manifest.len(),
        "signing": signing_detail,
        "signature": read.signature,
        "validation": read.validation,
        "embedding": {
            "method": EMBEDDING,
            "normalisation": "NFC",
            "normalised_hash": signed.normalised_hash,
            "normalisation_changed_bytes": signed.normalisation_changed_bytes,
            "text_bytes": signed.text_bytes,
            "wrapper_bytes": signed.wrapper_bytes,
        },
    });
    let invocation = Invocation::new(
        manifest(),
        started_at,
        Decision::Admit,
        format!(
            "C2PA manifest built from the sealed source record ({} ingredient{} by content \
             hash and grade), signed with the operator's certificate, embedded under \
             Annex A.8 and read back: {}",
            input.sources.len(),
            if input.sources.len() == 1 { "" } else { "s" },
            read.validation["summary"].as_str().unwrap_or("unread"),
        ),
        inputs,
        outputs,
        detail,
        Vec::new(),
        BLIND_SPOTS.to_vec(),
    );
    (
        invocation,
        Some(Label {
            text: signed.text,
            manifest: signed.manifest,
        }),
    )
}

/// The terms assertion: what each ingredient was admitted under and the
/// statement derived for the display (`docs/contracts/display-provenance.md`).
/// A working format offered to the open standards bodies, not a standard.
pub const TERMS_LABEL: &str = "ai.commonmeasure.terms";
/// Where a `constrained` training-and-mining entry of a display manifest
/// points: the terms assertion in the same manifest.
pub const TERMS_URI: &str = "self#jumbf=c2pa.assertions/ai.commonmeasure.terms";
/// The ingredient metadata key holding the digest of the release's sidecar
/// as served, so the signed fields are bound beside the text.
pub const SIDECAR_DIGEST_KEY: &str = "ai.commonmeasure.sidecar_digest";
/// The media type of a display ingredient: a signed release.
const RELEASE_FORMAT: &str = "application/x.signed-release+json";
/// The media type of a C2PA manifest store resource.
const MANIFEST_STORE_FORMAT: &str = "application/c2pa";

/// The rule set of the display entry point, stated canonically; the
/// configuration digest of a display invocation covers it.
const DISPLAY_RULES: &str = "\
output-provenance/1 display: builds one C2PA manifest per rendered display in the session \
path, signs it with the configured certificate and writes the manifest store to a sidecar \
beside the session log; nothing is embedded in the display text.\n\
ingredients: one c2pa.ingredient per signed release the display was rendered against, \
related as inputTo, titled by the release URL, format application/x.signed-release+json, \
its instance id the SHA-256 of the release text, its description the grade the crossing \
was recorded at, and its metadata the digest of the sidecar as served. Where the release \
carried a C2PA store that verified, the ingredient carries that store.\n\
actions: one c2pa.created action whose digital source type is trainedAlgorithmicMedia \
and whose software agent is this binary.\n\
identity: none. No cawg.identity assertion is added; the certificate names the claim \
generator, and nothing names the reader.\n\
terms: an ai.commonmeasure.terms assertion carrying per ingredient its licence, the \
source's terms, the crossing's declarations, the operator's terms reference for the host \
and the use derived from them, and the statement derived for the display: for each of \
ai_input and onward_commercial_product, disallow if any ingredient disallows, else unknown \
if any is unknown, else allow; reporting required if any ingredient requires it, with \
every destination. No ingredient makes every entry unknown.\n\
training and data mining: a cawg.training-mining assertion whose ai_inference entry is the \
derived ai_input and whose other entries combine the ingredients' declarations the same \
way; allow is allowed, disallow notAllowed, unknown constrained with constraint_info \
pointing at the terms assertion.\n\
binding: a c2pa.hash.data assertion over the NFC-normalised display text, no exclusion.\n\
signing: the certificate chain and PKCS#8 key named in COMMONMEASURE_PROVENANCE_CERTIFICATE \
and COMMONMEASURE_PROVENANCE_KEY, ES256. Without them the invocation is unavailable, no \
sidecar is written and the gap names the variables; the display is rendered regardless.\n\
validation: the store is read back against the display text before it is recorded, with \
the configured trust anchors where supplied; the record carries the state and every \
status code.\n";

static DISPLAY_MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    configuration_digest: sha256_digest(DISPLAY_RULES.as_bytes()),
    ..MANIFEST.clone()
});

/// The processor manifest a display invocation is recorded under: the same
/// processor, its configuration digest over the display rules.
pub fn display_manifest() -> &'static ProcessorManifest {
    &DISPLAY_MANIFEST
}

const DISPLAY_BLIND_SPOTS: &[&str] = &[
    "The signature proves the manifest was signed with the configured key. The \
     certificate names a claim generator, and whether it belongs to this edge is a \
     trust-list question this edge does not answer.",
    "The manifest is a sidecar beside the session log. The display text the host shows \
     carries no label, so a reader holding only the text cannot find the manifest.",
    "The terms are the sources' statements and the operator's references as recorded; \
     the derivation combines them and does not establish that any is accurate or binding.",
];

/// Whether a use is allowed, as one source states it or as a derivation
/// over several finds it. `Unknown` is never read as permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Allow,
    Disallow,
    Unknown,
}

impl Permission {
    /// The most restrictive of `permissions`: disallow beats unknown beats
    /// allow, and nothing to go on is unknown.
    pub fn most_restrictive(permissions: impl IntoIterator<Item = Permission>) -> Permission {
        let mut combined = None;
        for permission in permissions {
            combined = Some(match (combined, permission) {
                (_, Permission::Disallow) | (Some(Permission::Disallow), _) => Permission::Disallow,
                (_, Permission::Unknown) | (Some(Permission::Unknown), _) => Permission::Unknown,
                _ => Permission::Allow,
            });
        }
        combined.unwrap_or(Permission::Unknown)
    }

    /// What one source's statements say together, each `None` where a
    /// channel states nothing: disallow where any statement disallows,
    /// allow where one allows and none disallows, unknown where none
    /// states anything. An absent statement adds nothing, unlike an
    /// ingredient that states nothing in [`Usage::derive`].
    pub fn stated(statements: impl IntoIterator<Item = Option<Permission>>) -> Permission {
        let mut combined = Permission::Unknown;
        for statement in statements.into_iter().flatten() {
            match statement {
                Permission::Disallow => return Permission::Disallow,
                Permission::Allow => combined = Permission::Allow,
                Permission::Unknown => {}
            }
        }
        combined
    }

    /// The CAWG training-and-mining value: unknown is `constrained`, with
    /// the terms assertion as its constraint.
    fn cawg(self) -> Use {
        match self {
            Permission::Allow => Use::Allowed,
            Permission::Disallow => Use::NotAllowed,
            Permission::Unknown => Use::Constrained,
        }
    }
}

/// Whether use must be reported, and to where.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reporting {
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<String>,
}

/// The use one ingredient was admitted under, or the use derived for the
/// display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub ai_input: Permission,
    pub onward_commercial_product: Permission,
    pub reporting: Reporting,
}

impl Usage {
    /// The most restrictive of every ingredient's use: each permission as
    /// [`Permission::most_restrictive`], reporting required where any
    /// requires it, every destination listed once.
    pub fn derive<'a>(usages: impl IntoIterator<Item = &'a Usage> + Clone) -> Usage {
        let mut destinations: Vec<String> = Vec::new();
        for usage in usages.clone() {
            for destination in &usage.reporting.destinations {
                if !destinations.contains(destination) {
                    destinations.push(destination.clone());
                }
            }
        }
        Usage {
            ai_input: Permission::most_restrictive(usages.clone().into_iter().map(|u| u.ai_input)),
            onward_commercial_product: Permission::most_restrictive(
                usages
                    .clone()
                    .into_iter()
                    .map(|u| u.onward_commercial_product),
            ),
            reporting: Reporting {
                required: usages.into_iter().any(|u| u.reporting.required),
                destinations,
            },
        }
    }
}

/// What an ingredient's declarations say of the three CAWG uses beyond
/// inference, which no release term names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredMining {
    pub ai_training: Permission,
    pub ai_generative_training: Permission,
    pub data_mining: Permission,
}

/// The terms one ingredient was admitted under, as recorded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IngredientTerms {
    /// The release's own `licence`.
    pub licence: String,
    /// The release's `usage` block and free-text `terms`.
    pub source_terms: Value,
    /// The crossing's declarations: the effective answer per category, the
    /// RSL licences read and the `Content-Usage` header.
    pub declarations: Value,
    /// The operator's terms reference for the host, where declared: its
    /// reference and whether it requires reporting. Never the institution
    /// identifiers or the Anchor.
    pub host_policy_terms: Option<Value>,
    /// The use these terms add up to.
    pub usage: Usage,
    pub mining: DeclaredMining,
}

/// A C2PA store a release carried, and what reading it against the release
/// text established.
#[derive(Debug, Clone)]
pub struct SourceManifest {
    pub store: Vec<u8>,
    /// The state the release verifier recorded for it on the way in.
    pub state: String,
    /// The reader's validation results for the store
    /// ([`StoreReading::validation_results`]). C2PA requires them beside an
    /// ingredient that carries its own manifest.
    pub validation_results: Value,
}

/// One first-order product a display was rendered against.
#[derive(Debug, Clone)]
pub struct DisplayIngredient {
    /// The URL the release was served at.
    pub reference: String,
    /// SHA-256 of the release text, `sha256:<hex>`.
    pub content_hash: String,
    pub grade: Grade,
    /// The canonical digest of the sidecar as served.
    pub sidecar_digest: String,
    /// The release's own C2PA store, where it carried one that verified.
    pub source_manifest: Option<SourceManifest>,
    pub terms: IngredientTerms,
}

/// Everything one display invocation reads.
#[derive(Debug)]
pub struct DisplayInput<'a> {
    /// The display's plain text exactly as the host receives it.
    pub text: &'a str,
    /// Where the display is recorded, for the input artefact reference.
    pub display_reference: &'a str,
    pub ingredients: &'a [DisplayIngredient],
    pub signing: &'a SigningIdentity,
    /// Where the caller will write the manifest store; the record names it
    /// before it is written.
    pub manifest_reference: &'a str,
    /// A PEM bundle of trust anchors the operator supplied, for read-back.
    pub trust_anchors: Option<&'a str>,
}

/// The display's derived use and the CAWG training-and-mining entries.
fn display_terms(input: &DisplayInput) -> (Value, TrainingMining) {
    let derived = Usage::derive(input.ingredients.iter().map(|i| &i.terms.usage));
    let mining = |pick: fn(&DeclaredMining) -> Permission| {
        Permission::most_restrictive(input.ingredients.iter().map(|i| pick(&i.terms.mining)))
    };
    let training = TrainingMining {
        ai_inference: derived.ai_input.cawg(),
        ai_training: mining(|m| m.ai_training).cawg(),
        ai_generative_training: mining(|m| m.ai_generative_training).cawg(),
        data_mining: mining(|m| m.data_mining).cawg(),
        constraint_info: Some(TERMS_URI.to_owned()),
    };
    let ingredients: Vec<Value> = input
        .ingredients
        .iter()
        .map(|ingredient| {
            json!({
                "instance_id": ingredient.content_hash,
                "licence": ingredient.terms.licence,
                "source_terms": ingredient.terms.source_terms,
                "declarations": ingredient.terms.declarations,
                "host_policy_terms": ingredient.terms.host_policy_terms,
                "usage": ingredient.terms.usage,
            })
        })
        .collect();
    (
        json!({"ingredients": ingredients, "derived": derived}),
        training,
    )
}

/// The resource identifier of the `index`th ingredient's own manifest store.
fn ingredient_store_id(index: usize) -> String {
    format!("ingredient-{index}.c2pa")
}

/// The manifest definition a display is signed under: a pure function of
/// the display and its ingredients. It names the display text only by the
/// binding the signer adds, and no person.
pub fn display_definition(input: &DisplayInput) -> Value {
    let ingredients: Vec<Value> = input
        .ingredients
        .iter()
        .enumerate()
        .map(|(index, ingredient)| {
            let mut entry = json!({
                "title": ingredient.reference,
                "format": RELEASE_FORMAT,
                "relationship": "inputTo",
                "instance_id": ingredient.content_hash,
                "description": ingredient.grade.as_str(),
                "metadata": {SIDECAR_DIGEST_KEY: ingredient.sidecar_digest},
            });
            if let Some(source) = &ingredient.source_manifest {
                entry["manifest_data"] = json!({
                    "format": MANIFEST_STORE_FORMAT,
                    "identifier": ingredient_store_id(index),
                });
                entry["validation_results"] = source.validation_results.clone();
            }
            entry
        })
        .collect();
    let (terms, training) = display_terms(input);
    json!({
        "claim_generator_info": [{
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }],
        "title": "Common Measure display",
        "format": TEXT_FORMAT,
        "ingredients": ingredients,
        "assertions": [
            {
                "label": "c2pa.actions",
                "data": {"actions": [{
                    "action": "c2pa.created",
                    "digitalSourceType": DIGITAL_SOURCE_TYPE,
                    "softwareAgent": {
                        "name": env!("CARGO_PKG_NAME"),
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }]},
            },
            {
                "label": TRAINING_MINING_LABEL,
                "data": training.assertion(),
            },
            {
                "label": TERMS_LABEL,
                "data": terms,
            },
        ],
    })
}

/// Build, sign and read back one display's manifest. Returns the record and,
/// where it was signed and read back, the manifest store to write at
/// `manifest_reference`.
pub fn invoke_display(input: &DisplayInput) -> (Invocation, Option<Vec<u8>>) {
    let started_at = Utc::now();
    let definition = display_definition(input);
    let mut inputs = vec![ArtefactRef {
        reference: input.display_reference.to_owned(),
        content_hash: Some(display_text_sha256(input.text)),
        tokens: None,
    }];
    for ingredient in input.ingredients {
        inputs.push(ArtefactRef {
            reference: ingredient.reference.clone(),
            content_hash: Some(ingredient.content_hash.clone()),
            tokens: None,
        });
        inputs.push(ArtefactRef {
            reference: format!("{}#sidecar", ingredient.reference),
            content_hash: Some(ingredient.sidecar_digest.clone()),
            tokens: None,
        });
    }
    let unavailable = |reason: String, signing: Value| {
        Invocation::new(
            display_manifest(),
            started_at,
            Decision::Abstain,
            "C2PA manifest built for the display; not signed, no sidecar written",
            inputs.clone(),
            Vec::new(),
            json!({"manifest_definition": definition, "signing": signing}),
            vec![Gap::new(GapReason::CapabilityUnavailable, reason)],
            DISPLAY_BLIND_SPOTS.to_vec(),
        )
    };
    let material = match input.signing {
        SigningIdentity::Configured(material) => material,
        SigningIdentity::Unconfigured => {
            return (
                unavailable(
                    format!(
                        "No signing identity is configured. Set {CERTIFICATE_VARIABLE} to a \
                         certificate chain and {KEY_VARIABLE} to its PKCS#8 key; the display \
                         stands without a manifest."
                    ),
                    json!({"basis": "unconfigured"}),
                ),
                None,
            );
        }
        SigningIdentity::Encypher(_) => {
            return (
                unavailable(
                    "A display manifest is signed locally only; the remote signer is not \
                     sent a display. The display stands without a manifest."
                        .to_owned(),
                    json!({"basis": "remote signer not used for displays"}),
                ),
                None,
            );
        }
        SigningIdentity::Unusable { reference, reason } => {
            return (
                unavailable(
                    format!(
                        "The configured signing identity cannot be used ({reason}); the \
                         display stands without a manifest."
                    ),
                    json!({"basis": "unusable", "reference": reference}),
                ),
                None,
            );
        }
    };
    let signing_detail = json!({
        "basis": "operator-configured certificate chain and key, naming the claim generator",
        "certificate": material.certificate_ref,
        "key": material.key_ref,
        "alg": "ES256",
    });
    let resources: Vec<(String, Vec<u8>)> = input
        .ingredients
        .iter()
        .enumerate()
        .filter_map(|(index, ingredient)| {
            let store = ingredient.source_manifest.as_ref()?;
            Some((ingredient_store_id(index), store.store.clone()))
        })
        .collect();
    let text: String = input.text.nfc().collect();
    let store = match sign_store(&definition, &text, material, None, None, &resources) {
        Ok(store) => store,
        Err(reason) => {
            return (
                unavailable(
                    format!(
                        "The manifest could not be signed ({reason}); the display stands \
                         without a manifest."
                    ),
                    signing_detail,
                ),
                None,
            );
        }
    };
    let reading = match read_store_with_trust(&store, &text, input.trust_anchors) {
        Ok(reading) => reading,
        Err(error) => {
            return (
                unavailable(
                    format!(
                        "The manifest was signed but could not be read back ({error}); the \
                         display stands without a manifest."
                    ),
                    signing_detail,
                ),
                None,
            );
        }
    };
    let outputs = vec![ArtefactRef {
        reference: input.manifest_reference.to_owned(),
        content_hash: Some(sha256_digest(&store)),
        tokens: None,
    }];
    let detail = json!({
        "manifest_definition": definition,
        "manifest_label": reading.manifest_label,
        "manifest_bytes": store.len(),
        "signing": signing_detail,
        "signature": reading.signature,
        "validation": reading.validation,
        "binding": {
            "method": "c2pa.hash.data",
            "normalisation": "NFC",
            "display_text_sha256": display_text_sha256(input.text),
        },
    });
    let invocation = Invocation::new(
        display_manifest(),
        started_at,
        Decision::Admit,
        format!(
            "C2PA manifest built for the display ({} ingredient{} by content hash, grade and \
             sidecar digest, no identity assertion), signed, written beside the session log \
             and read back: {}",
            input.ingredients.len(),
            if input.ingredients.len() == 1 {
                ""
            } else {
                "s"
            },
            reading.validation["summary"].as_str().unwrap_or("unread"),
        ),
        inputs,
        outputs,
        detail,
        Vec::new(),
        DISPLAY_BLIND_SPOTS.to_vec(),
    );
    (invocation, Some(store))
}

/// A display invocation whose signed store could not be written at
/// `reference`: abstained, no output, the gap naming the reference and the
/// error. The definition, the signing and the in-memory read-back stay on
/// the record.
pub fn display_unwritten(
    mut invocation: Invocation,
    reference: &str,
    error: &std::io::Error,
) -> Invocation {
    invocation.decision = Decision::Abstain;
    invocation.method =
        "C2PA manifest built and signed for the display; the sidecar could not be written"
            .to_owned();
    invocation.outputs.clear();
    invocation.detail["written"] = json!(false);
    invocation.gaps.push(Gap::new(
        GapReason::CapabilityUnavailable,
        format!(
            "The manifest store could not be written at {reference} ({error}); the display \
             stands without a manifest."
        ),
    ));
    invocation
}

/// The role a source holds in the identity assertion of its own release
/// manifest: the party that published the text.
pub const PUBLISHER_ROLE: &str = "cawg.publisher";
/// The digital source type of a source's own text.
pub const SOURCE_DIGITAL_SOURCE_TYPE: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCreation";

/// A source's own C2PA manifest store for a release text, as a source that
/// publishes releases writes it into `Sidecar.c2pa`: a `c2pa.created`
/// action, the source's `cawg.training-mining` where it declares one, a
/// `cawg.identity` under its certificate as publisher, and a data hash over
/// the NFC-normalised text. The tests and the demonstration's stand-in
/// gatekeeper sign with it; the edge only reads such stores.
pub fn sign_source_store(
    title: &str,
    text: &str,
    training_mining: Option<&TrainingMining>,
    material: &SigningMaterial,
) -> Result<Vec<u8>, String> {
    let text: String = text.nfc().collect();
    sign_store(
        &source_definition(title, training_mining),
        &text,
        material,
        Some(&SOURCE_IDENTITY),
        None,
        &[],
    )
}

/// A source store as [`sign_source_store`] writes it, except that its data
/// hash excludes the `(start, length)` byte ranges of the NFC text, given in
/// order and apart, and hashes the rest: a store that verifies without
/// binding the whole text, as a non-conforming source could sign one.
#[cfg(test)]
pub(crate) fn sign_source_store_excluding(
    title: &str,
    text: &str,
    excluded: &[(usize, usize)],
    material: &SigningMaterial,
) -> Result<Vec<u8>, String> {
    let text: String = text.nfc().collect();
    let mut binding = DataHash::new(DATA_HASH_NAME, "sha256");
    let mut bound = Vec::new();
    let mut at = 0;
    for &(start, length) in excluded {
        binding.add_exclusion(HashRange::new(start as u64, length as u64));
        bound.extend_from_slice(&text.as_bytes()[at..start]);
        at = start + length;
    }
    bound.extend_from_slice(&text.as_bytes()[at..]);
    binding.set_hash(sha2_digest(&bound));
    sign_bound(
        &source_definition(title, None),
        binding,
        material,
        Some(&SOURCE_IDENTITY),
        &[],
    )
}

/// The definition of a source's own store: a `c2pa.created` action and the
/// source's `cawg.training-mining` where it declares one.
fn source_definition(title: &str, training_mining: Option<&TrainingMining>) -> Value {
    let mut assertions = vec![json!({
        "label": "c2pa.actions",
        "data": {"actions": [{
            "action": "c2pa.created",
            "digitalSourceType": SOURCE_DIGITAL_SOURCE_TYPE,
        }]},
    })];
    if let Some(training) = training_mining {
        assertions.push(json!({"label": TRAINING_MINING_LABEL, "data": training.assertion()}));
    }
    json!({
        "title": title,
        "format": TEXT_FORMAT,
        "assertions": assertions,
    })
}

/// SHA-256 of the NFC-normalised display text, `sha256:<hex>`: what the
/// display manifest's data hash binds, checkable without the text.
pub fn display_text_sha256(text: &str) -> String {
    let text: String = text.nfc().collect();
    sha256_digest(text.as_bytes())
}

fn unavailable(
    started_at: chrono::DateTime<Utc>,
    inputs: Vec<ArtefactRef>,
    definition: Value,
    reason: String,
    signing: Value,
) -> Invocation {
    Invocation::new(
        manifest(),
        started_at,
        Decision::Abstain,
        "C2PA manifest built from the sealed source record; not signed, not embedded",
        inputs,
        Vec::new(),
        json!({
            "manifest_definition": definition,
            "signing": signing,
        }),
        vec![Gap::new(GapReason::CapabilityUnavailable, reason)],
        BLIND_SPOTS.to_vec(),
    )
}

/// A signer whose reserve is sized to the signature it will produce.
struct BoundedReserve {
    inner: BoxedSigner,
    reserve: usize,
}

impl Signer for BoundedReserve {
    fn sign(&self, data: &[u8]) -> c2pa::Result<Vec<u8>> {
        self.inner.sign(data)
    }

    fn alg(&self) -> SigningAlg {
        self.inner.alg()
    }

    fn certs(&self) -> c2pa::Result<Vec<Vec<u8>>> {
        self.inner.certs()
    }

    fn reserve_size(&self) -> usize {
        self.reserve
    }

    fn raw_signer(&self) -> Option<Box<&dyn c2pa::crypto::raw_signature::RawSigner>> {
        self.inner.raw_signer()
    }
}

fn bounded_signer(material: &SigningMaterial) -> Result<BoxedSigner, String> {
    let inner = c2pa::create_signer::from_keys(
        &material.certificate_chain,
        &material.private_key,
        SigningAlg::Es256,
        None,
    )
    .map_err(|error| format!("signer: {error}"))?;
    let chain: usize = inner
        .certs()
        .map_err(|error| format!("certificate chain: {error}"))?
        .iter()
        .map(Vec::len)
        .sum();
    Ok(Box::new(BoundedReserve {
        inner,
        reserve: chain + SIGNATURE_HEADROOM,
    }))
}

struct Signed {
    text: String,
    manifest: Vec<u8>,
    normalised_hash: String,
    normalisation_changed_bytes: bool,
    text_bytes: usize,
    wrapper_bytes: usize,
}

/// The `cawg.identity` assertion a signer adds: the assertions it
/// references and the roles it names.
struct IdentityRoles<'a> {
    referenced: &'a [&'a str],
    roles: &'a [&'a str],
}

/// A source's identity on its own store: the source, as publisher, over
/// the actions.
const SOURCE_IDENTITY: IdentityRoles<'static> = IdentityRoles {
    referenced: &["c2pa.actions"],
    roles: &[PUBLISHER_ROLE],
};

/// The batch path's identity: the operator, as producer, over the actions
/// and the source record.
const OPERATOR_IDENTITY: IdentityRoles<'static> = IdentityRoles {
    referenced: &["c2pa.actions", SOURCE_RECORD_LABEL],
    roles: &[OPERATOR_ROLE],
};

fn sign(definition: &Value, answer: &str, material: &SigningMaterial) -> Result<Signed, String> {
    let text: String = answer.nfc().collect();
    let mut wrapper_bytes = c2pa_text::worst_case_wrapper_byte_length(2048);
    let mut manifest = Vec::new();
    let mut settled = false;
    for _ in 0..SIGNING_ROUNDS {
        // The claim signer and the identity credential are one certificate:
        // the operator signs the manifest and asserts, in the same manifest,
        // that it is the operator.
        manifest = sign_store(
            definition,
            &text,
            material,
            Some(&OPERATOR_IDENTITY),
            Some(wrapper_bytes),
            &[],
        )?;
        let needed = c2pa_text::worst_case_wrapper_byte_length(manifest.len());
        if needed == wrapper_bytes {
            settled = true;
            break;
        }
        wrapper_bytes = needed;
    }
    if !settled {
        return Err(format!(
            "the wrapper length did not settle in {SIGNING_ROUNDS} rounds"
        ));
    }
    let wrapper = c2pa_text::encode_wrapper_padded(&manifest, wrapper_bytes)
        .map_err(|error| format!("wrapper: {error}"))?;
    let normalisation_changed_bytes = text != answer;
    Ok(Signed {
        normalised_hash: sha256_digest(text.as_bytes()),
        text_bytes: text.len(),
        text: format!("{text}{wrapper}"),
        manifest,
        normalisation_changed_bytes,
        wrapper_bytes,
    })
}

/// Sign `definition` into a raw C2PA manifest store whose `c2pa.hash.data`
/// binds the already NFC-normalised `text`, excluding `appended` bytes after
/// it where a wrapper will follow. `identity` adds a `cawg.identity`
/// assertion under the same certificate; `None` adds none. `resources` are
/// the binary resources the definition names by identifier, such as an
/// ingredient's own manifest store.
fn sign_store(
    definition: &Value,
    text: &str,
    material: &SigningMaterial,
    identity: Option<&IdentityRoles>,
    appended: Option<usize>,
    resources: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, String> {
    let mut binding = DataHash::new(DATA_HASH_NAME, "sha256");
    if let Some(appended) = appended {
        binding.add_exclusion(HashRange::new(text.len() as u64, appended as u64));
    }
    binding.set_hash(sha2_digest(text.as_bytes()));
    sign_bound(definition, binding, material, identity, resources)
}

/// Sign `definition` into a raw C2PA manifest store with `binding` as its
/// hard binding.
fn sign_bound(
    definition: &Value,
    binding: DataHash,
    material: &SigningMaterial,
    identity: Option<&IdentityRoles>,
    resources: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, String> {
    let signer = match identity {
        Some(identity) => c2pa::create_signer::from_x509_identity(
            bounded_signer(material)?,
            bounded_signer(material)?,
            identity.referenced,
            identity.roles,
        ),
        None => bounded_signer(material)?,
    };
    let context = Context::new().with_signer(signer).into_shared();
    let definition = serde_json::to_string(definition).map_err(|error| error.to_string())?;
    let mut builder = Builder::from_shared_context(&context)
        .with_definition(definition.as_str())
        .map_err(|error| format!("manifest definition: {error}"))?;
    for (identifier, bytes) in resources {
        builder
            .add_resource(identifier, Cursor::new(bytes.clone()))
            .map_err(|error| format!("resource {identifier}: {error}"))?;
    }
    builder
        .add_assertion(c2pa::assertions::labels::DATA_HASH, &binding)
        .map_err(|error| format!("data hash: {error}"))?;
    builder
        .sign_embeddable(RAW_MANIFEST_FORMAT)
        .map_err(|error| format!("signing: {error}"))
}

fn sha2_digest(bytes: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).to_vec()
}

/// Why a text could not be read as a labelled output.
#[derive(Debug)]
pub enum ReadBackError {
    /// The supplied local trust anchors cannot be loaded.
    Trust(String),
    /// The text carries no Annex A.8 wrapper.
    NoLabel,
    /// The wrapper is present and malformed.
    Wrapper(String),
    /// The manifest store inside the wrapper cannot be parsed or bound.
    Manifest(String),
}

impl std::fmt::Display for ReadBackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadBackError::Trust(reason) => write!(f, "invalid trust bundle: {reason}"),
            ReadBackError::NoLabel => write!(f, "the text carries no C2PA Annex A.8 label"),
            ReadBackError::Wrapper(reason) => write!(f, "the label wrapper is malformed: {reason}"),
            ReadBackError::Manifest(reason) => {
                write!(f, "the manifest cannot be read: {reason}")
            }
        }
    }
}

impl std::error::Error for ReadBackError {}

/// What a reader finds in a labelled output.
#[derive(Debug, Clone, Serialize)]
pub struct ReadBack {
    /// The text with the wrapper removed, hashed rather than carried.
    pub clean_text_hash: String,
    pub clean_text_bytes: usize,
    /// The wrapper's length in bytes, `None` when the extractor found the
    /// manifest but reported no span.
    pub wrapper_bytes: Option<usize>,
    pub manifest_bytes: usize,
    pub manifest_label: String,
    /// `state` (`trusted`, `valid`, `invalid`), `trusted`, `summary` and
    /// every status the SDK reported.
    pub validation: Value,
    pub signature: Value,
    /// The ingredients as the manifest names them: reference, content hash
    /// and grade.
    pub ingredients: Vec<Source>,
    pub actions: Value,
    pub training_mining: Value,
    pub source_record: Value,
    pub identity: Value,
    /// The SDK's whole view of the manifest store.
    pub manifest: Value,
}

/// Read a labelled output back: extract the wrapper, validate the manifest
/// against the text it is bound to, and report what it claims.
pub fn read_back(text: &str) -> Result<ReadBack, ReadBackError> {
    read_back_with_trust(text, None)
}

/// Read a labelled output against an optional, explicitly supplied PEM bundle.
/// The bundle contains local trust anchors, not a claim of C2PA-list membership.
/// Its exact bytes are hashed into the report. SDK global settings and network
/// trust discovery are not used. The same anchors apply to claim signing and
/// CAWG identity assertions; their individual SDK results remain in the report.
pub fn read_back_with_trust(
    text: &str,
    trust_anchors: Option<&str>,
) -> Result<ReadBack, ReadBackError> {
    let extracted = c2pa_text::extract_manifest(text)
        .map_err(|error| ReadBackError::Wrapper(error.to_string()))?;
    let Some(manifest_bytes) = extracted.manifest else {
        return Err(ReadBackError::NoLabel);
    };
    let store = read_manifest(
        &manifest_bytes,
        Some(text.as_bytes().to_vec()),
        trust_anchors,
    )?;
    let active = &store.manifest["manifests"][&store.manifest_label];
    let ingredients = active["ingredients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ingredient| {
            Some(Source {
                reference: ingredient["title"].as_str()?.to_owned(),
                content_hash: ingredient["instance_id"].as_str()?.to_owned(),
                grade: Grade::parse(ingredient["description"].as_str()?)?,
            })
        })
        .collect();
    Ok(ReadBack {
        clean_text_hash: sha256_digest(extracted.clean_text.as_bytes()),
        clean_text_bytes: extracted.clean_text.len(),
        wrapper_bytes: extracted.length,
        manifest_bytes: manifest_bytes.len(),
        ingredients,
        actions: store.assertion("c2pa.actions.v2"),
        training_mining: store.assertion(TRAINING_MINING_LABEL),
        source_record: store.assertion(SOURCE_RECORD_LABEL),
        identity: store.assertion("cawg.identity"),
        manifest_label: store.manifest_label,
        validation: store.validation,
        signature: store.signature,
        manifest: store.manifest,
    })
}

/// What a reader finds in a raw manifest store read against the asset it
/// binds: a display's sidecar against the display text, or a release's
/// store against the release text.
#[derive(Debug, Clone, Serialize)]
pub struct StoreReading {
    pub manifest_label: String,
    /// As in [`ReadBack::validation`].
    pub validation: Value,
    pub signature: Value,
    /// The SDK's whole view of the manifest store.
    pub manifest: Value,
    /// Every exclusion of the active manifest's `c2pa.hash.data`
    /// assertions, `(start, length)` in bytes of the asset, in order; `None`
    /// where the SDK's detailed report could not be read. The SDK hashes
    /// with these exclusions and does not require that they leave the whole
    /// text bound, so a reader that needs full coverage checks them.
    #[serde(skip)]
    pub data_hash_exclusions: Option<Vec<(u64, u64)>>,
}

impl StoreReading {
    /// The active manifest's assertion under `label`, or null.
    pub fn assertion(&self, label: &str) -> Value {
        self.assertions(label)
            .into_iter()
            .next()
            .unwrap_or(Value::Null)
    }

    /// Every assertion of the active manifest under `label`, in order.
    pub fn assertions(&self, label: &str) -> Vec<Value> {
        self.active()["assertions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|assertion| assertion["label"] == label)
            .map(|assertion| assertion["data"].clone())
            .collect()
    }

    /// The active manifest as the SDK views it.
    pub fn active(&self) -> &Value {
        &self.manifest["manifests"][&self.manifest_label]
    }

    /// The reader's validation results in the C2PA `validationResults`
    /// shape, as an ingredient carrying this store records them.
    pub fn validation_results(&self) -> Value {
        self.manifest["validation_results"].clone()
    }

    /// Every validation status code the reader reported.
    pub fn status_codes(&self) -> Vec<String> {
        self.validation["statuses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|status| status["code"].as_str().map(str::to_owned))
            .collect()
    }
}

/// Read a raw manifest store back against the text it binds, which is
/// NFC-normalised here as it was for signing. Offline: no remote manifest
/// and no OCSP response is fetched. `trust_anchors` is an explicitly
/// supplied PEM bundle as in [`read_back_with_trust`].
pub fn read_store_with_trust(
    manifest: &[u8],
    text: &str,
    trust_anchors: Option<&str>,
) -> Result<StoreReading, ReadBackError> {
    let text: String = text.nfc().collect();
    read_manifest(manifest, Some(text.into_bytes()), trust_anchors)
}

/// Read a raw manifest store back without the asset it binds: the claim,
/// its signature and the ingredients are validated, the hard binding is
/// not. For a display's sidecar when the display text is not at hand; the
/// report says the binding was not re-verified rather than reading the
/// absence as a pass or a failure.
pub fn read_store_unbound(
    manifest: &[u8],
    trust_anchors: Option<&str>,
) -> Result<StoreReading, ReadBackError> {
    let mut reading = read_manifest(manifest, None, trust_anchors)?;
    // Read on its own, the store's data hash is computed over nothing and
    // cannot match; that status says nothing about the store.
    let (set_aside, statuses): (Vec<Value>, Vec<Value>) = reading.validation["statuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .partition(|status| status["code"] == DATA_HASH_MISMATCH);
    let trusted = reading.validation_results()["activeManifest"]["success"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|status| status["code"] == "signingCredential.trusted");
    let untrusted_only = statuses
        .iter()
        .all(|status| status["code"] == "signingCredential.untrusted");
    let (state, summary) = if statuses.is_empty() && trusted {
        (
            "trusted",
            "the claim signature verifies under the supplied trust anchors; the binding to the \
             text was not re-verified",
        )
    } else if untrusted_only {
        (
            "valid",
            "the claim signature verifies, signer trust not established; the binding to the \
             text was not re-verified",
        )
    } else {
        (
            "invalid",
            "the manifest does not verify; see the statuses. The binding to the text was not \
             re-verified",
        )
    };
    reading.validation["state"] = json!(state);
    reading.validation["trusted"] = json!(state == "trusted");
    reading.validation["summary"] = json!(summary);
    reading.validation["statuses"] = json!(statuses);
    reading.validation["binding"] = json!({
        "re_verified": false,
        "set_aside": set_aside,
    });
    Ok(reading)
}

/// The status a store read without its asset always reports.
const DATA_HASH_MISMATCH: &str = "assertion.dataHash.mismatch";

/// `asset` is the bytes the store's data hash binds, or `None` to read the
/// store on its own.
fn read_manifest(
    manifest: &[u8],
    asset: Option<Vec<u8>>,
    trust_anchors: Option<&str>,
) -> Result<StoreReading, ReadBackError> {
    let mut settings = c2pa::settings::Settings::default();
    settings.verify.remote_manifest_fetch = false;
    settings.verify.ocsp_fetch = false;
    if let Some(pem) = trust_anchors {
        validate_trust_bundle(pem)?;
        settings = settings
            .with_value("trust.user_anchors", pem)
            .and_then(|settings| settings.with_value("cawg_trust.user_anchors", pem))
            .map_err(|error| ReadBackError::Trust(error.to_string()))?;
    }
    let context = Context::new()
        .with_settings(settings)
        .map_err(|error| ReadBackError::Trust(error.to_string()))?;
    let reader = match asset {
        Some(asset) => Reader::from_context(context).with_manifest_data_and_stream(
            manifest,
            TEXT_FORMAT,
            Cursor::new(asset),
        ),
        None => Reader::from_context(context)
            .with_stream(MANIFEST_STORE_FORMAT, Cursor::new(manifest.to_vec())),
    }
    .map_err(|error| ReadBackError::Manifest(error.to_string()))?;
    let view: Value = serde_json::from_str(&reader.json())
        .map_err(|error| ReadBackError::Manifest(error.to_string()))?;
    let label = reader
        .active_label()
        .ok_or_else(|| ReadBackError::Manifest("no active manifest".to_owned()))?
        .to_owned();
    let signature = view["manifests"][&label]["signature_info"].clone();
    let data_hash_exclusions = data_hash_exclusions(&reader, &label);

    let statuses: Vec<Value> = reader
        .validation_status()
        .into_iter()
        .flatten()
        .map(|status| {
            json!({
                "code": status.code(),
                "url": status.url(),
                "explanation": status.explanation(),
            })
        })
        .collect();
    let untrusted_only = !statuses.is_empty()
        && statuses
            .iter()
            .all(|status| status["code"] == "signingCredential.untrusted");
    let (state, trusted, summary) = match reader.validation_state() {
        c2pa::ValidationState::Trusted => (
            "trusted",
            true,
            "valid and trusted: the signature verifies under the supplied trust anchors",
        ),
        c2pa::ValidationState::Valid if untrusted_only => (
            "valid",
            false,
            "valid but untrusted: the signature verifies but signer trust is not established \
             under this verification configuration",
        ),
        c2pa::ValidationState::Valid => (
            "valid",
            false,
            "valid: the signature verifies; see the statuses",
        ),
        c2pa::ValidationState::Invalid => (
            "invalid",
            false,
            "invalid: the manifest does not verify against this text",
        ),
    };
    Ok(StoreReading {
        manifest_label: label,
        validation: json!({
            "state": state,
            "trusted": trusted,
            "summary": summary,
            "trust_list": if trust_anchors.is_some() { "local trust anchors" } else { "none configured" },
            "trust_anchors_hash": trust_anchors.map(|pem| sha256_digest(pem.as_bytes())),
            "trust_scope": ["claim", "cawg.identity"],
            "checked_at": Utc::now().to_rfc3339(),
            "verifier": { "name": "c2pa-rs", "version": c2pa::VERSION },
            "statuses": statuses,
        }),
        signature,
        manifest: view,
        data_hash_exclusions,
    })
}

/// The exclusions of the manifest `label`'s `c2pa.hash.data` assertions,
/// read from the SDK's detailed report, which keeps the assertion store as
/// signed; the plain view leaves hard bindings out. `None` where the report
/// or any range in it does not read.
fn data_hash_exclusions(reader: &Reader, label: &str) -> Option<Vec<(u64, u64)>> {
    let report: Value = serde_json::from_str(&reader.detailed_json_checked().ok()?).ok()?;
    let data_hash = c2pa::assertions::labels::DATA_HASH;
    report["manifests"][label]["assertion_store"]
        .as_object()?
        .iter()
        // A second assertion under one label carries an instance suffix.
        .filter(|(key, _)| {
            key.strip_prefix(data_hash)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("__"))
        })
        .flat_map(|(_, assertion)| {
            assertion["exclusions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|range| Some((range["start"].as_u64()?, range["length"].as_u64()?)))
        .collect()
}

/// A `cawg.identity` assertion as the reader reports it, summarised: the
/// credential kind, the roles, the credential's issuer and serial, and its
/// subject where the reader names one. An X.509 credential whose serial is
/// the claim signer's is that certificate, so it names the claim signer's
/// subject. Never the signature bytes.
pub fn identity_summary(identity: &Value, claim_signature: &Value) -> Value {
    let credential = &identity["signature_info"];
    let same_certificate = !credential["cert_serial_number"].is_null()
        && credential["cert_serial_number"] == claim_signature["cert_serial_number"]
        && credential["issuer"] == claim_signature["issuer"];
    json!({
        "kind": identity["signer_payload"]["sig_type"],
        "roles": identity["signer_payload"]["role"],
        "subject": if same_certificate {
            claim_signature["common_name"].clone()
        } else {
            credential["common_name"].clone()
        },
        "issuer": credential["issuer"],
        "cert_serial_number": credential["cert_serial_number"],
    })
}

fn validate_trust_bundle(pem: &str) -> Result<(), ReadBackError> {
    let mut remaining = pem.trim();
    let mut count = 0;
    while !remaining.is_empty() {
        if remaining.lines().next() != Some("-----BEGIN CERTIFICATE-----") {
            return Err(ReadBackError::Trust(
                "expected a PEM certificate".to_owned(),
            ));
        }
        let (rest, certificate) = x509_parser::pem::parse_x509_pem(remaining.as_bytes())
            .map_err(|error| ReadBackError::Trust(error.to_string()))?;
        // The parser accepts mismatched END labels; a configured certificate
        // bundle must not silently accept another PEM block type's footer.
        let consumed = &remaining[..remaining.len() - rest.len()];
        if consumed.lines().last().map(str::trim) != Some("-----END CERTIFICATE-----") {
            return Err(ReadBackError::Trust(
                "expected a PEM certificate footer".to_owned(),
            ));
        }
        let (trailing, _) = x509_parser::parse_x509_certificate(&certificate.contents)
            .map_err(|error| ReadBackError::Trust(error.to_string()))?;
        if !trailing.is_empty() {
            return Err(ReadBackError::Trust("trailing certificate data".to_owned()));
        }
        remaining = std::str::from_utf8(rest)
            .map_err(|error| ReadBackError::Trust(error.to_string()))?
            .trim();
        count += 1;
    }
    if count == 0 {
        return Err(ReadBackError::Trust("no certificates supplied".to_owned()));
    }
    Ok(())
}

/// The source record a plan's published summary implies: the run id and
/// manifest hash from the summary, the answer's hash from the plan, and the
/// window from the transform stage's outputs (empty for a plan that ran no
/// transform, the baseline). This is what the label is compared with, so a
/// summary edited after the label was signed no longer derives it.
pub fn record_from_summary(summary: &Value, plan_id: &str) -> Result<Value, String> {
    let plan = summary["plans"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|plan| plan["id"] == plan_id)
        .ok_or_else(|| format!("the summary has no plan {plan_id}"))?;
    let answer = plan["answer"]
        .as_str()
        .ok_or_else(|| format!("plan {plan_id} records no answer"))?;
    let run_id = summary["run"]["id"]
        .as_str()
        .ok_or("the summary records no run id")?;
    let run_manifest_hash = summary["run"]["manifest_hash"]
        .as_str()
        .ok_or("the summary records no manifest hash")?;
    let sources: Vec<Source> = plan["processors"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|invocation| invocation["stage"] == "transform")
        .and_then(|invocation| invocation["outputs"].as_array())
        .into_iter()
        .flatten()
        .map(|output| {
            Ok(Source {
                reference: output["reference"]
                    .as_str()
                    .ok_or("a transform output has no reference")?
                    .to_owned(),
                content_hash: output["content_hash"]
                    .as_str()
                    .ok_or("a transform output has no content hash")?
                    .to_owned(),
                grade: Grade::Mediated,
            })
        })
        .collect::<Result<_, &str>>()?;
    Ok(json!({
        "run_id": run_id,
        "plan_id": plan_id,
        "run_manifest_hash": run_manifest_hash,
        "answer_hash": sha256_digest(answer.as_bytes()),
        "sources": sources,
    }))
}

/// Field-by-field comparison of a label's source record with the record a
/// summary implies: every top-level field that differs, by name, and every
/// source position whose reference, hash or grade differs. A reference that
/// differs only in its userinfo is not a difference.
pub fn record_differences(label: &Value, summary: &Value) -> Vec<String> {
    let mut differences = Vec::new();
    for field in ["run_id", "plan_id", "run_manifest_hash", "answer_hash"] {
        if label[field] != summary[field] {
            differences.push(format!(
                "{field}: the label carries {}, the summary implies {}",
                label[field], summary[field]
            ));
        }
    }
    let empty = Vec::new();
    let labelled = label["sources"].as_array().unwrap_or(&empty);
    let implied = summary["sources"].as_array().unwrap_or(&empty);
    if labelled.len() != implied.len() {
        differences.push(format!(
            "sources: the label carries {} source(s), the summary implies {}",
            labelled.len(),
            implied.len()
        ));
    }
    for (position, (ours, theirs)) in labelled.iter().zip(implied).enumerate() {
        // An Encypher label carries each reference as it left the machine,
        // with its userinfo removed, while the summary keeps it as recorded.
        // Credentials do not name a different resource, so they are not a
        // difference.
        let reference = |source: &Value| {
            source["reference"]
                .as_str()
                .map(|reference| without_userinfo(reference).into_owned())
        };
        if reference(ours) != reference(theirs) {
            differences.push(format!(
                "sources[{position}].reference: the label carries {}, the summary implies {}",
                ours["reference"], theirs["reference"]
            ));
        }
        for field in ["content_hash", "grade"] {
            if ours[field] != theirs[field] {
                differences.push(format!(
                    "sources[{position}].{field}: the label carries {}, the summary implies {}",
                    ours[field], theirs[field]
                ));
            }
        }
    }
    differences
}

/// A source reference as it may leave the machine: any userinfo removed and
/// every other byte as recorded. Operator manifest and supplier URLs can
/// carry credentials (`https://user:secret@host/path`); they authenticate a
/// fetch and are not part of the source's identity. Nothing else changes,
/// with no case folding and no port or path normalisation, so the reference
/// still reads as the one in the run's own record.
///
/// Userinfo is what a URL parser reads as userinfo, so `http:\\u:p@h/` loses
/// its credentials and `http://h\@evil.example/` keeps its host `h`. The
/// userinfo is cut out of the text when the result parses to the URL the
/// parser gives with the userinfo cleared; otherwise the parser's
/// serialisation is used. A value that does not parse but has a
/// `scheme://user@host` authority, such as one with an out-of-range port,
/// has its userinfo cut out of the text. Any other value is returned as it
/// is.
fn without_userinfo(reference: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    match url::Url::parse(reference) {
        Ok(url) if url.username().is_empty() && url.password().is_none() => {
            Cow::Borrowed(reference)
        }
        Ok(mut url) => {
            // Both setters refuse only a URL that cannot carry credentials,
            // and this one carries them.
            let _ = url.set_username("");
            let _ = url.set_password(None);
            match authority_without_userinfo(reference) {
                Some(text) if url::Url::parse(&text).is_ok_and(|parsed| parsed == url) => {
                    Cow::Owned(text)
                }
                _ => Cow::Owned(url.into()),
            }
        }
        Err(_) => {
            authority_without_userinfo(reference).map_or(Cow::Borrowed(reference), Cow::Owned)
        }
    }
}

/// `value` with the userinfo of its authority cut out, when it has a scheme
/// followed by slashes and an authority containing `@`. The authority ends at
/// the first `/`, `\`, `?` or `#`; its userinfo ends at the last `@` in it.
fn authority_without_userinfo(value: &str) -> Option<String> {
    let colon = value.find(':')?;
    let scheme = &value[..colon];
    let is_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !is_scheme {
        return None;
    }
    let after_scheme = &value[colon + 1..];
    let authority = after_scheme.trim_start_matches(['/', '\\']);
    if authority.len() == after_scheme.len() {
        return None;
    }
    let authority_start = value.len() - authority.len();
    let authority_end = authority
        .find(['/', '\\', '?', '#'])
        .unwrap_or(authority.len());
    let at = authority[..authority_end].rfind('@')?;
    Some(format!(
        "{}{}",
        &value[..authority_start],
        &authority[at + 1..]
    ))
}

/// An ephemeral stand-in publisher for the tests: a private root and the
/// end-entity certificate it issues, with the key usage the C2PA
/// certificate profile requires, generated per call so no publisher key is
/// committed. Returns the signing material and the root's PEM, which a test
/// hands to the reader as a trust anchor where it wants `trusted`.
#[cfg(test)]
pub(crate) fn test_publisher() -> (SigningMaterial, String) {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    };
    let named = |common_name: &str| {
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, common_name);
        name.push(DnType::OrganizationName, "Common Measure Ltd");
        name
    };
    let root_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut root = CertificateParams::default();
    root.distinguished_name = named("Stand-in publisher test root");
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let root = root.self_signed(&root_key).unwrap();
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut leaf = CertificateParams::default();
    leaf.distinguished_name = named("Stand-in publisher, test");
    leaf.is_ca = IsCa::ExplicitNoCa;
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    leaf.use_authority_key_identifier_extension = true;
    let leaf = leaf.signed_by(&leaf_key, &root, &root_key).unwrap();
    let material = SigningMaterial {
        certificate_chain: format!("{}{}", leaf.pem(), root.pem()).into_bytes(),
        private_key: leaf_key.serialize_pem().into_bytes(),
        certificate_ref: "ephemeral stand-in publisher certificate".to_owned(),
        key_ref: "ephemeral stand-in publisher key".to_owned(),
    };
    (material, root.pem())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OutputProvenance {
        OutputProvenance {
            encypher: None,
            training_mining: TrainingMining {
                ai_inference: Use::Allowed,
                ai_training: Use::NotAllowed,
                ai_generative_training: Use::NotAllowed,
                data_mining: Use::Constrained,
                constraint_info: Some("https://operator.example/ai-use".to_owned()),
            },
        }
    }

    fn input<'a>(
        sources: &'a [Source],
        policy: &'a OutputProvenance,
        signing: &'a SigningIdentity,
    ) -> Input<'a> {
        Input {
            run_id: "run-1",
            plan_id: "internal",
            run_manifest_hash: "sha256:manifest",
            answer: "The cap rose. CITATION: https://a.example/one",
            answer_reference: "plans/internal/answer",
            sources,
            policy,
            signing,
            labelled_reference: "provenance/internal.txt",
            manifest_reference: "provenance/internal.c2pa",
        }
    }

    #[test]
    fn the_definition_carries_every_source_by_hash_and_grade_and_no_text() {
        let sources = vec![
            Source {
                reference: "https://a.example/one".to_owned(),
                content_hash: "sha256:aa".to_owned(),
                grade: Grade::Mediated,
            },
            Source {
                reference: "https://a.example/two".to_owned(),
                content_hash: "sha256:bb".to_owned(),
                grade: Grade::Observed,
            },
        ];
        let policy = policy();
        let signing = SigningIdentity::Unconfigured;
        let input = input(&sources, &policy, &signing);
        let definition = definition(&input);
        let ingredients = definition["ingredients"].as_array().unwrap();
        assert_eq!(ingredients.len(), 2);
        assert_eq!(ingredients[0]["instance_id"], "sha256:aa");
        assert_eq!(ingredients[0]["description"], "mediated");
        assert_eq!(ingredients[1]["description"], "observed");
        let serialised = definition.to_string();
        assert!(!serialised.contains("The cap rose"));
        assert!(serialised.contains(&sha256_digest(input.answer.as_bytes())));
        let training = &definition["assertions"][1]["data"]["entries"];
        assert_eq!(training["cawg.ai_training"]["use"], "notAllowed");
        assert_eq!(
            training["cawg.data_mining"]["constraint_info"],
            "https://operator.example/ai-use"
        );
        assert!(
            training["cawg.ai_inference"]
                .get("constraint_info")
                .is_none()
        );
    }

    #[test]
    fn without_a_signing_identity_the_invocation_abstains_and_names_the_variables() {
        let sources = Vec::new();
        let policy = policy();
        let signing = SigningIdentity::Unconfigured;
        let (invocation, label) = invoke(&input(&sources, &policy, &signing));
        assert!(label.is_none());
        let record = invocation.to_value();
        assert_eq!(record["decision"], "abstain");
        let gap = record["gaps"][0]["detail"].as_str().unwrap();
        assert!(gap.contains(CERTIFICATE_VARIABLE) && gap.contains(KEY_VARIABLE));
        assert!(record["detail"]["manifest_definition"]["assertions"].is_array());
        assert!(record["outputs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_constrained_entry_needs_its_terms() {
        let mut policy = policy();
        policy.training_mining.constraint_info = None;
        assert!(policy.validate().is_err());
        policy.training_mining.data_mining = Use::NotAllowed;
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn signing_material_debug_prints_lengths_and_never_the_key() {
        let material = SigningMaterial {
            certificate_chain: b"-----BEGIN CERTIFICATE-----".to_vec(),
            private_key: b"-----BEGIN PRIVATE KEY-----secret".to_vec(),
            certificate_ref: "/keys/signer.pem".to_owned(),
            key_ref: "/keys/signer-key.pem".to_owned(),
        };
        let printed = format!("{material:?}");
        assert!(printed.contains("/keys/signer-key.pem"));
        assert!(printed.contains("private_key_bytes: 33"));
        assert!(!printed.contains("secret") && !printed.contains("PRIVATE KEY"));
    }

    #[test]
    fn a_summary_edited_after_signing_no_longer_derives_the_record() {
        let summary = json!({
            "run": {"id": "run-1", "manifest_hash": "sha256:manifest"},
            "plans": [{
                "id": "internal-only",
                "answer": "The cap rose.",
                "processors": [{
                    "stage": "transform",
                    "outputs": [
                        {"reference": "https://a.example/one", "content_hash": "sha256:aa"},
                        {"reference": "https://a.example/two", "content_hash": "sha256:bb"}
                    ]
                }]
            }]
        });
        let implied = record_from_summary(&summary, "internal-only").unwrap();
        assert!(record_differences(&implied, &implied).is_empty());
        let mut edited = summary.clone();
        edited["plans"][0]["processors"][0]["outputs"][1]["content_hash"] = json!("sha256:cc");
        let differences = record_differences(
            &implied,
            &record_from_summary(&edited, "internal-only").unwrap(),
        );
        assert_eq!(differences.len(), 1);
        assert!(differences[0].starts_with("sources[1].content_hash"));
        let mut answered = summary;
        answered["plans"][0]["answer"] = json!("The cap fell.");
        let differences = record_differences(
            &implied,
            &record_from_summary(&answered, "internal-only").unwrap(),
        );
        assert!(differences.iter().any(|d| d.starts_with("answer_hash")));
        assert!(record_from_summary(&implied, "absent").is_err());
    }

    #[test]
    fn userinfo_is_removed_as_a_url_parser_reads_it() {
        for (raw, expected) in [
            ("https://token@a.example/x", "https://a.example/x"),
            ("https://:p@a.example/x", "https://a.example/x"),
            // The last `@` in the authority ends the userinfo.
            ("https://u@v:p@a.example/x", "https://a.example/x"),
            (
                "https://u:p@a.example:8443/x#f@g",
                "https://a.example:8443/x#f@g",
            ),
            // A special scheme reads a backslash or a single slash as the
            // start of the authority; the text keeps them.
            (r"http:\\u:p@h/l.xml", r"http:\\h/l.xml"),
            ("https:/u:p@h/l.xml", "https:/h/l.xml"),
            // A backslash before an `@` ends a special scheme's host, so
            // there is no userinfo to remove.
            (
                r"http://h\@evil.example/l.xml",
                r"http://h\@evil.example/l.xml",
            ),
            ("terms://legal@acme/contract-7", "terms://acme/contract-7"),
            // Where the text alone finds no userinfo, the parser's
            // serialisation is used: in a non-special scheme a backslash does
            // not end the authority, so `a\b` is userinfo.
            (r"TERMS://a\b@Acme/c", "terms://Acme/c"),
            (
                "/license.xml?next=http://u:p@h/",
                "/license.xml?next=http://u:p@h/",
            ),
            ("https://@a.example/x", "https://@a.example/x"),
            ("not a url", "not a url"),
        ] {
            assert_eq!(without_userinfo(raw), expected, "sending {raw}");
        }
    }

    #[test]
    fn a_label_naming_a_reference_without_its_userinfo_matches_the_summary() {
        let summary = json!({
            "run": {"id": "run-1", "manifest_hash": "sha256:manifest"},
            "plans": [{
                "id": "internal-only",
                "answer": "The cap rose.",
                "processors": [{
                    "stage": "transform",
                    "outputs": [
                        {"reference": "https://u:p@a.example/m.json", "content_hash": "sha256:aa"}
                    ]
                }]
            }]
        });
        let implied = record_from_summary(&summary, "internal-only").unwrap();
        let mut label = implied.clone();
        label["sources"][0]["reference"] = json!("https://a.example/m.json");
        assert!(record_differences(&label, &implied).is_empty());
        label["sources"][0]["reference"] = json!("https://b.example/m.json");
        let differences = record_differences(&label, &implied);
        assert_eq!(differences.len(), 1);
        assert!(differences[0].starts_with("sources[0].reference"));
    }

    fn identity(name: &str) -> SigningMaterial {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/provenance");
        SigningMaterial {
            certificate_chain: std::fs::read(root.join(format!("{name}.pem"))).unwrap(),
            private_key: std::fs::read(root.join(format!("{name}-key.pem"))).unwrap(),
            certificate_ref: format!("demo/provenance/{name}.pem"),
            key_ref: format!("demo/provenance/{name}-key.pem"),
        }
    }

    /// The issuing root of a demonstration chain: its second certificate.
    fn root_of(material: &SigningMaterial) -> String {
        let chain = String::from_utf8(material.certificate_chain.clone()).unwrap();
        let at = chain
            .match_indices("-----BEGIN CERTIFICATE-----")
            .nth(1)
            .unwrap()
            .0;
        chain[at..].to_owned()
    }

    const RELEASE_TEXT: &str = "The harbour bridge reopened on Monday after repairs.";
    const DISPLAY_TEXT: &str = "Display of the signed release sha256:aa (strict display)\n\n\
        [from the source; p0] The harbour bridge reopened on Monday after repairs.\n";

    fn usage(ai_input: Permission, onward: Permission, report: Option<&str>) -> Usage {
        Usage {
            ai_input,
            onward_commercial_product: onward,
            reporting: Reporting {
                required: report.is_some(),
                destinations: report.into_iter().map(str::to_owned).collect(),
            },
        }
    }

    fn ingredient(usage: Usage, source_manifest: Option<SourceManifest>) -> DisplayIngredient {
        DisplayIngredient {
            reference: "https://source.example/a".to_owned(),
            content_hash: sha256_digest(RELEASE_TEXT.as_bytes()),
            grade: Grade::Mediated,
            sidecar_digest: "sha256:5eed".to_owned(),
            source_manifest,
            terms: IngredientTerms {
                licence: "CC BY-SA 4.0".to_owned(),
                source_terms: json!({"terms": ["Show a kept passage whole."]}),
                declarations: json!({"effective": {"ai-input": "allow"}}),
                host_policy_terms: None,
                usage,
                mining: DeclaredMining {
                    ai_training: Permission::Disallow,
                    ai_generative_training: Permission::Unknown,
                    data_mining: Permission::Unknown,
                },
            },
        }
    }

    fn display_input<'a>(
        ingredients: &'a [DisplayIngredient],
        signing: &'a SigningIdentity,
        trust_anchors: Option<&'a str>,
    ) -> DisplayInput<'a> {
        DisplayInput {
            text: DISPLAY_TEXT,
            display_reference: "sessions/s.ndjson#display_rendered",
            ingredients,
            signing,
            manifest_reference: "sessions/s.displays/7.c2pa",
            trust_anchors,
        }
    }

    #[test]
    fn the_derived_use_is_the_most_restrictive_and_unknown_is_never_allowed() {
        use Permission::{Allow, Disallow, Unknown};
        assert_eq!(Permission::most_restrictive([Allow, Allow]), Allow);
        assert_eq!(Permission::most_restrictive([Allow, Unknown]), Unknown);
        assert_eq!(
            Permission::most_restrictive([Unknown, Disallow, Allow]),
            Disallow
        );
        assert_eq!(Permission::most_restrictive([]), Unknown);
        let derived = Usage::derive(&[
            usage(Allow, Disallow, Some("https://a.example/report")),
            usage(Allow, Allow, None),
            usage(Unknown, Allow, Some("https://b.example/report")),
        ]);
        assert_eq!(derived.ai_input, Unknown);
        assert_eq!(derived.onward_commercial_product, Disallow);
        assert!(derived.reporting.required);
        assert_eq!(
            derived.reporting.destinations,
            ["https://a.example/report", "https://b.example/report"]
        );
        assert_eq!(Permission::stated([None, Some(Allow)]), Allow);
        assert_eq!(Permission::stated([Some(Allow), Some(Disallow)]), Disallow);
        assert_eq!(Permission::stated([None, None]), Unknown);
        let none = Usage::derive(&[]);
        assert_eq!(
            (none.ai_input, none.onward_commercial_product),
            (Unknown, Unknown)
        );
    }

    #[test]
    fn a_display_definition_names_the_release_and_derives_the_cawg_entries() {
        let ingredients = [ingredient(
            usage(Permission::Allow, Permission::Disallow, None),
            None,
        )];
        let signing = SigningIdentity::Unconfigured;
        let definition = display_definition(&display_input(&ingredients, &signing, None));
        let ingredient = &definition["ingredients"][0];
        assert_eq!(
            ingredient["instance_id"],
            sha256_digest(RELEASE_TEXT.as_bytes())
        );
        assert_eq!(ingredient["description"], "mediated");
        assert_eq!(ingredient["format"], "application/x.signed-release+json");
        assert_eq!(ingredient["relationship"], "inputTo");
        assert_eq!(ingredient["metadata"][SIDECAR_DIGEST_KEY], "sha256:5eed");
        assert!(ingredient.get("manifest_data").is_none());
        let labels: Vec<&str> = definition["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["c2pa.actions", TRAINING_MINING_LABEL, TERMS_LABEL]);
        let entries = &definition["assertions"][1]["data"]["entries"];
        assert_eq!(entries["cawg.ai_inference"]["use"], "allowed");
        assert_eq!(entries["cawg.ai_training"]["use"], "notAllowed");
        assert_eq!(entries["cawg.data_mining"]["use"], "constrained");
        assert_eq!(entries["cawg.data_mining"]["constraint_info"], TERMS_URI);
        let terms = &definition["assertions"][2]["data"];
        assert_eq!(terms["derived"]["onward_commercial_product"], "disallow");
        assert_eq!(terms["ingredients"][0]["licence"], "CC BY-SA 4.0");
        assert_eq!(
            terms["ingredients"][0]["usage"]["onward_commercial_product"],
            "disallow"
        );
        assert!(!definition.to_string().contains("harbour bridge"));
    }

    #[test]
    fn a_display_manifest_is_signed_without_identity_and_reads_back_against_the_text() {
        let ingredients = [ingredient(
            usage(Permission::Allow, Permission::Disallow, None),
            None,
        )];
        let signer = identity("signer");
        let anchors = root_of(&signer);
        let signing = SigningIdentity::Configured(signer);
        let (invocation, store) =
            invoke_display(&display_input(&ingredients, &signing, Some(&anchors)));
        let store = store.expect("signed");
        let record = invocation.to_value();
        assert_eq!(record["decision"], "admit", "{record}");
        assert_eq!(
            record["detail"]["validation"]["state"], "trusted",
            "{record}"
        );
        assert_eq!(record["outputs"][0]["content_hash"], sha256_digest(&store));
        assert_eq!(
            record["detail"]["binding"]["display_text_sha256"],
            display_text_sha256(DISPLAY_TEXT)
        );

        let reading = read_store_with_trust(&store, DISPLAY_TEXT, None).unwrap();
        assert_eq!(reading.validation["state"], "valid");
        assert!(
            reading.assertions("cawg.identity").is_empty(),
            "no identity"
        );
        assert_eq!(reading.data_hash_exclusions, Some(vec![]), "the whole text");
        let ingredient = &reading.active()["ingredients"][0];
        assert_eq!(
            ingredient["instance_id"],
            sha256_digest(RELEASE_TEXT.as_bytes())
        );
        assert_eq!(ingredient["description"], "mediated");
        assert_eq!(ingredient["metadata"][SIDECAR_DIGEST_KEY], "sha256:5eed");
        assert_eq!(
            reading.assertion(TERMS_LABEL)["derived"]["onward_commercial_product"],
            "disallow"
        );
        // The binding is over the display text: another text does not verify.
        let other = read_store_with_trust(&store, "Another display.", None).unwrap();
        assert_eq!(other.validation["state"], "invalid");
        let bytes = String::from_utf8_lossy(&store);
        assert!(!bytes.contains("harbour bridge"), "no text in the store");

        // Without the text the claim still verifies, and the binding is
        // reported as not re-verified rather than as a failure.
        let unbound = read_store_unbound(&store, Some(&anchors)).unwrap();
        assert_eq!(
            unbound.validation["state"], "trusted",
            "{}",
            unbound.validation
        );
        assert_eq!(unbound.validation["binding"]["re_verified"], false);
        assert_eq!(
            unbound.validation["binding"]["set_aside"][0]["code"],
            DATA_HASH_MISMATCH
        );
        assert_eq!(
            read_store_unbound(&store, None).unwrap().validation["state"],
            "valid"
        );
        let mut tampered = store.clone();
        let at = tampered.len() - 8;
        tampered[at] ^= 0xff;
        assert!(
            read_store_unbound(&tampered, None)
                .map(|reading| reading.validation["state"] == "invalid")
                .unwrap_or(true),
            "a damaged store does not read as valid"
        );
    }

    #[test]
    fn a_display_ingredient_carries_the_release_s_own_store() {
        let (publisher, _) = test_publisher();
        let source = sign_source_store(
            "https://source.example/a",
            RELEASE_TEXT,
            Some(&policy().training_mining),
            &publisher,
        )
        .unwrap();
        let read = read_store_with_trust(&source, RELEASE_TEXT, None).unwrap();
        assert_eq!(read.validation["state"], "valid");
        let identities = read.assertions("cawg.identity");
        assert_eq!(identities.len(), 1);
        let summary = identity_summary(&identities[0], &read.signature);
        assert_eq!(summary["subject"], "Stand-in publisher, test");
        assert_eq!(summary["roles"], json!([PUBLISHER_ROLE]));
        assert_eq!(summary["kind"], "cawg.x509.cose");

        let ingredients = [ingredient(
            usage(Permission::Allow, Permission::Disallow, None),
            Some(SourceManifest {
                store: source,
                state: "untrusted".to_owned(),
                validation_results: read.validation_results(),
            }),
        )];
        let signing = SigningIdentity::Configured(identity("signer"));
        let (invocation, store) = invoke_display(&display_input(&ingredients, &signing, None));
        let store = store.unwrap_or_else(|| panic!("{}", invocation.to_value()));
        let reading = read_store_with_trust(&store, DISPLAY_TEXT, None).unwrap();
        assert_ne!(
            reading.validation["state"], "invalid",
            "{}",
            reading.validation
        );
        let ingredient = &reading.active()["ingredients"][0];
        let carried = ingredient["active_manifest"]
            .as_str()
            .expect("the source's manifest");
        let source_manifest = &reading.manifest["manifests"][carried];
        assert_eq!(
            source_manifest["signature_info"]["common_name"],
            "Stand-in publisher, test"
        );
        // The source's identity travels inside its own manifest; the
        // display's active manifest asserts none.
        assert!(reading.assertions("cawg.identity").is_empty());
    }

    #[test]
    fn without_a_signing_identity_a_display_abstains_and_names_the_variables() {
        let ingredients = [ingredient(
            usage(Permission::Allow, Permission::Allow, None),
            None,
        )];
        let signing = SigningIdentity::Unconfigured;
        let (invocation, store) = invoke_display(&display_input(&ingredients, &signing, None));
        assert!(store.is_none());
        let record = invocation.to_value();
        assert_eq!(record["decision"], "abstain");
        assert_eq!(record["processor"]["name"], NAME);
        let gap = record["gaps"][0]["detail"].as_str().unwrap();
        assert!(gap.contains(CERTIFICATE_VARIABLE) && gap.contains(KEY_VARIABLE));
        assert!(record["detail"]["manifest_definition"]["ingredients"].is_array());
        assert!(record["outputs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn unlabelled_text_reads_back_as_no_label() {
        assert!(matches!(
            read_back("plain text, no wrapper"),
            Err(ReadBackError::NoLabel)
        ));
    }
}
