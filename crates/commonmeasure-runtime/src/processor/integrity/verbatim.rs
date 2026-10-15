//! Two answers to "supported?" from the words alone.
//!
//! The fidelity verifier's six-word run is recorded as evidence: on edited
//! summaries the run rule passes many a sentence with one changed entity, so
//! it admits nothing. The whole sentence verbatim is the display rule's own
//! test: every word of the sentence occurs in the release text,
//! consecutively and in order.

use commonmeasure_inference::ContextPart;
use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};

use super::super::fidelity;
use super::words::{find_run, words};
use super::{Answer, Case, Label, Question};

pub const WHOLE_NAME: &str = "verbatim-whole/1";

/// The fidelity verifier's basis as an answer records it.
pub fn six_word_name() -> String {
    format!("{}/{}", fidelity::NAME, fidelity::VERSION)
}

/// The fidelity verifier over the sentence and the case's text, with no
/// citations: the sentence is judged against the text alone.
pub fn six_word_run(case: &Case) -> Answer {
    let part = ContextPart {
        source_ref: case.id.clone(),
        content_hash: sha256_digest(case.passage.as_bytes()),
        text: case.passage.clone(),
    };
    let (invocation, claims) =
        fidelity::invoke(&case.summary, &json!({ "citations": [] }), &[part]);
    let record = invocation.to_value();
    let supported = !claims.is_empty()
        && claims
            .iter()
            .all(|claim| claim.verdict == fidelity::ClaimVerdict::Supported);
    let (label, probability, reason) = if claims.is_empty() {
        (
            Label::NotApplicable,
            None,
            "the sentence is too short to be a claim",
        )
    } else if supported {
        (
            Label::Yes,
            Some(1.0),
            "a run of at least six consecutive words of the sentence occurs in the release text",
        )
    } else {
        (
            Label::No,
            Some(0.0),
            "no run of six consecutive words of the sentence occurs in the release text; a \
             paraphrase is invisible to this stage",
        )
    };
    Answer {
        question: Question::Supported,
        label,
        probability,
        basis: six_word_name(),
        reason: reason.to_owned(),
        detail: json!({
            "configuration_digest": record["processor"]["configuration_digest"],
            "claims": claim_refs(&record["detail"]["claims"]),
        }),
    }
}

/// The fidelity verifier's claims as the integrity record keeps them: spans,
/// counts, verdicts and evidence spans and hashes. The verifier's own record
/// carries each claim's text and its reasons can quote; neither is kept.
fn claim_refs(claims: &Value) -> Value {
    claims
        .as_array()
        .into_iter()
        .flatten()
        .map(|claim| {
            json!({
                "index": claim["index"],
                "start": claim["start"],
                "end": claim["end"],
                "words": claim["words"],
                "verdict": claim["verdict"],
                "basis": claim["basis"],
                "evidence": claim["evidence"],
            })
        })
        .collect()
}

/// Every word of the sentence occurs in the case's text consecutively and in
/// order.
pub fn whole(case: &Case) -> Answer {
    let sentence = words(&case.summary);
    let found = !sentence.is_empty() && find_run(&sentence, &words(&case.passage)).is_some();
    Answer {
        question: Question::Supported,
        label: if found { Label::Yes } else { Label::No },
        probability: Some(if found { 1.0 } else { 0.0 }),
        basis: WHOLE_NAME.to_owned(),
        reason: if found {
            "the whole sentence occurs verbatim in the release text"
        } else {
            "the sentence does not occur whole in the release text"
        }
        .to_owned(),
        detail: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(passage: &str, summary: &str) -> Case {
        Case {
            id: "t".into(),
            passage: passage.into(),
            summary: summary.into(),
            markers: Vec::new(),
        }
    }

    const PASSAGE: &str =
        "The council approved the plan on Tuesday after a long debate about parking.";

    #[test]
    fn a_verbatim_run_is_supported() {
        let answer = six_word_run(&case(PASSAGE, "The council approved the plan on Tuesday."));
        assert_eq!(answer.label, Label::Yes);
        assert_eq!(answer.basis, "fidelity-verifier/1");
    }

    #[test]
    fn a_paraphrase_is_not_supported_here() {
        let answer = six_word_run(&case(
            PASSAGE,
            "Councillors agreed to the proposal this week.",
        ));
        assert_eq!(answer.label, Label::No);
    }

    #[test]
    fn the_whole_sentence_must_occur_for_the_whole_test() {
        assert_eq!(
            whole(&case(PASSAGE, "The council approved the plan on Tuesday.")).label,
            Label::Yes
        );
        assert_eq!(
            whole(&case(PASSAGE, "The council approved the plan on Monday.")).label,
            Label::No
        );
    }
}
