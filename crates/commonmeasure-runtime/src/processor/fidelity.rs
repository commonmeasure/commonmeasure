//! The fidelity verifier: a deterministic `verify`-stage processor that
//! attaches per-claim support evidence to an answer.
//!
//! The grounding evaluator judges what the answer cites. This processor
//! judges what the answer says: it segments the answer into claims and, for
//! each claim, records whether a verbatim span of the window's retained text
//! backs it, whether it reproduces a quote its own citation misattributes,
//! or whether nothing verbatim backs it at all. Every verdict is a pure
//! function of the answer, the grounding record and the window text; no
//! clock, no sampling, no model. A paraphrase of the window is invisible
//! here, which the record states, and the optional model judge
//! (`super::judge`) is measured against these verdicts rather than replacing
//! them.
//!
//! No universal truth score is produced. `supported` means a word sequence of
//! the claim entered the window, not that the claim is true.

use commonmeasure_types::canonical::sha256_digest;
use std::sync::LazyLock;

use chrono::Utc;
use commonmeasure_inference::ContextPart;
use commonmeasure_types::Decision;
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use crate::evaluate::{find_quote, names_a_citation, same_word, words_with_offsets};

pub const NAME: &str = "fidelity-verifier";
pub const VERSION: &str = "1";

/// A segment with fewer words than this is not a claim: a heading, a label
/// or a fragment, which carries nothing checkable.
const MINIMUM_CLAIM_WORDS: usize = 3;

/// A claim is backed by the window only where at least this many consecutive
/// words of it occur in one part's retained text. Shorter runs are ordinary
/// English shared by any two texts on one subject.
const MINIMUM_WINDOW_RUN_WORDS: usize = 6;

/// The rule set, stated canonically; the configuration digest covers it. A
/// commit that changes what the verifier decides changes this text and
/// `VERSION` together.
const RULES: &str = "fidelity-verifier/1: claims are the sentences of the answer's lines that \
     do not name a CITATION, each line first stripped of list, numbering, blockquote and \
     emphasis decoration and then split where `.`, `!` or `?` is followed by whitespace \
     or the end of the line, closing quotes and brackets kept with the sentence; a \
     segment of fewer than 3 whitespace-delimited words is not a claim. Verdicts, in \
     priority order, with words compared as the grounding evaluator compares them \
     (case-insensitive, trailing punctuation ignored, whitespace flexible): \
     contradicted = the claim contains the quote of a citation the grounding evaluator \
     judged contradicted; supported = the claim contains the quote of a citation judged \
     supported, recorded against that citation's span, or else a run of at least 6 \
     consecutive words of the claim occurs in one window part's retained text, the \
     longest such run recorded with the part's content hash and byte span; \
     unsupported = neither. Claim byte offsets are into the answer as recorded. The \
     supported fraction is supported claims over all claims and is null where there \
     are no claims.";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Verify,
    capability: "claim-support",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: NO_AMBIENT_AUTHORITY,
    determinism: Determinism::Deterministic,
    limits: "in-process and synchronous; one pass over the answer against each window part, \
             no supervisor timeout",
    failure_by_mode: FailureByMode {
        // A verifier that cannot run leaves the answer as it was, with a gap:
        // an unverified answer is still the answer the model gave.
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
    "Supported means a word sequence of the claim entered the window, not that the claim is \
     true, and not that the window as a whole agrees with it.",
    "A paraphrase of the window is invisible: a claim that restates a source in other words \
     is unsupported here, and the model judge's disagreement on such claims is expected.",
    "Sentence segmentation is a punctuation rule: an abbreviation splits a sentence, and a \
     claim spread over several sentences is judged one sentence at a time.",
    "Contradicted carries the grounding evaluator's meaning: the claim reproduces a quote its \
     citation attributes to a source that does not contain it. No claim is judged false.",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimVerdict {
    Supported,
    Contradicted,
    Unsupported,
}

impl ClaimVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Contradicted => "contradicted",
            Self::Unsupported => "unsupported",
        }
    }
}

/// One claim of the answer with its verdict, as the judge receives it.
#[derive(Debug, Clone)]
pub struct Claim {
    /// Byte offsets into the answer as recorded.
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub verdict: ClaimVerdict,
}

/// Where a supported claim's words were found.
#[derive(Debug, Clone, Serialize)]
struct Evidence {
    /// The window part: its URL and the hash of its retained text.
    reference: String,
    content_hash: String,
    /// Byte span in the part's retained text.
    start: usize,
    end: usize,
    /// Byte span in the answer of the words that matched.
    claim_start: usize,
    claim_end: usize,
    words: usize,
}

/// Verify one answer. Returns the invocation record and the claims, so the
/// judge can be handed exactly what was verified.
pub fn invoke(answer: &str, grounding: &Value, parts: &[ContextPart]) -> (Invocation, Vec<Claim>) {
    let started_at = Utc::now();
    let segments = segment(answer);
    let citations: Vec<&Value> = grounding["citations"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let part_words: Vec<Vec<(&str, usize, usize)>> = parts
        .iter()
        .map(|part| words_with_offsets(&part.text))
        .collect();

    let mut records = Vec::new();
    let mut claims = Vec::new();
    for (index, (start, end)) in segments.iter().enumerate() {
        let text = &answer[*start..*end];
        let (verdict, basis, citation, evidence, reason) =
            judge_claim(text, *start, &citations, parts, &part_words);
        records.push(json!({
            "index": index,
            "start": start,
            "end": end,
            "text": text,
            "words": text.split_whitespace().count(),
            "verdict": verdict,
            "basis": basis,
            "citation": citation,
            "evidence": evidence,
            "reason": reason,
        }));
        claims.push(Claim {
            start: *start,
            end: *end,
            text: text.to_owned(),
            verdict,
        });
    }

    let count = |verdict: ClaimVerdict| claims.iter().filter(|c| c.verdict == verdict).count();
    let supported = count(ClaimVerdict::Supported);
    let fraction = if claims.is_empty() {
        Value::Null
    } else {
        json!(supported as f64 / claims.len() as f64)
    };

    let mut inputs = vec![ArtefactRef {
        reference: "answer".to_owned(),
        content_hash: Some(sha256_digest(answer.as_bytes())),
        tokens: None,
    }];
    inputs.extend(parts.iter().map(|part| ArtefactRef {
        reference: part.source_ref.clone(),
        content_hash: Some(part.content_hash.clone()),
        tokens: None,
    }));

    let invocation = Invocation::new(
        manifest(),
        started_at,
        // The verify stage decides nothing about admission: the record is the
        // output, and it stands beside the answer.
        Decision::Abstain,
        "deterministic sentence segmentation of the answer, then per-claim word-sequence \
         matching against the supported and contradicted citations and the window parts' \
         retained text",
        inputs,
        Vec::new(),
        json!({
            "claim_count": claims.len(),
            "claim_counts": {
                "supported": supported,
                "contradicted": count(ClaimVerdict::Contradicted),
                "unsupported": count(ClaimVerdict::Unsupported),
            },
            "supported_fraction": fraction,
            "minimum_claim_words": MINIMUM_CLAIM_WORDS,
            "minimum_window_run_words": MINIMUM_WINDOW_RUN_WORDS,
            "claims": records,
        }),
        Vec::new(),
        BLIND_SPOTS.to_vec(),
    );
    (invocation, claims)
}

/// The verdict for one claim, with its basis (`citation` or `window`), the
/// citation index where a citation decided it, the evidence span and the
/// reason.
fn judge_claim(
    text: &str,
    offset: usize,
    citations: &[&Value],
    parts: &[ContextPart],
    part_words: &[Vec<(&str, usize, usize)>],
) -> (ClaimVerdict, Value, Value, Value, String) {
    let quoting = |verdict: &str| {
        citations
            .iter()
            .enumerate()
            .filter(|(_, citation)| citation["verdict"] == verdict)
            .find_map(|(index, citation)| {
                let quote = citation["quote"].as_str()?;
                find_quote(quote, text).map(|span| (index, *citation, quote, span))
            })
    };

    if let Some((index, citation, quote, _)) = quoting("contradicted") {
        return (
            ClaimVerdict::Contradicted,
            json!("citation"),
            json!(index),
            Value::Null,
            format!(
                "the claim contains \"{quote}\", which citation #{} attributes to {} and the \
                 grounding evaluator found absent from that source's retained text",
                index + 1,
                citation["url"].as_str().unwrap_or_default()
            ),
        );
    }
    if let Some((index, citation, quote, (claim_start, claim_end))) = quoting("supported") {
        let matched = &citation["matched"];
        let evidence = Evidence {
            reference: citation["url"].as_str().unwrap_or_default().to_owned(),
            content_hash: matched["content_hash"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            start: matched["start"].as_u64().unwrap_or_default() as usize,
            end: matched["end"].as_u64().unwrap_or_default() as usize,
            claim_start: offset + claim_start,
            claim_end: offset + claim_end,
            words: quote.split_whitespace().count(),
        };
        return (
            ClaimVerdict::Supported,
            json!("citation"),
            json!(index),
            serde_json::to_value(&evidence).unwrap_or(Value::Null),
            format!(
                "the claim contains the quote of citation #{}, which the grounding evaluator \
                 found in the retained text of {} at the recorded span",
                index + 1,
                evidence.reference
            ),
        );
    }

    let claim_words = words_with_offsets(text);
    let mut best: Option<(usize, usize, usize, usize)> = None;
    for (part_index, words) in part_words.iter().enumerate() {
        if let Some((claim_at, part_at, length)) = longest_shared_run(&claim_words, words)
            && best.is_none_or(|(_, _, _, held)| length > held)
        {
            best = Some((part_index, claim_at, part_at, length));
        }
    }
    match best {
        Some((part_index, claim_at, part_at, length)) if length >= MINIMUM_WINDOW_RUN_WORDS => {
            let part = &parts[part_index];
            let words = &part_words[part_index];
            let evidence = Evidence {
                reference: part.source_ref.clone(),
                content_hash: part.content_hash.clone(),
                start: words[part_at].1,
                end: words[part_at + length - 1].2,
                claim_start: offset + claim_words[claim_at].1,
                claim_end: offset + claim_words[claim_at + length - 1].2,
                words: length,
            };
            let reason = format!(
                "{length} consecutive words of the claim occur in the retained text of {} at \
                 the recorded span",
                evidence.reference
            );
            (
                ClaimVerdict::Supported,
                json!("window"),
                Value::Null,
                serde_json::to_value(&evidence).unwrap_or(Value::Null),
                reason,
            )
        }
        _ => (
            ClaimVerdict::Unsupported,
            Value::Null,
            Value::Null,
            Value::Null,
            format!(
                "no supported citation's quote and no run of {MINIMUM_WINDOW_RUN_WORDS} or more \
                 consecutive words of the claim occurs in the window's retained text; a \
                 paraphrase of the window is invisible to this check"
            ),
        ),
    }
}

/// The longest run of consecutive `claim` words occurring consecutively in
/// `part`: (claim index, part index, length). Ties resolve to the earliest
/// claim position, then the earliest part position.
fn longest_shared_run(
    claim: &[(&str, usize, usize)],
    part: &[(&str, usize, usize)],
) -> Option<(usize, usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;
    for claim_at in 0..claim.len() {
        for part_at in 0..part.len() {
            if !same_word(claim[claim_at].0, part[part_at].0) {
                continue;
            }
            let mut length = 1;
            while claim_at + length < claim.len()
                && part_at + length < part.len()
                && same_word(claim[claim_at + length].0, part[part_at + length].0)
            {
                length += 1;
            }
            if best.is_none_or(|(_, _, held)| length > held) {
                best = Some((claim_at, part_at, length));
            }
        }
    }
    best
}

/// The claims of an answer as byte spans: every sentence of every
/// non-citation line, decoration stripped, of at least
/// `MINIMUM_CLAIM_WORDS` words.
fn segment(answer: &str) -> Vec<(usize, usize)> {
    let mut claims = Vec::new();
    let mut line_start = 0;
    for line in answer.split_inclusive('\n') {
        let start = line_start;
        line_start += line.len();
        let line = line.trim_end_matches(['\n', '\r']);
        if names_a_citation(line) {
            continue;
        }
        let content_start = start + decoration_length(line);
        let content = &answer[content_start..start + line.len()];
        for (sentence_start, sentence_end) in sentences(content) {
            let sentence = &content[sentence_start..sentence_end];
            if sentence.split_whitespace().count() < MINIMUM_CLAIM_WORDS {
                continue;
            }
            claims.push((content_start + sentence_start, content_start + sentence_end));
        }
    }
    claims
}

/// How many bytes of list, numbering, blockquote and emphasis decoration
/// open `line`: the same set the grounding evaluator strips before a
/// `CITATION:` marker.
fn decoration_length(line: &str) -> usize {
    let mut rest = line;
    loop {
        let stripped = rest
            .trim_start_matches(['-', '*', '+', '>', '#', '`', '_', ' ', '\t'])
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')', ' ', '\t']);
        if stripped.len() == rest.len() {
            return line.len() - rest.len();
        }
        rest = stripped;
    }
}

/// Sentence spans of one line of text, trimmed, split where a terminator is
/// followed by whitespace or the end. Closing quotes and brackets after the
/// terminator stay with the sentence.
fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < text.len() {
        let byte = bytes[index];
        if matches!(byte, b'.' | b'!' | b'?') {
            let mut end = index + 1;
            while end < text.len() && matches!(bytes[end], b'"' | b'\'' | b')' | b']' | b'}') {
                end += 1;
            }
            // Multi-byte closing quotes (”, ’) are kept with the sentence too.
            for closing in ["\u{201d}", "\u{2019}", "\u{bb}"] {
                if text[end..].starts_with(closing) {
                    end += closing.len();
                }
            }
            let terminal = end >= text.len() || bytes[end].is_ascii_whitespace();
            if terminal {
                push_trimmed(text, start, end, &mut spans);
                start = end;
                index = end;
                continue;
            }
        }
        index += 1;
    }
    push_trimmed(text, start, text.len(), &mut spans);
    spans
}

fn push_trimmed(text: &str, start: usize, end: usize, spans: &mut Vec<(usize, usize)>) {
    let segment = &text[start..end];
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return;
    }
    let leading = segment.len() - segment.trim_start().len();
    let trailing = segment.len() - segment.trim_end().len();
    spans.push((start + leading, end - trailing));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::{EvaluationInput, grounding};

    fn part(url: &str, text: &str) -> ContextPart {
        ContextPart {
            source_ref: url.to_owned(),
            content_hash: sha256_digest(text.as_bytes()),
            text: text.to_owned(),
        }
    }

    fn verify(answer: &str, parts: &[ContextPart]) -> (Value, Vec<Claim>) {
        let record = grounding(&EvaluationInput {
            answer: Some(answer),
            cited_answer_required: true,
            parts,
        });
        let (invocation, claims) = invoke(answer, &record, parts);
        (invocation.to_value(), claims)
    }

    const SOURCE: &str = "Purpose: The cap is set twice a year by the regulator. It applies per \
                          unit of energy, not to the whole bill, and it is reviewed quarterly \
                          from 2024.";

    #[test]
    fn a_claim_reproducing_a_mid_text_run_is_supported_at_its_span() {
        let parts = [part("https://example.org/cap", SOURCE)];
        let answer = "The regulator says the cap applies per unit of energy, not to the whole \
                      bill.\nNothing here is about tariffs.";
        let (record, claims) = verify(answer, &parts);
        let detail = &record["detail"];
        assert_eq!(detail["claim_count"], 2);
        let first = &detail["claims"][0];
        assert_eq!(first["verdict"], "supported");
        assert_eq!(first["basis"], "window");
        let evidence = &first["evidence"];
        let (start, end) = (
            evidence["start"].as_u64().unwrap() as usize,
            evidence["end"].as_u64().unwrap() as usize,
        );
        assert!(
            start > 0,
            "the matched span must sit mid-text, not at the opening words"
        );
        assert_eq!(
            SOURCE[start..end].split_whitespace().collect::<Vec<_>>(),
            [
                "applies", "per", "unit", "of", "energy,", "not", "to", "the", "whole", "bill,"
            ]
        );
        let (claim_start, claim_end) = (
            evidence["claim_start"].as_u64().unwrap() as usize,
            evidence["claim_end"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            &answer[claim_start..claim_end],
            "applies per unit of energy, not to the whole bill."
        );
        assert_eq!(detail["claims"][1]["verdict"], "unsupported");
        assert_eq!(claims[1].verdict, ClaimVerdict::Unsupported);
        assert_eq!(detail["supported_fraction"], 0.5);
        assert_eq!(record["assurance"], "observed");
    }

    #[test]
    fn a_supported_citation_quote_inside_a_claim_backs_it_whatever_its_length() {
        let parts = [part("https://example.org/cap", SOURCE)];
        let answer = "It is reviewed quarterly.\nCITATION: https://example.org/cap :: \"reviewed \
                      quarterly\"";
        let (record, _) = verify(answer, &parts);
        let claim = &record["detail"]["claims"][0];
        assert_eq!(claim["verdict"], "supported");
        assert_eq!(claim["basis"], "citation");
        assert_eq!(claim["citation"], 0);
        assert_eq!(claim["evidence"]["words"], 2);
        assert_eq!(
            record["detail"]["claim_count"], 1,
            "the CITATION line is not a claim"
        );
    }

    #[test]
    fn a_claim_quoting_a_contradicted_citation_is_contradicted() {
        let parts = [part("https://example.org/cap", SOURCE)];
        let answer = "The cap is set once a decade by the regulator.\nCITATION: \
                      https://example.org/cap :: \"set once a decade\"";
        let (record, _) = verify(answer, &parts);
        let claim = &record["detail"]["claims"][0];
        assert_eq!(claim["verdict"], "contradicted");
        assert_eq!(claim["citation"], 0);
        assert_eq!(claim["evidence"], Value::Null);
    }

    #[test]
    fn short_runs_and_short_segments_do_not_count() {
        let parts = [part("https://example.org/cap", SOURCE)];
        // Five shared words: one short of the window rule.
        let answer = "Summary:\nThe cap is set twice yearly by ministers.";
        let (record, _) = verify(answer, &parts);
        let detail = &record["detail"];
        assert_eq!(
            detail["claim_count"], 1,
            "a one-word heading is not a claim"
        );
        assert_eq!(detail["claims"][0]["verdict"], "unsupported");
    }

    #[test]
    fn an_answer_with_no_claims_publishes_no_fraction() {
        let parts = [part("https://example.org/cap", SOURCE)];
        let (record, claims) = verify("CITATION: https://example.org/cap :: \"per unit\"", &parts);
        assert!(claims.is_empty());
        assert_eq!(record["detail"]["supported_fraction"], Value::Null);
    }

    #[test]
    fn segmentation_strips_decoration_and_keeps_offsets_into_the_answer() {
        let answer = "1. First point stands here. Second point follows!\n- **Third point** is \
                      quoted.\"";
        let spans = segment(answer);
        let texts: Vec<&str> = spans.iter().map(|(s, e)| &answer[*s..*e]).collect();
        assert_eq!(
            texts,
            [
                "First point stands here.",
                "Second point follows!",
                "Third point** is quoted.\""
            ]
        );
    }

    #[test]
    fn the_rule_text_and_the_digest_move_with_the_version() {
        assert!(RULES.starts_with(&format!("{NAME}/{VERSION}:")));
        assert_eq!(
            manifest().configuration_digest,
            "sha256:c19f1370d25fb29ce9b92616aef99b3cc5c46c4fd67691b6af18034af3658fa4"
        );
    }
}
