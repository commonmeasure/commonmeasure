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
//! establish who holds the key. That is a blind spot, stated below and in
//! every record.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use base64::Engine as _;
use chrono::Utc;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::release::{
    MEDIA_TYPE, Release, ReleaseRecord, SCHEME, VerificationState, signed_message,
};
use commonmeasure_types::{Decision, Gap, GapReason, PolicyMode};
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
strict refuses and observe and prefer record and carry.\n";

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

const BLIND_SPOTS: &[&str] = &[
    "The signature is verified under the key the sidecar names. Verified means the holder \
     of that key signed this text and these fields; it does not establish who holds the \
     key, since no key is pinned or looked up for the source.",
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

/// Read and verify a body served under the release media type.
///
/// `coded` is the content coding and the coded bytes where the origin served
/// the body under one; the input hash is over the bytes as served.
pub fn invoke(
    mode: PolicyMode,
    reference: &str,
    body: &[u8],
    coded: Option<&[u8]>,
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
                BLIND_SPOTS.to_vec(),
            );
            return (invocation, Outcome::Unreadable { reason });
        }
    };

    let (state, failure) = verify(&release);
    let ruling = match failure {
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
            "text_recorded": false,
        }),
        ruling.gap().cloned().into_iter().collect(),
        BLIND_SPOTS.to_vec(),
    );
    let record = ReleaseRecord {
        media_type: MEDIA_TYPE.to_owned(),
        verification: state,
        reason: failure.map(str::to_owned),
        sidecar: release.sidecar.clone(),
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

fn state_named(state: VerificationState) -> &'static str {
    match state {
        VerificationState::Verified => "verified",
        VerificationState::Failed => "failed",
        VerificationState::Missing => "missing",
    }
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
        };
        let key = SigningKey::from_bytes(&[7; 32]);
        let engine = base64::engine::general_purpose::STANDARD;
        Release {
            text,
            sidecar: Sidecar {
                signature: engine.encode(key.sign(&signed_message(&signed)).to_bytes()),
                signer_public_key: engine.encode(key.verifying_key().as_bytes()),
                signature_scheme: SCHEME.into(),
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
