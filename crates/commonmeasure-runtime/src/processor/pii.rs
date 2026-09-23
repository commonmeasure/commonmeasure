//! The PII detector: a deterministic `admit`-stage processor.
//!
//! Pattern rules over the text of a source, before that text can reach a
//! model. The verdict is evidence, not deletion: the finding is recorded the
//! same way whatever is done with it. In `observe` and `prefer` the crossing
//! carries on with the finding recorded, as every breach does. In `strict`
//! the finding refuses the crossing when the source is internal or private,
//! or when the policy sets `refuse_on_pii`; on a public source `strict`
//! records the finding and admits the crossing, because a public page's
//! published contact details are not the personal data the detector exists
//! to keep out of a model (`docs/FAIL-POLICY.md`, clause 6). This is the one
//! processor whose strict disposition depends on where the text came from,
//! and the caller says which, from the classification policy already makes.
//!
//! The matched text is deliberately never recorded. A finding names its
//! category and byte offsets; copying an identifier into the evidence trail
//! would spread the thing the detector exists to catch, and the offsets are
//! checkable against the sealed source bytes without it.

use commonmeasure_types::canonical::sha256_digest;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::Utc;
use commonmeasure_types::{Decision, Gap, GapReason, PolicyMode};
use serde::Serialize;
use serde_json::json;

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use crate::policy::{Ruling, approximate_tokens};

pub const NAME: &str = "pii-detector";
pub const VERSION: &str = "1";

/// The rule set, stated canonically. The manifest's configuration digest
/// covers exactly this text: the detector's behaviour cannot change without
/// the digest changing.
const RULES: &str = "\
email-address: a local part of [A-Za-z0-9._%+-] before '@', then domain labels of \
letters, digits and hyphens joined by dots, ending in an alphabetic label of at \
least two characters\n\
payment-card-number: 13 to 19 digits, optionally separated by single spaces or \
hyphens, passing the Luhn check, not adjacent to further digits\n\
national-insurance-number: two upper-case letters excluding D, F, I, Q, U and V \
(and O in second position), six digits, and a final letter A to D, with optional \
single spaces between pairs\n\
international-phone-number: '+' followed by 8 to 15 digits with optional single \
spaces, hyphens or parentheses, not preceded by a letter or digit\n";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Admit,
    capability: "pii-detection",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: NO_AMBIENT_AUTHORITY,
    determinism: Determinism::Deterministic,
    limits: "in-process and synchronous; one pass over the text, no supervisor timeout",
    failure_by_mode: FailureByMode {
        strict: FailBehaviour::FailClosed,
        prefer: FailBehaviour::FailOpen,
        observe: FailBehaviour::FailOpen,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "Pattern rules detect structured identifiers only; names, postal addresses and \
     other free-text personal data are not detected.",
    "Only crossings this runtime carries are scanned; an observed crossing has \
     already happened and is recorded, not scanned.",
    "A mediated HTML page is scanned as its extracted text, so an identifier that \
     appears only in markup, a script or a comment is not scanned, and a finding's \
     offsets index the extracted text rather than the bytes the origin served.",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiCategory {
    EmailAddress,
    PaymentCardNumber,
    NationalInsuranceNumber,
    InternationalPhoneNumber,
}

impl PiiCategory {
    fn label(self) -> &'static str {
        match self {
            Self::EmailAddress => "email_address",
            Self::PaymentCardNumber => "payment_card_number",
            Self::NationalInsuranceNumber => "national_insurance_number",
            Self::InternationalPhoneNumber => "international_phone_number",
        }
    }
}

/// Where the scanned text came from, as the operator's policy already
/// classifies it. `Internal` covers a named internal prefix, a loopback or
/// private address reached under `allow_private_hosts`, and the operator's
/// own corpus; `Public` is everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    Public,
    Internal,
}

/// One identifier found in the text, by category and byte offsets only.
#[derive(Debug, Clone, Serialize)]
pub struct PiiFinding {
    pub category: PiiCategory,
    pub start: usize,
    pub end: usize,
}

/// Scan one source's text and turn the verdict into the shared `Ruling`
/// shape.
///
/// `source` is the caller's classification of where the text came from and
/// `refuse_on_pii` the policy's switch; together with `mode` they decide
/// whether a finding refuses (`strict`, and the source is internal or the
/// switch is set) or is carried with the finding recorded. `source_ref`
/// names the source in the record; `basis` names what was scanned in the
/// refusal sentence — the extracted text of a page, or the unextracted body
/// with its content type — so a reader of the sentence knows which text the
/// finding's offsets index.
pub fn invoke(
    mode: PolicyMode,
    source: SourceClass,
    refuse_on_pii: bool,
    source_ref: &str,
    basis: &str,
    text: &str,
    content_hash: Option<&str>,
) -> (Invocation, Ruling) {
    let started_at = Utc::now();
    let findings = scan(text);

    let ruling = if findings.is_empty() {
        Ruling::Allowed
    } else {
        let summary = categories(&findings)
            .into_iter()
            .map(|(category, count)| format!("{} ×{count}", category.label()))
            .collect::<Vec<_>>()
            .join(", ");
        let reason = format!(
            "The PII detector found {} structured personal identifier{} ({summary}) in {basis}.",
            findings.len(),
            if findings.len() == 1 { "" } else { "s" },
        );
        let gap = Gap::new(
            GapReason::PolicyRefused,
            format!(
                "{source_ref} carries structured personal identifiers ({summary}); the \
                 finding is recorded in the plan's processor invocations."
            ),
        );
        // The one departure from `Ruling::breach`: strict refuses a finding
        // on an internal or private source, or everywhere under the switch,
        // and otherwise carries it. The finding is recorded identically
        // whichever way it is ruled.
        let refuses =
            mode == PolicyMode::Strict && (refuse_on_pii || source == SourceClass::Internal);
        if refuses {
            Ruling::Refused { reason, gap }
        } else {
            Ruling::AllowedWithBreach { reason, gap }
        }
    };

    let decision = if ruling.is_refusal() {
        Decision::Refuse
    } else {
        Decision::Admit
    };
    let invocation = Invocation::new(
        manifest(),
        started_at,
        decision,
        "one deterministic pass of the pattern rules the configuration digest pins",
        vec![ArtefactRef {
            reference: source_ref.to_owned(),
            content_hash: content_hash.map(str::to_owned),
            tokens: Some(approximate_tokens(text)),
        }],
        // The admit stage transforms nothing: the record is the output.
        Vec::new(),
        json!({
            "findings": findings,
            "categories": categories(&findings)
                .into_iter()
                .map(|(category, count)| (category.label().to_owned(), json!(count)))
                .collect::<serde_json::Map<_, _>>(),
            "matched_text_recorded": false,
        }),
        ruling.gap().cloned().into_iter().collect(),
        BLIND_SPOTS.to_vec(),
    );
    (invocation, ruling)
}

fn categories(findings: &[PiiFinding]) -> BTreeMap<PiiCategory, usize> {
    let mut counts = BTreeMap::new();
    for finding in findings {
        *counts.entry(finding.category).or_default() += 1;
    }
    counts
}

/// All findings, in byte order. Deterministic: same text, same findings.
pub fn scan(text: &str) -> Vec<PiiFinding> {
    let mut findings = Vec::new();
    emails(text, &mut findings);
    card_numbers(text, &mut findings);
    national_insurance_numbers(text, &mut findings);
    phone_numbers(text, &mut findings);
    findings.sort_by_key(|finding| (finding.start, finding.end));
    findings
}

fn is_local_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_domain_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

fn valid_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let shaped = labels
        .iter()
        .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'));
    let top = labels.last().expect("split yields at least one label");
    shaped && top.len() >= 2 && top.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn emails(text: &str, findings: &mut Vec<PiiFinding>) {
    let bytes = text.as_bytes();
    for at in 0..bytes.len() {
        if bytes[at] != b'@' {
            continue;
        }
        let mut start = at;
        while start > 0 && is_local_part(bytes[start - 1]) {
            start -= 1;
        }
        if start == at {
            continue;
        }
        let mut end = at + 1;
        while end < bytes.len() && is_domain_part(bytes[end]) {
            end += 1;
        }
        while end > at + 1 && matches!(bytes[end - 1], b'.' | b'-') {
            end -= 1;
        }
        if valid_domain(&text[at + 1..end]) {
            findings.push(PiiFinding {
                category: PiiCategory::EmailAddress,
                start,
                end,
            });
        }
    }
}

fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(position, digit)| {
            let value = u32::from(digit - b'0');
            if position % 2 == 1 {
                let doubled = value * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                value
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn card_numbers(text: &str, findings: &mut Vec<PiiFinding>) {
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if !bytes[cursor].is_ascii_digit() || (cursor > 0 && bytes[cursor - 1].is_ascii_digit()) {
            cursor += 1;
            continue;
        }
        // A run of digits with single spaces or hyphens between groups.
        let start = cursor;
        let mut digits = Vec::new();
        let mut end = cursor;
        let mut position = cursor;
        while position < bytes.len() {
            if bytes[position].is_ascii_digit() {
                digits.push(bytes[position]);
                position += 1;
                end = position;
            } else if matches!(bytes[position], b' ' | b'-')
                && position + 1 < bytes.len()
                && bytes[position + 1].is_ascii_digit()
            {
                position += 1;
            } else {
                break;
            }
        }
        if (13..=19).contains(&digits.len()) && luhn(&digits) {
            findings.push(PiiFinding {
                category: PiiCategory::PaymentCardNumber,
                start,
                end,
            });
        }
        cursor = end.max(cursor + 1);
    }
}

/// Letters that cannot open a National Insurance number.
fn valid_ni_prefix(first: u8, second: u8) -> bool {
    let excluded = |letter: u8| matches!(letter, b'D' | b'F' | b'I' | b'Q' | b'U' | b'V');
    first.is_ascii_uppercase()
        && second.is_ascii_uppercase()
        && !excluded(first)
        && !excluded(second)
        && second != b'O'
}

fn national_insurance_numbers(text: &str, findings: &mut Vec<PiiFinding>) {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let Some(end) = match_ni(&bytes[start..]) else {
            continue;
        };
        let end = start + end;
        if end < bytes.len() && bytes[end].is_ascii_alphanumeric() {
            continue;
        }
        findings.push(PiiFinding {
            category: PiiCategory::NationalInsuranceNumber,
            start,
            end,
        });
    }
}

/// Match `QQ 12 34 56 C` with optional single spaces between pairs, returning
/// the matched length.
fn match_ni(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 9 || !valid_ni_prefix(*bytes.first()?, *bytes.get(1)?) {
        return None;
    }
    let mut position = 2;
    let mut digits = 0;
    while digits < 6 {
        if bytes.get(position) == Some(&b' ') && digits % 2 == 0 {
            position += 1;
        }
        if !bytes.get(position)?.is_ascii_digit() {
            return None;
        }
        position += 1;
        digits += 1;
    }
    if bytes.get(position) == Some(&b' ') {
        position += 1;
    }
    matches!(*bytes.get(position)?, b'A'..=b'D').then_some(position + 1)
}

fn phone_numbers(text: &str, findings: &mut Vec<PiiFinding>) {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'+' || (start > 0 && bytes[start - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let mut digits = 0;
        let mut end = start + 1;
        let mut position = start + 1;
        while position < bytes.len() {
            if bytes[position].is_ascii_digit() {
                digits += 1;
                position += 1;
                end = position;
            } else if matches!(bytes[position], b' ' | b'-' | b'(' | b')')
                && position + 1 < bytes.len()
                && (bytes[position + 1].is_ascii_digit()
                    || matches!(bytes[position + 1], b'(' | b')'))
            {
                position += 1;
            } else {
                break;
            }
        }
        if (8..=15).contains(&digits) {
            findings.push(PiiFinding {
                category: PiiCategory::InternationalPhoneNumber,
                start,
                end,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<PiiCategory> {
        scan(text).into_iter().map(|f| f.category).collect()
    }

    #[test]
    fn an_email_address_is_found_and_its_offsets_cover_it() {
        let text = "write to jane.doe+casework@example.co.uk today";
        let findings = scan(text);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, PiiCategory::EmailAddress);
        assert_eq!(
            &text[findings[0].start..findings[0].end],
            "jane.doe+casework@example.co.uk"
        );
    }

    #[test]
    fn an_at_sign_without_a_domain_is_not_an_email() {
        assert!(kinds("mentions @handle and price @ 4").is_empty());
        assert!(kinds("odd@domain-without-dot").is_empty());
    }

    #[test]
    fn a_luhn_valid_card_number_is_found_in_grouped_and_plain_forms() {
        for text in ["pay 4111 1111 1111 1111 now", "pay 4111111111111111 now"] {
            let findings = scan(text);
            assert_eq!(findings.len(), 1, "{text}");
            assert_eq!(findings[0].category, PiiCategory::PaymentCardNumber);
        }
    }

    #[test]
    fn a_luhn_invalid_digit_run_is_not_a_card() {
        assert!(kinds("order 4111111111111112 shipped").is_empty());
        assert!(kinds("timestamp 31536000 and 1234567890123 reference").is_empty());
    }

    #[test]
    fn a_national_insurance_number_is_found_with_and_without_spaces() {
        for text in ["NI AB 12 34 56 C on file", "NI AB123456C on file"] {
            let findings = scan(text);
            assert_eq!(findings.len(), 1, "{text}");
            assert_eq!(findings[0].category, PiiCategory::NationalInsuranceNumber);
        }
    }

    #[test]
    fn an_excluded_prefix_or_suffix_is_not_a_national_insurance_number() {
        assert!(kinds("ref QQ123456C held").is_empty(), "excluded prefix");
        assert!(kinds("ref AB123456E held").is_empty(), "suffix beyond D");
        assert!(
            kinds("ref XAB123456C held").is_empty(),
            "embedded in a word"
        );
    }

    #[test]
    fn an_international_phone_number_is_found_and_short_runs_are_not() {
        let findings = scan("call +44 20 7946 0958 or +44 (0)20 7946 0958");
        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .all(|f| f.category == PiiCategory::InternationalPhoneNumber)
        );
        assert!(kinds("C++11 and +4 degrees").is_empty());
    }

    #[test]
    fn the_recorded_exa_text_carries_no_findings() {
        // The demo bytes must keep flowing: a false positive here would refuse
        // the recorded corpus every strict run depends on.
        let text = "Article 53: Obligations for providers of general-purpose AI models | AI \
                    Act Service Desk Skip to main content  # AI Act  […elided, 400 chars \
                    total; shape recorded, text not committed]";
        assert!(scan(text).is_empty());
    }

    #[test]
    fn strict_refuses_and_observe_carries_the_same_finding() {
        let text = "contact jane@example.com";
        let (_, strict) = invoke(
            PolicyMode::Strict,
            SourceClass::Internal,
            false,
            "https://rag.corp.internal/x",
            "https://rag.corp.internal/x",
            text,
            None,
        );
        assert!(strict.is_refusal());
        let (invocation, observe) = invoke(
            PolicyMode::Observe,
            SourceClass::Internal,
            false,
            "https://rag.corp.internal/x",
            "https://rag.corp.internal/x",
            text,
            None,
        );
        assert!(!observe.is_refusal());
        assert_eq!(
            strict.gap(),
            observe.gap(),
            "the mode decides what happens to a finding, never how it is recorded"
        );
        let record = invocation.to_value();
        assert_eq!(record["detail"]["matched_text_recorded"], false);
        assert!(
            !record.to_string().contains("jane@example.com"),
            "the identifier itself must never enter the evidence record"
        );
    }

    /// Where the text came from decides what strict does with a finding:
    /// a public source is carried with the finding recorded, an internal
    /// one is refused, and the record is the same either way.
    #[test]
    fn a_public_source_carries_a_pii_finding_in_strict_and_an_internal_one_is_refused() {
        let text = "Contact the Land Registry at customersupport@landregistry.gov.uk.";
        let public = "https://www.gov.uk/government/organisations/land-registry";
        let (public_invocation, public_ruling) = invoke(
            PolicyMode::Strict,
            SourceClass::Public,
            false,
            public,
            public,
            text,
            None,
        );
        assert!(
            matches!(public_ruling, Ruling::AllowedWithBreach { .. }),
            "strict carries a public source with the finding recorded"
        );
        assert_eq!(public_invocation.to_value()["decision"], "admit");
        assert_eq!(
            public_invocation.to_value()["detail"]["categories"]["email_address"],
            1
        );
        let internal = "https://rag.corp.internal/contacts";
        let (internal_invocation, internal_ruling) = invoke(
            PolicyMode::Strict,
            SourceClass::Internal,
            false,
            internal,
            internal,
            text,
            None,
        );
        assert!(internal_ruling.is_refusal());
        assert_eq!(internal_invocation.to_value()["decision"], "refuse");
        for ruling in [&public_ruling, &internal_ruling] {
            assert!(
                ruling
                    .reason()
                    .unwrap()
                    .contains("1 structured personal identifier (email_address ×1)"),
                "the finding reads the same whichever way it is ruled"
            );
        }
        assert_eq!(
            public_ruling.gap().map(|gap| &gap.reason),
            internal_ruling.gap().map(|gap| &gap.reason)
        );
        for record in [public_invocation.to_value(), internal_invocation.to_value()] {
            assert!(!record.to_string().contains("customersupport@"));
        }
    }

    /// The switch restores the refusal on every source; observe and prefer
    /// carry a finding on every source, switch or not.
    #[test]
    fn refuse_on_pii_refuses_a_public_source_in_strict() {
        let text = "contact jane@example.com";
        let public = "https://www.gov.uk/x";
        let (_, switched) = invoke(
            PolicyMode::Strict,
            SourceClass::Public,
            true,
            public,
            public,
            text,
            None,
        );
        assert!(switched.is_refusal());
        for mode in [PolicyMode::Observe, PolicyMode::Prefer] {
            for source in [SourceClass::Public, SourceClass::Internal] {
                let (_, ruling) = invoke(mode, source, true, public, public, text, None);
                assert!(
                    matches!(ruling, Ruling::AllowedWithBreach { .. }),
                    "{mode:?} carries every finding"
                );
            }
        }
    }
}
