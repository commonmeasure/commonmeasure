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
            "name": IN_PROCESS.crate_name,
            "version": IN_PROCESS.crate_version,
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
                        "name": IN_PROCESS.crate_name,
                        "version": IN_PROCESS.crate_version,
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

fn sign(definition: &Value, answer: &str, material: &SigningMaterial) -> Result<Signed, String> {
    let text: String = answer.nfc().collect();
    let digest = sha2_digest(text.as_bytes());

    // The claim signer and the identity credential are one certificate: the
    // operator signs the manifest and asserts, in the same manifest, that it
    // is the operator.
    let combined = c2pa::create_signer::from_x509_identity(
        bounded_signer(material)?,
        bounded_signer(material)?,
        &["c2pa.actions", SOURCE_RECORD_LABEL],
        &[OPERATOR_ROLE],
    );
    let context = Context::new().with_signer(combined).into_shared();
    let definition = serde_json::to_string(definition).map_err(|error| error.to_string())?;

    let mut wrapper_bytes = c2pa_text::worst_case_wrapper_byte_length(2048);
    let mut manifest = Vec::new();
    let mut settled = false;
    for _ in 0..SIGNING_ROUNDS {
        let mut builder = Builder::from_shared_context(&context)
            .with_definition(definition.as_str())
            .map_err(|error| format!("manifest definition: {error}"))?;
        let mut binding = DataHash::new(DATA_HASH_NAME, "sha256");
        binding.add_exclusion(HashRange::new(text.len() as u64, wrapper_bytes as u64));
        binding.set_hash(digest.clone());
        builder
            .add_assertion(c2pa::assertions::labels::DATA_HASH, &binding)
            .map_err(|error| format!("data hash: {error}"))?;
        manifest = builder
            .sign_embeddable(RAW_MANIFEST_FORMAT)
            .map_err(|error| format!("signing: {error}"))?;
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
    let mut settings = c2pa::settings::Settings::default();
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
    let extracted = c2pa_text::extract_manifest(text)
        .map_err(|error| ReadBackError::Wrapper(error.to_string()))?;
    let Some(manifest_bytes) = extracted.manifest else {
        return Err(ReadBackError::NoLabel);
    };
    let reader = Reader::from_context(context)
        .with_manifest_data_and_stream(
            &manifest_bytes,
            TEXT_FORMAT,
            Cursor::new(text.as_bytes().to_vec()),
        )
        .map_err(|error| ReadBackError::Manifest(error.to_string()))?;
    let view: Value = serde_json::from_str(&reader.json())
        .map_err(|error| ReadBackError::Manifest(error.to_string()))?;
    let label = reader
        .active_label()
        .ok_or_else(|| ReadBackError::Manifest("no active manifest".to_owned()))?
        .to_owned();
    let active = view["manifests"][&label].clone();

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
    let assertion = |wanted: &str| {
        active["assertions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|assertion| assertion["label"] == wanted)
            .map(|assertion| assertion["data"].clone())
            .unwrap_or(Value::Null)
    };

    Ok(ReadBack {
        clean_text_hash: sha256_digest(extracted.clean_text.as_bytes()),
        clean_text_bytes: extracted.clean_text.len(),
        wrapper_bytes: extracted.length,
        manifest_bytes: manifest_bytes.len(),
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
        signature: active["signature_info"].clone(),
        ingredients,
        actions: assertion("c2pa.actions.v2"),
        training_mining: assertion(TRAINING_MINING_LABEL),
        source_record: assertion(SOURCE_RECORD_LABEL),
        identity: assertion("cawg.identity"),
        manifest: view,
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
/// source position whose reference, hash or grade differs.
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
        for field in ["reference", "content_hash", "grade"] {
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
    fn unlabelled_text_reads_back_as_no_label() {
        assert!(matches!(
            read_back("plain text, no wrapper"),
            Err(ReadBackError::NoLabel)
        ));
    }
}
