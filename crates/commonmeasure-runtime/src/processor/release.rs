//! The release verifier: a deterministic `admit`-stage processor.
//!
//! A body served under the signed-release media type
//! ([`commonmeasure_types::release::MEDIA_TYPE`]) is the source's text with a
//! sidecar of passages, keep-intact markers, terms and attribution, signed
//! with Ed25519. This processor reads that body, checks the sidecar's
//! structure against the text, and verifies the signature against the text.
//! The release format is a working one that Common Measure reads and does not
//! define (`commonmeasure_types::release`).
//!
//! Two outcomes are kept apart. A body that does not read as a release (not
//! JSON of the format, or a passage or marker that does not lie in the text)
//! is unavailable in every mode: the edge never hands it over as plain text,
//! because the source served it as a release. A release that reads but whose
//! signature fails or is missing is a breach under the shared mode
//! discipline: `strict` refuses it, `observe` and `prefer` carry it with the
//! breach recorded (`docs/FAIL-POLICY.md` §6).
//!
//! Verification uses the key the sidecar names. It establishes that the
//! holder of that key signed this text and these fields; it does not
//! establish who holds the key. Where the source policy has a
//! `release_signer` term for the host that served the release, the key is
//! then checked against the keys the operator anchored there: a key outside
//! them is a breach under the same discipline, and a key inside them closes
//! the blind spot to "the operator anchored this key to this host", which is
//! still not who holds it. Where no term matches, the blind spot stays whole.
//! Either way it is stated below and in every record.
//!
//! A sidecar may also carry a C2PA manifest store the source signed with its
//! own certificate ([`commonmeasure_types::release::Sidecar::c2pa`]). It is
//! read back offline against the text and its state recorded beside the
//! verification state; it neither admits nor refuses the release, which
//! the Ed25519 signature alone does (`docs/contracts/display-provenance.md`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use base64::Engine as _;
use chrono::Utc;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::release::{
    Anchoring, C2paBinding, C2paCoverage, C2paRecord, C2paState, MEDIA_TYPE, Release,
    ReleaseRecord, SCHEME, VerificationState, signed_message,
};
use commonmeasure_types::{
    Constraint, Decision, Gap, GapReason, PolicyMode, SignerKey, release_signer_for,
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::json;

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use crate::agent_text;
use crate::agent_text::AgentText;
use crate::policy::{Ruling, approximate_tokens};

pub const NAME: &str = "release-verifier";
pub const VERSION: &str = "1";

/// The rules, stated canonically. The configuration digest covers this text.
const RULES: &str = "\
media type: a body whose Content-Type names application/x.signed-release+json, parameters \
and case aside, is read as a signed release; no other body reaches this processor.\n\
reading: the body is UTF-8 JSON with `text` and `sidecar`; the sidecar has `signed`, \
`signature`, `signer_public_key` and `signature_scheme`; `signed` has source, title, revid, \
url, licence, attribution, terms, display_mode (labelled where absent), text_sha256, \
passages, markers and released_at, and no other field, nor does a passage or marker carry \
a field outside its own. Passage ids are unique; every passage and marker is a byte span of \
the text on character boundaries; every marker names a passage and lies within it. A body \
that fails any of these does not read as a release and is unavailable in every mode.\n\
verification: missing where the signature or the signer key is empty; otherwise failed \
where the scheme is not ed25519-over-sha256(text)-and-canonical-signed-json, where the \
lower-case hex SHA-256 of the text differs from text_sha256, where the key is not 32 bytes \
of standard base64, where the signature is not 64 bytes of standard base64, or where the \
Ed25519 signature does not verify under that key over text_sha256, a newline and the RFC \
8785 canonical JSON of signed; verified otherwise. Verification is strict: a key or a \
signature R of small order, or a non-canonical S, fails.\n\
verdicts: verified admits; failed and missing are a breach under the shared mode discipline, \
strict refuses and observe and prefer record and carry.\n\
anchoring: the host of the URL the release was served from, lower-case with trailing dots \
removed, is looked up among the source policy's release_signer terms in declaration order, \
and the first whose host pattern matches applies. Where one applies, the signer key is \
anchored where its standard base64 decodes to the 32 bytes of a key the term names; a \
verified release whose key is not anchored is a breach under the shared mode discipline. \
The record names the term's host pattern and whether the key is anchored, never the keys \
the term names; where no term applies it says none.\n\
c2pa: where the sidecar carries c2pa, it is decoded from standard base64 and read with the \
C2PA SDK reader against the NFC-normalised text, offline, with the configured trust anchors \
where any are supplied; the state is invalid where it does not decode, parse or verify, \
untrusted where it verifies and its signer chains to no supplied anchor, trusted where it \
does. A store whose c2pa.hash.data assertions exclude any range is invalid whatever the \
reader reports and its binding does not match; a store whose exclusions cannot be read is \
invalid. The record carries the state, the store's hash, whether the data hash binds the \
whole text and matches it, the number of excluded ranges and their summed length where there \
are any, whether that text is the one text_sha256 names, the claim signer, each \
cawg.identity summarised, the source's cawg.training-mining and the status codes. The state \
neither admits nor refuses the release.\n";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Admit,
    capability: "signed-release-verification",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: NO_AMBIENT_AUTHORITY,
    determinism: Determinism::Deterministic,
    limits: "in-process and synchronous; one JSON parse, one SHA-256 and one Ed25519 \
             verification per body, no supervisor timeout",
    // A body served as a release that cannot be read is never delivered as
    // anything else, whatever the mode.
    failure_by_mode: FailureByMode {
        strict: FailBehaviour::FailClosed,
        prefer: FailBehaviour::FailClosed,
        observe: FailBehaviour::FailClosed,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

/// The first blind spot where no `release_signer` term applies.
const UNANCHORED: &str = "The signature is verified under the key the sidecar names. Verified \
     means the holder of that key signed this text and these fields; it does not establish who \
     holds the key, since no release_signer term of the source policy matches this host and no \
     key is pinned or looked up for the source.";

/// The first blind spot where a `release_signer` term applies.
const ANCHORED: &str = "The signature is verified under the key the sidecar names, and that \
     key is checked against the keys a release_signer term of the source policy anchors to \
     this host. An anchored key establishes that the operator anchored this key to this host; \
     it does not establish who holds the key or how the operator came to trust it.";

const BLIND_SPOTS: &[&str] = &[
    "The release format is a working format that Common Measure reads and does not define; \
     a release in another format or under another media type is not recognised and is \
     treated as any other body.",
    "The signed fields are the source's statements: verification says they are unaltered \
     since signing, not that the terms, attribution or passages are accurate.",
    "A field outside the format in the signed fields, a passage or a marker makes the body \
     unreadable as a release; fields outside the format elsewhere in the body or sidecar are \
     not read, verified or recorded.",
];

/// A release that read and was ruled on.
#[derive(Debug)]
pub struct Read {
    pub release: Release,
    /// What the crossing record keeps: the sidecar and the verification
    /// state, never the text.
    pub record: ReleaseRecord,
    /// The verdict under the mode: a failed or missing signature is a breach.
    pub ruling: Ruling,
    /// SHA-256 over the release text, in the `sha256:` form.
    pub content_hash: String,
}

/// What the verifier made of a body served as a release.
#[derive(Debug)]
pub enum Outcome {
    Read(Box<Read>),
    /// The body did not read as a release. The reason is this processor's
    /// own sentence and names nothing the source chose.
    Unreadable {
        reason: &'static str,
    },
}

/// Read and verify a body served under the release media type, with no
/// `release_signer` terms and no C2PA trust anchors.
///
/// `coded` is the content coding and the coded bytes where the origin served
/// the body under one; the input hash is over the bytes as served.
pub fn invoke(
    mode: PolicyMode,
    reference: &str,
    body: &[u8],
    coded: Option<&[u8]>,
) -> (Invocation, Outcome) {
    invoke_with_trust(mode, reference, body, coded, &[], None)
}

/// [`invoke`], checking the signer key against the `release_signer` terms in
/// `constraints`, the effective source policy's, and reading a C2PA store the
/// sidecar carries against `trust_anchors`, a PEM bundle the operator
/// supplied, where there is one. `reference` is the URL the release was
/// served from; its host selects the term.
pub fn invoke_with_trust(
    mode: PolicyMode,
    reference: &str,
    body: &[u8],
    coded: Option<&[u8]>,
    constraints: &[Constraint],
    trust_anchors: Option<&str>,
) -> (Invocation, Outcome) {
    let started_at = Utc::now();
    let retrieved_hash = sha256_digest(coded.unwrap_or(body));
    let input = ArtefactRef {
        reference: reference.to_owned(),
        content_hash: Some(retrieved_hash),
        tokens: None,
    };
    let release = match read(body) {
        Ok(release) => release,
        Err(reason) => {
            let gap = Gap::new(
                GapReason::CapabilityUnavailable,
                format!(
                    "{reference} was served as {MEDIA_TYPE} and does not read as one: {reason}; \
                     it is not delivered as plain text."
                ),
            );
            let invocation = Invocation::new(
                manifest(),
                started_at,
                Decision::Refuse,
                "the body read as UTF-8 JSON of the release format, and the read failed",
                vec![input],
                Vec::new(),
                json!({ "media_type": MEDIA_TYPE, "read": false, "reason": reason }),
                vec![gap],
                // Nothing was verified or anchored, so neither key sentence
                // applies.
                BLIND_SPOTS.to_vec(),
            );
            return (invocation, Outcome::Unreadable { reason });
        }
    };

    let (state, failure) = verify(&release);
    let anchoring = anchoring(reference, &release, constraints);
    let unanchored = matches!(
        anchoring,
        Anchoring::Policy {
            key_anchored: false,
            ..
        }
    );
    let ruling = match failure {
        // A key the operator did not anchor is ruled on only once the
        // signature verified: a failed or missing one is already the breach,
        // and its reason is the one the agent and the record need.
        None if unanchored => {
            let said = "The release's signer key is not one the source policy anchors to this host";
            Ruling::breach(
                mode,
                format!("{said}."),
                agent_text![said, "."],
                Gap::new(
                    GapReason::PolicyRefused,
                    format!(
                        "{reference} is a signed release that verified under a key the \
                         release_signer term for its host does not name. The sidecar and the \
                         anchoring are on the crossing record."
                    ),
                ),
            )
        }
        None => Ruling::Allowed,
        Some(reason) => {
            let said = match state {
                VerificationState::Missing => "The release carries no signature to verify",
                _ => "The release's signature did not verify",
            };
            Ruling::breach(
                mode,
                format!("{said}: {reason}."),
                agent_text![said, ": ", reason, "."],
                Gap::new(
                    GapReason::PolicyRefused,
                    format!(
                        "{reference} is a signed release whose verification is {}: {reason}. \
                         The sidecar and the state are on the crossing record.",
                        state_named(state)
                    ),
                ),
            )
        }
    };
    let content_hash = sha256_digest(release.text.as_bytes());
    let c2pa = read_c2pa(&release, trust_anchors);
    let decision = if ruling.is_refusal() {
        Decision::Refuse
    } else {
        Decision::Admit
    };
    let invocation = Invocation::new(
        manifest(),
        started_at,
        decision,
        "the body read as a release and its signature verified under the rules the \
         configuration digest pins",
        vec![input],
        // The text the release carries, by hash: what the screens rule on
        // and the agent reads.
        vec![ArtefactRef {
            reference: reference.to_owned(),
            content_hash: Some(content_hash.clone()),
            tokens: Some(approximate_tokens(&release.text)),
        }],
        json!({
            "media_type": MEDIA_TYPE,
            "read": true,
            "verification": state,
            "reason": failure,
            "signature_scheme": release.sidecar.signature_scheme,
            "signer_public_key": release.sidecar.signer_public_key,
            "text_sha256": release.sidecar.signed.text_sha256,
            "passages": release.sidecar.signed.passages.len(),
            "markers": release.sidecar.signed.markers.len(),
            "display_mode": release.sidecar.signed.display_mode,
            "c2pa": c2pa.as_ref().map_or_else(|| json!({"state": "absent"}), |c2pa| json!(c2pa)),
            "anchoring": anchoring,
            "text_recorded": false,
        }),
        ruling.gap().cloned().into_iter().collect(),
        blind_spots(&anchoring),
    );
    let record = ReleaseRecord {
        media_type: MEDIA_TYPE.to_owned(),
        verification: state,
        reason: failure.map(str::to_owned),
        sidecar: release.sidecar.clone(),
        c2pa,
        anchoring,
    };
    (
        invocation,
        Outcome::Read(Box::new(Read {
            release,
            record,
            ruling,
            content_hash,
        })),
    )
}

fn blind_spots(anchoring: &Anchoring) -> Vec<&'static str> {
    let first = match anchoring {
        Anchoring::Policy { .. } => ANCHORED,
        Anchoring::None => UNANCHORED,
    };
    std::iter::once(first)
        .chain(BLIND_SPOTS.iter().copied())
        .collect()
}

/// Which `release_signer` term applies to the host `reference` names, and
/// whether the sidecar's key is one it anchors. A key that does not decode
/// is not anchored, whatever verification made of it.
fn anchoring(reference: &str, release: &Release, constraints: &[Constraint]) -> Anchoring {
    let host = url::Url::parse(reference)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_end_matches('.').to_owned())
        })
        .unwrap_or_default();
    let Some((pattern, keys)) = release_signer_for(constraints, &host) else {
        return Anchoring::None;
    };
    let named = SignerKey::parse(&release.sidecar.signer_public_key).ok();
    Anchoring::Policy {
        host: pattern.as_written(),
        key_anchored: named.is_some_and(|named| keys.contains(&named)),
    }
}

fn state_named(state: VerificationState) -> &'static str {
    match state {
        VerificationState::Verified => "verified",
        VerificationState::Failed => "failed",
        VerificationState::Missing => "missing",
    }
}

/// Read the sidecar's C2PA store, where it carries one, against the text.
fn read_c2pa(release: &Release, trust_anchors: Option<&str>) -> Option<C2paRecord> {
    let encoded = release.sidecar.c2pa.as_deref()?;
    let mut record = C2paRecord {
        state: C2paState::Invalid,
        reason: None,
        manifest_sha256: None,
        manifest_label: None,
        binding: None,
        signer: None,
        identities: Vec::new(),
        training_mining: None,
        statuses: Vec::new(),
        verifier: format!("c2pa-rs {}", c2pa::VERSION),
    };
    let Ok(store) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        record.reason = Some("the store is not standard base64".to_owned());
        return Some(record);
    };
    record.manifest_sha256 = Some(sha256_digest(&store));
    let reading =
        match super::provenance::read_store_with_trust(&store, &release.text, trust_anchors) {
            Ok(reading) => reading,
            Err(error) => {
                record.reason = Some(error.to_string());
                return Some(record);
            }
        };
    record.statuses = reading.status_codes();
    record.state = match reading.validation["state"].as_str() {
        Some("trusted") => C2paState::Trusted,
        Some("valid") => C2paState::Untrusted,
        _ => C2paState::Invalid,
    };
    if record.state == C2paState::Invalid {
        record.reason = Some("the manifest does not verify against the release text".to_owned());
    }
    // The SDK checks the digest over whatever the exclusions leave; a
    // release's store is a sidecar and binds the whole text, so any
    // exclusion leaves part of the text unbound however the digest reads.
    let coverage = reading.data_hash_exclusions.as_ref().map(|exclusions| {
        (!exclusions.is_empty()).then(|| C2paCoverage {
            excluded_ranges: exclusions.len(),
            excluded_bytes: exclusions
                .iter()
                .fold(0, |total: u64, (_, length)| total.saturating_add(*length)),
        })
    });
    match &coverage {
        None => {
            record.state = C2paState::Invalid;
            record.reason =
                Some("the data hash's exclusions cannot be read from the store".to_owned());
        }
        Some(Some(partial)) => {
            record.state = C2paState::Invalid;
            record.reason = Some(format!(
                "partial coverage: the data hash excludes {} range(s), {} byte(s) declared, so \
                 it does not bind the whole release text",
                partial.excluded_ranges, partial.excluded_bytes
            ));
        }
        Some(None) => {}
    }
    let normalised: String =
        unicode_normalization::UnicodeNormalization::nfc(release.text.as_str()).collect();
    record.binding = Some(C2paBinding {
        matches: coverage == Some(None)
            && !record.statuses.iter().any(|code| {
                code.starts_with("assertion.dataHash") || code == "claim.hardBindings.missing"
            }),
        same_text_as_signed: format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(normalised.as_bytes())
        ) == release.sidecar.signed.text_sha256,
        coverage: coverage.flatten(),
    });
    record.manifest_label = Some(reading.manifest_label.clone());
    record.signer = Some(json!({
        "common_name": reading.signature["common_name"],
        "issuer": reading.signature["issuer"],
        "alg": reading.signature["alg"],
    }));
    record.identities = reading
        .assertions("cawg.identity")
        .iter()
        .map(|identity| super::provenance::identity_summary(identity, &reading.signature))
        .collect();
    record.training_mining = Some(reading.assertion(super::provenance::TRAINING_MINING_LABEL))
        .filter(|value| !value.is_null());
    Some(record)
}

/// Parse the body and check its structure against the text.
fn read(body: &[u8]) -> Result<Release, &'static str> {
    let release: Release = serde_json::from_slice(body)
        .map_err(|_| "the body is not UTF-8 JSON in the release format")?;
    let signed = &release.sidecar.signed;
    let mut ids = BTreeSet::new();
    for passage in &signed.passages {
        if !ids.insert(passage.id.as_str()) {
            return Err("two passages share an id");
        }
        if release.text.get(passage.start..passage.end).is_none() {
            return Err("a passage does not lie in the text");
        }
    }
    for marker in &signed.markers {
        if release.text.get(marker.start..marker.end).is_none() {
            return Err("a marker does not lie in the text");
        }
        let Some(passage) = signed.passages.iter().find(|p| p.id == marker.passage) else {
            return Err("a marker names no passage of the release");
        };
        if marker.start < passage.start || marker.end > passage.end {
            return Err("a marker does not lie within the passage it names");
        }
    }
    Ok(release)
}

/// The verification state and, where it is not `verified`, why.
fn verify(release: &Release) -> (VerificationState, Option<&'static str>) {
    let sidecar = &release.sidecar;
    if sidecar.signature.is_empty() {
        return (
            VerificationState::Missing,
            Some("the sidecar has no signature"),
        );
    }
    if sidecar.signer_public_key.is_empty() {
        return (
            VerificationState::Missing,
            Some("the sidecar names no signer key"),
        );
    }
    let failed = |reason| (VerificationState::Failed, Some(reason));
    if sidecar.signature_scheme != SCHEME {
        return failed("the signature scheme is not one this verifier reads");
    }
    let text_sha256 = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(release.text.as_bytes())
    );
    if text_sha256 != sidecar.signed.text_sha256 {
        return failed("the text does not match its signed hash");
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let Some(key) = engine
        .decode(&sidecar.signer_public_key)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
    else {
        return failed("the signer key is not an Ed25519 public key");
    };
    let Some(signature) = engine
        .decode(&sidecar.signature)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
    else {
        return failed("the signature is not an Ed25519 signature");
    };
    // `verify_strict`: the permissive check accepts a small-order key with a
    // small-order R, which verifies any message, so the signature would bind
    // nothing to the text.
    match key.verify_strict(&signed_message(&sidecar.signed), &signature) {
        Ok(()) => (VerificationState::Verified, None),
        Err(_) => failed("the signature does not verify under the key the sidecar names"),
    }
}

/// The verdict as the agent reads it where a release did not read.
pub fn unreadable_told(reason: &'static str) -> AgentText {
    agent_text![
        "the body was served as a signed release and does not read as one: ",
        reason,
        "."
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_types::release::{KeptSpan, Passage, Sidecar, Signed};
    use ed25519_dalek::{Signer as _, SigningKey};

    fn signed_release() -> Release {
        signed_release_by(7)
    }

    /// A release signed with the key whose seed is `seed` repeated.
    fn signed_release_by(seed: u8) -> Release {
        let text = "The lead paragraph.\n\nThe mayor said: \"We accept it.\"".to_owned();
        let second = text.find("The mayor").unwrap();
        let quote = text.find('"').unwrap();
        let signed = Signed {
            source: "a source".into(),
            title: "A title".into(),
            revid: 7,
            url: "https://source.example/a".into(),
            licence: "CC BY-SA 4.0".into(),
            attribution: "A title, by its authors".into(),
            terms: vec!["Show a kept passage whole.".into()],
            display_mode: "strict".into(),
            text_sha256: format!(
                "{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
            ),
            passages: vec![
                Passage {
                    id: "p0".into(),
                    start: 0,
                    end: second - 2,
                },
                Passage {
                    id: "p1".into(),
                    start: second,
                    end: text.len(),
                },
            ],
            markers: vec![KeptSpan {
                start: quote,
                end: text.len(),
                label: "quotation".into(),
                passage: "p1".into(),
            }],
            released_at: "2026-10-13T00:00:00Z".into(),
            usage: None,
        };
        let key = SigningKey::from_bytes(&[seed; 32]);
        let engine = base64::engine::general_purpose::STANDARD;
        Release {
            text,
            sidecar: Sidecar {
                signature: engine.encode(key.sign(&signed_message(&signed)).to_bytes()),
                signer_public_key: engine.encode(key.verifying_key().as_bytes()),
                signature_scheme: SCHEME.into(),
                c2pa: None,
                signed,
            },
        }
    }

    fn body(release: &Release) -> Vec<u8> {
        serde_json::to_vec(release).unwrap()
    }

    fn read_outcome(mode: PolicyMode, release: &Release) -> (Invocation, Box<Read>) {
        match invoke(mode, "https://source.example/a", &body(release), None) {
            (invocation, Outcome::Read(read)) => (invocation, read),
            (_, Outcome::Unreadable { reason }) => panic!("unreadable: {reason}"),
        }
    }

    #[test]
    fn a_signed_release_verifies_and_the_record_keeps_no_text() {
        let release = signed_release();
        let (invocation, read) = read_outcome(PolicyMode::Strict, &release);
        assert_eq!(read.record.verification, VerificationState::Verified);
        assert!(read.record.reason.is_none());
        assert_eq!(read.ruling, Ruling::Allowed);
        assert_eq!(read.content_hash, sha256_digest(release.text.as_bytes()));
        let record = invocation.to_value();
        assert_eq!(record["decision"], "admit");
        assert_eq!(record["stage"], "admit");
        assert_eq!(record["detail"]["verification"], "verified");
        assert_eq!(record["outputs"][0]["content_hash"], read.content_hash);
        let kept = serde_json::to_string(&read.record).unwrap() + &record.to_string();
        assert!(
            !kept.contains("The mayor said"),
            "the text never enters a record"
        );
    }

    /// `release` re-signed after its signed fields were changed.
    fn resigned(mut release: Release) -> Release {
        let key = SigningKey::from_bytes(&[7; 32]);
        release.sidecar.signature = base64::engine::general_purpose::STANDARD.encode(
            key.sign(&signed_message(&release.sidecar.signed))
                .to_bytes(),
        );
        release
    }

    #[test]
    fn a_usage_block_is_signed_and_a_release_without_one_is_unchanged() {
        use commonmeasure_types::release::{Usage, UsageReporting, UsageTerm};
        let plain = signed_release();
        assert!(
            !String::from_utf8(signed_message(&plain.sidecar.signed))
                .unwrap()
                .contains("usage")
        );
        let mut release = plain.clone();
        release.sidecar.signed.usage = Some(Usage {
            ai_input: UsageTerm::Allow,
            onward_commercial_product: UsageTerm::Disallow,
            reporting: Some(UsageReporting {
                required: true,
                destination: Some("https://source.example/report".into()),
            }),
        });
        let release = resigned(release);
        let (_, read) = read_outcome(PolicyMode::Strict, &release);
        assert_eq!(read.record.verification, VerificationState::Verified);
        let mut changed = release.clone();
        changed
            .sidecar
            .signed
            .usage
            .as_mut()
            .unwrap()
            .onward_commercial_product = UsageTerm::Allow;
        let (_, read) = read_outcome(PolicyMode::Observe, &changed);
        assert_eq!(read.record.verification, VerificationState::Failed);
    }

    /// `release` with a C2PA store signed by `material` over `text`.
    fn with_store(
        mut release: Release,
        text: &str,
        material: &super::super::provenance::SigningMaterial,
    ) -> Release {
        let store = super::super::provenance::sign_source_store(
            &release.sidecar.signed.url,
            text,
            None,
            material,
        )
        .unwrap();
        release.sidecar.c2pa = Some(base64::engine::general_purpose::STANDARD.encode(store));
        release
    }

    fn c2pa_of(release: &Release, anchors: Option<&str>) -> (serde_json::Value, Box<Read>) {
        match invoke_with_trust(
            PolicyMode::Strict,
            "https://source.example/a",
            &body(release),
            None,
            &[],
            anchors,
        ) {
            (invocation, Outcome::Read(read)) => {
                (invocation.to_value()["detail"]["c2pa"].clone(), read)
            }
            (_, Outcome::Unreadable { reason }) => panic!("unreadable: {reason}"),
        }
    }

    #[test]
    fn a_release_s_own_c2pa_store_is_read_and_its_state_recorded() {
        let (publisher, root) = super::super::provenance::test_publisher();
        let plain = signed_release();

        let (detail, read) = c2pa_of(&plain, None);
        assert_eq!(detail, json!({"state": "absent"}));
        assert!(read.record.c2pa.is_none());

        let release = with_store(plain.clone(), &plain.text, &publisher);
        let (detail, read) = c2pa_of(&release, None);
        assert_eq!(read.record.verification, VerificationState::Verified);
        let c2pa = read.record.c2pa.clone().unwrap();
        assert_eq!(c2pa.state, C2paState::Untrusted, "{detail}");
        assert_eq!(
            c2pa.binding,
            Some(C2paBinding {
                matches: true,
                same_text_as_signed: true,
                coverage: None,
            })
        );
        assert_eq!(c2pa.identities.len(), 1);
        assert_eq!(c2pa.identities[0]["subject"], "Stand-in publisher, test");
        assert_eq!(c2pa.identities[0]["roles"], json!(["cawg.publisher"]));
        assert_eq!(detail["state"], "untrusted");
        assert!(!detail.to_string().contains("The mayor said"), "no text");

        let (_, read) = c2pa_of(&release, Some(&root));
        assert_eq!(read.record.c2pa.unwrap().state, C2paState::Trusted);

        // A store bound to another text is invalid; the release still
        // verifies, because its Ed25519 signature is what admits it.
        let mismatched = with_store(plain.clone(), "Another text altogether.", &publisher);
        let (detail, read) = c2pa_of(&mismatched, None);
        assert_eq!(read.record.verification, VerificationState::Verified);
        assert_eq!(read.ruling, Ruling::Allowed);
        let c2pa = read.record.c2pa.unwrap();
        assert_eq!(c2pa.state, C2paState::Invalid, "{detail}");
        assert!(!c2pa.binding.unwrap().matches);

        let mut garbled = plain;
        garbled.sidecar.c2pa = Some("not base64!".into());
        let (_, read) = c2pa_of(&garbled, None);
        let c2pa = read.record.c2pa.unwrap();
        assert_eq!(c2pa.state, C2paState::Invalid);
        assert_eq!(
            c2pa.reason.as_deref(),
            Some("the store is not standard base64")
        );
    }

    /// The SDK hashes what a data hash's exclusions leave and reads such a
    /// store as verifying, trusted under the publisher's root; a release's
    /// store must bind the whole text, so it is invalid, says how much it
    /// leaves out, and the release still verifies on its Ed25519 signature.
    #[test]
    fn a_store_whose_data_hash_excludes_any_of_the_text_is_invalid() {
        use super::super::provenance::{read_store_with_trust, sign_source_store_excluding};
        let (publisher, root) = super::super::provenance::test_publisher();
        let plain = signed_release();
        let whole = plain.text.len();
        let mayor = plain.text.find("The mayor").unwrap();
        for (excluded, bytes) in [(vec![(mayor, 9)], 9), (vec![(0, whole)], whole as u64)] {
            let store = sign_source_store_excluding(
                &plain.sidecar.signed.url,
                &plain.text,
                &excluded,
                &publisher,
            )
            .unwrap();
            let reading = read_store_with_trust(&store, &plain.text, Some(&root)).unwrap();
            assert_eq!(
                reading.validation["state"], "trusted",
                "{}",
                reading.validation
            );
            assert_eq!(
                reading.data_hash_exclusions,
                Some(
                    excluded
                        .iter()
                        .map(|&(start, length)| (start as u64, length as u64))
                        .collect()
                )
            );

            let mut release = plain.clone();
            release.sidecar.c2pa = Some(base64::engine::general_purpose::STANDARD.encode(store));
            let (detail, read) = c2pa_of(&release, Some(&root));
            assert_eq!(read.record.verification, VerificationState::Verified);
            assert_eq!(read.ruling, Ruling::Allowed);
            let c2pa = read.record.c2pa.unwrap();
            assert_eq!(c2pa.state, C2paState::Invalid, "{detail}");
            assert_eq!(
                c2pa.binding,
                Some(C2paBinding {
                    matches: false,
                    same_text_as_signed: true,
                    coverage: Some(C2paCoverage {
                        excluded_ranges: 1,
                        excluded_bytes: bytes,
                    }),
                })
            );
            assert!(
                c2pa.reason
                    .as_deref()
                    .unwrap()
                    .starts_with("partial coverage"),
                "{detail}"
            );
            assert_eq!(detail["state"], "invalid");
            assert_eq!(
                detail["binding"]["coverage"],
                json!({"excluded_ranges": 1, "excluded_bytes": bytes})
            );
            for sentence in ["The lead paragraph", "The mayor said", "We accept it"] {
                assert!(!detail.to_string().contains(sentence), "no text: {detail}");
            }
        }
    }

    #[test]
    fn a_changed_text_fails_and_only_strict_refuses() {
        let mut release = signed_release();
        release.text = release.text.replace("lead", "LEAD");
        for (mode, refused) in [
            (PolicyMode::Strict, true),
            (PolicyMode::Prefer, false),
            (PolicyMode::Observe, false),
        ] {
            let (invocation, read) = read_outcome(mode, &release);
            assert_eq!(read.record.verification, VerificationState::Failed);
            assert_eq!(
                read.record.reason.as_deref(),
                Some("the text does not match its signed hash")
            );
            assert_eq!(read.ruling.is_refusal(), refused, "{mode:?}");
            assert!(
                read.ruling.reason().is_some(),
                "{mode:?}: a breach either way"
            );
            let decision = if refused { "refuse" } else { "admit" };
            assert_eq!(invocation.to_value()["decision"], decision);
        }
    }

    #[test]
    fn a_changed_signed_field_does_not_verify() {
        let mut release = signed_release();
        release.sidecar.signed.display_mode = "labelled".into();
        let (_, read) = read_outcome(PolicyMode::Observe, &release);
        assert_eq!(read.record.verification, VerificationState::Failed);
        assert_eq!(
            read.record.reason.as_deref(),
            Some("the signature does not verify under the key the sidecar names")
        );
    }

    /// The identity point as key with an identity R and zero S: the
    /// permissive check accepts it over any message. Returns the release with
    /// that key and signature, its text altered and its hash restated.
    fn weak_key_forgery(text_change: Option<(&str, &str)>) -> Release {
        let mut release = signed_release();
        if let Some((from, to)) = text_change {
            release.text = release.text.replace(from, to);
            release.sidecar.signed.text_sha256 = format!(
                "{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(release.text.as_bytes())
            );
        }
        let engine = base64::engine::general_purpose::STANDARD;
        let mut key = [0u8; 32];
        key[0] = 1;
        let mut signature = [0u8; 64];
        signature[0] = 1;
        release.sidecar.signer_public_key = engine.encode(key);
        release.sidecar.signature = engine.encode(signature);
        release
    }

    // Catches: a permissive Ed25519 check, under which a small-order key
    // and R verify any text, so a forged sidecar reads as verified.
    #[test]
    fn a_weak_key_forgery_does_not_verify_over_any_text() {
        for release in [
            weak_key_forgery(None),
            weak_key_forgery(Some(("mayor", "minister"))),
        ] {
            for (mode, refused) in [
                (PolicyMode::Strict, true),
                (PolicyMode::Prefer, false),
                (PolicyMode::Observe, false),
            ] {
                let (invocation, read) = read_outcome(mode, &release);
                assert_eq!(read.record.verification, VerificationState::Failed);
                assert_eq!(
                    read.record.reason.as_deref(),
                    Some("the signature does not verify under the key the sidecar names")
                );
                assert_eq!(read.ruling.is_refusal(), refused, "{mode:?}");
                assert!(read.ruling.reason().is_some(), "{mode:?}: a breach");
                let decision = if refused { "refuse" } else { "admit" };
                assert_eq!(invocation.to_value()["detail"]["verification"], "failed");
                assert_eq!(invocation.to_value()["decision"], decision);
            }
        }
    }

    // Catches: a signature that is checked against the signed hash alone and
    // not the text, so a genuine signature carries over to another text.
    #[test]
    fn a_genuine_signature_over_another_text_does_not_verify() {
        let mut release = signed_release();
        release.text = release.text.replace("mayor", "minister");
        release.sidecar.signed.text_sha256 = format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(release.text.as_bytes())
        );
        let (_, read) = read_outcome(PolicyMode::Strict, &release);
        assert_eq!(read.record.verification, VerificationState::Failed);
        assert_eq!(
            read.record.reason.as_deref(),
            Some("the signature does not verify under the key the sidecar names")
        );
        assert!(read.ruling.is_refusal());
    }

    #[test]
    fn a_missing_signature_is_a_breach_named_missing() {
        let mut release = signed_release();
        release.sidecar.signature.clear();
        let (_, read) = read_outcome(PolicyMode::Strict, &release);
        assert_eq!(read.record.verification, VerificationState::Missing);
        assert!(read.ruling.is_refusal());
        assert!(
            read.ruling
                .reason()
                .unwrap()
                .starts_with("The release carries no signature")
        );
    }

    #[test]
    fn a_body_that_does_not_read_is_unreadable_in_every_mode() {
        let mut outside = signed_release();
        outside.sidecar.signed.passages[1].end = outside.text.len() + 1;
        let mut orphan = signed_release();
        orphan.sidecar.signed.markers[0].passage = "p9".into();
        // A marker naming p0 that spans p1: correctly signed, and it would
        // carry p1's words labelled as p0's.
        let mut astray = signed_release();
        astray.sidecar.signed.markers[0].passage = "p0".into();
        let engine = base64::engine::general_purpose::STANDARD;
        let key = SigningKey::from_bytes(&[7; 32]);
        astray.sidecar.signature =
            engine.encode(key.sign(&signed_message(&astray.sidecar.signed)).to_bytes());
        // A field outside the format in the signed fields, added without
        // re-signing: serde would otherwise drop it and verify the rest.
        let mut extra: serde_json::Value = serde_json::to_value(signed_release()).unwrap();
        extra["sidecar"]["signed"]["extra_condition"] = json!("pay on every read");
        let mut extra_marker: serde_json::Value = serde_json::to_value(signed_release()).unwrap();
        extra_marker["sidecar"]["signed"]["markers"][0]["note"] = json!("an aside");
        for (bytes, expected) in [
            (
                b"not json".to_vec(),
                "the body is not UTF-8 JSON in the release format",
            ),
            (body(&outside), "a passage does not lie in the text"),
            (body(&orphan), "a marker names no passage of the release"),
            (
                body(&astray),
                "a marker does not lie within the passage it names",
            ),
            (
                serde_json::to_vec(&extra).unwrap(),
                "the body is not UTF-8 JSON in the release format",
            ),
            (
                serde_json::to_vec(&extra_marker).unwrap(),
                "the body is not UTF-8 JSON in the release format",
            ),
        ] {
            for mode in [PolicyMode::Strict, PolicyMode::Prefer, PolicyMode::Observe] {
                let (invocation, outcome) = invoke(mode, "https://source.example/a", &bytes, None);
                match outcome {
                    Outcome::Unreadable { reason } => assert_eq!(reason, expected),
                    Outcome::Read(_) => panic!("read a body that should not read"),
                }
                let record = invocation.to_value();
                assert_eq!(record["decision"], "refuse");
                assert_eq!(record["gaps"][0]["reason"], "capability_unavailable");
            }
        }
    }

    /// The public key whose seed is `seed` repeated, as a sidecar names it.
    fn public_key(seed: u8) -> String {
        base64::engine::general_purpose::STANDARD.encode(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .as_bytes(),
        )
    }

    fn release_signer(host: &str, seeds: &[u8]) -> Constraint {
        let keys: Vec<String> = seeds.iter().map(|seed| public_key(*seed)).collect();
        serde_json::from_value(json!({"kind": "release_signer", "host": host, "keys": keys}))
            .unwrap()
    }

    fn anchored_outcome(
        mode: PolicyMode,
        release: &Release,
        constraints: &[Constraint],
    ) -> (serde_json::Value, Box<Read>) {
        match invoke_with_trust(
            mode,
            "https://news.source.example/a",
            &body(release),
            None,
            constraints,
            None,
        ) {
            (invocation, Outcome::Read(read)) => (invocation.to_value(), read),
            (_, Outcome::Unreadable { reason }) => panic!("unreadable: {reason}"),
        }
    }

    const MODES: [PolicyMode; 3] = [PolicyMode::Strict, PolicyMode::Prefer, PolicyMode::Observe];

    #[test]
    fn a_key_the_operator_anchored_to_the_host_verifies_anchored_in_every_mode() {
        let release = signed_release();
        let terms = [release_signer("*.source.example", &[7])];
        for mode in MODES {
            let (record, read) = anchored_outcome(mode, &release, &terms);
            assert_eq!(read.record.verification, VerificationState::Verified);
            assert_eq!(read.ruling, Ruling::Allowed, "{mode:?}");
            assert_eq!(
                read.record.anchoring,
                Anchoring::Policy {
                    host: "*.source.example".into(),
                    key_anchored: true,
                }
            );
            assert_eq!(record["decision"], "admit");
            assert_eq!(
                record["detail"]["anchoring"],
                json!({"applied": "policy", "host": "*.source.example", "key_anchored": true})
            );
            let blind_spots = record["blind_spots"].to_string();
            assert!(blind_spots.contains("the operator anchored this key to this host"));
            assert!(!blind_spots.contains("no release_signer term"));
        }
    }

    // Catches: a sidecar that names its own key passing as the source's
    // where the operator anchored another key to the host.
    #[test]
    fn a_key_the_operator_did_not_anchor_is_a_breach_and_only_strict_refuses() {
        let release = signed_release();
        let terms = [release_signer("news.source.example", &[8])];
        for (mode, refused) in [
            (PolicyMode::Strict, true),
            (PolicyMode::Prefer, false),
            (PolicyMode::Observe, false),
        ] {
            let (record, read) = anchored_outcome(mode, &release, &terms);
            assert_eq!(read.record.verification, VerificationState::Verified);
            assert!(read.record.reason.is_none(), "the signature verified");
            assert_eq!(
                read.record.anchoring,
                Anchoring::Policy {
                    host: "news.source.example".into(),
                    key_anchored: false,
                }
            );
            assert_eq!(read.ruling.is_refusal(), refused, "{mode:?}");
            assert_eq!(
                read.ruling.reason(),
                Some("The release's signer key is not one the source policy anchors to this host."),
                "{mode:?}: a breach either way"
            );
            let decision = if refused { "refuse" } else { "admit" };
            assert_eq!(record["decision"], decision);
            assert_eq!(record["gaps"][0]["reason"], "policy_refused");
            assert_eq!(
                record["detail"]["anchoring"],
                json!({"applied": "policy", "host": "news.source.example", "key_anchored": false})
            );
            let kept = record.to_string() + &serde_json::to_string(&read.record).unwrap();
            assert!(
                !kept.contains(&public_key(8)),
                "the record never carries the term's keys"
            );
        }
    }

    /// A signature that fails is the breach the record names, whatever the
    /// anchoring says.
    #[test]
    fn a_failed_signature_under_an_unanchored_key_is_named_as_the_failure() {
        let mut release = signed_release();
        release.text = release.text.replace("lead", "LEAD");
        let terms = [release_signer("news.source.example", &[8])];
        let (_, read) = anchored_outcome(PolicyMode::Strict, &release, &terms);
        assert_eq!(read.record.verification, VerificationState::Failed);
        assert!(read.ruling.is_refusal());
        assert!(
            read.ruling
                .reason()
                .unwrap()
                .starts_with("The release's signature did not verify")
        );
        assert!(matches!(
            read.record.anchoring,
            Anchoring::Policy {
                key_anchored: false,
                ..
            }
        ));
    }

    #[test]
    fn with_no_term_for_the_host_verified_stands_and_the_blind_spot_is_named() {
        let release = signed_release();
        for terms in [vec![], vec![release_signer("other.example", &[8])]] {
            for mode in MODES {
                let (record, read) = anchored_outcome(mode, &release, &terms);
                assert_eq!(read.record.verification, VerificationState::Verified);
                assert_eq!(read.ruling, Ruling::Allowed);
                assert_eq!(read.record.anchoring, Anchoring::None);
                assert_eq!(record["detail"]["anchoring"], json!({"applied": "none"}));
                assert!(
                    record["blind_spots"]
                        .to_string()
                        .contains("no release_signer term of the source policy matches this host")
                );
            }
        }
        // A record written before anchoring existed reads as none.
        let (_, read) = anchored_outcome(PolicyMode::Strict, &release, &[]);
        let mut written = serde_json::to_value(&read.record).unwrap();
        written.as_object_mut().unwrap().remove("anchoring");
        let earlier: ReleaseRecord = serde_json::from_value(written).unwrap();
        assert_eq!(earlier.anchoring, Anchoring::None);
    }

    #[test]
    fn during_a_rotation_either_anchored_key_verifies() {
        let terms = [release_signer("news.source.example", &[7, 8])];
        for seed in [7, 8] {
            let (_, read) = anchored_outcome(PolicyMode::Strict, &signed_release_by(seed), &terms);
            assert_eq!(read.record.verification, VerificationState::Verified);
            assert_eq!(read.ruling, Ruling::Allowed, "key {seed}");
            assert!(matches!(
                read.record.anchoring,
                Anchoring::Policy {
                    key_anchored: true,
                    ..
                }
            ));
        }
        let (_, read) = anchored_outcome(PolicyMode::Strict, &signed_release_by(9), &terms);
        assert!(read.ruling.is_refusal(), "a key outside the set");
    }

    // Catches: a later, broader term overriding the first one that matches.
    #[test]
    fn the_first_matching_term_decides() {
        let release = signed_release();
        let terms = [
            release_signer("news.source.example", &[8]),
            release_signer("*.source.example", &[7]),
        ];
        let (_, read) = anchored_outcome(PolicyMode::Strict, &release, &terms);
        assert_eq!(
            read.record.anchoring,
            Anchoring::Policy {
                host: "news.source.example".into(),
                key_anchored: false,
            }
        );
        assert!(read.ruling.is_refusal());
        let (_, read) = anchored_outcome(PolicyMode::Strict, &release, &[terms[1].clone()]);
        assert_eq!(read.ruling, Ruling::Allowed);
    }

    #[test]
    fn the_input_hash_is_over_the_bytes_as_served() {
        let release = signed_release();
        let coded = b"gzip bytes as served".to_vec();
        let (invocation, _) = invoke(
            PolicyMode::Observe,
            "https://source.example/a",
            &body(&release),
            Some(&coded),
        );
        assert_eq!(
            invocation.to_value()["inputs"][0]["content_hash"],
            sha256_digest(&coded)
        );
    }
}
