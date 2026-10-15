//! "Cuts a kept passage?" as plain code. A source marks a span to be shown
//! whole with its context or not at all. A sentence cuts it where it
//! reproduces a run of the kept span's words without reproducing the whole
//! span. A sentence that reproduces the whole span, or none of it, does not.
//!
//! A paraphrase of a kept passage is invisible here, as it is to the
//! fidelity verifier.

use serde_json::{Value, json};

use super::words::{find_run, longest_common_run, span, words};
use super::{Answer, Case, Label, Question};

pub const NAME: &str = "keep-intact-containment/1";

/// The run length that counts as reproducing a kept passage: the fidelity
/// verifier's own threshold, so the two stages agree on what "verbatim"
/// means.
pub const MINIMUM_RUN_WORDS: usize = 6;

/// The answer, and the index into the case's markers of the first span the
/// sentence cuts.
pub fn check(case: &Case) -> (Answer, Option<usize>) {
    if case.markers.is_empty() {
        return (
            Answer {
                question: Question::CutsKeptPassage,
                label: Label::NotApplicable,
                probability: None,
                basis: NAME.to_owned(),
                reason: "the release carries no keep-intact marker".to_owned(),
                detail: Value::Null,
            },
            None,
        );
    }
    let sentence = words(&case.summary);
    let mut findings = Vec::new();
    let mut cut = None;
    for (index, marker) in case.markers.iter().enumerate() {
        // A verified release's markers lie in its text; a span that does not
        // is recorded and judged as nothing cut, never sliced.
        let Some(kept_text) = case.passage.get(marker.start..marker.end) else {
            findings
                .push(json!({ "marker": index, "label": marker.label, "outcome": "outside_text" }));
            continue;
        };
        let kept = words(kept_text);
        let whole = find_run(&kept, &sentence).is_some();
        let (run, _, kept_at) = longest_common_run(&sentence, &kept);
        let outcome = if whole {
            "whole"
        } else if run >= MINIMUM_RUN_WORDS {
            "cut"
        } else {
            "absent"
        };
        if outcome == "cut" && cut.is_none() {
            cut = Some(index);
        }
        let run_span = if run > 0 {
            let (s, e) = span(&kept, kept_at, kept_at + run);
            json!({ "start": marker.start + s, "end": marker.start + e })
        } else {
            Value::Null
        };
        findings.push(json!({
            "marker": index,
            "label": marker.label,
            "outcome": outcome,
            "longest_run_words": run,
            "run_span": run_span,
        }));
    }
    let (label, probability, reason) = match cut {
        Some(index) => (
            Label::Yes,
            Some(1.0),
            format!(
                "the sentence reproduces part of kept span {index} ({}) without the whole of it",
                case.markers[index].label
            ),
        ),
        None => (
            Label::No,
            Some(0.0),
            "the sentence reproduces either the whole of each kept span it touches or none of it"
                .to_owned(),
        ),
    };
    (
        Answer {
            question: Question::CutsKeptPassage,
            label,
            probability,
            basis: NAME.to_owned(),
            reason,
            detail: json!({ "markers": findings }),
        },
        cut,
    )
}

#[cfg(test)]
mod tests {
    use super::super::Marker;
    use super::*;

    const PASSAGE: &str = "Before the vote the mayor read a statement. \
        \"We accept the inspector's findings in full and we will publish the costs of the \
        scheme before the end of the year, whatever they turn out to be,\" she said. \
        The vote followed.";

    fn case(summary: &str) -> Case {
        let start = PASSAGE.find('"').unwrap();
        let end = PASSAGE.rfind('"').unwrap() + 1;
        Case {
            id: "t".into(),
            passage: PASSAGE.into(),
            summary: summary.into(),
            markers: vec![Marker {
                start,
                end,
                label: "statement".into(),
            }],
        }
    }

    #[test]
    fn a_fragment_of_the_statement_is_a_cut() {
        let (answer, cut) = check(&case(
            "The mayor said \"we accept the inspector's findings in full\".",
        ));
        assert_eq!(answer.label, Label::Yes);
        assert_eq!(cut, Some(0));
    }

    #[test]
    fn the_whole_statement_is_not_a_cut() {
        let (answer, cut) = check(&case(
            "The mayor said: \"We accept the inspector's findings in full and we will \
             publish the costs of the scheme before the end of the year, whatever they \
             turn out to be.\"",
        ));
        assert_eq!(answer.label, Label::No);
        assert_eq!(cut, None);
    }

    #[test]
    fn a_sentence_about_the_vote_is_not_a_cut() {
        let (answer, _) = check(&case("The vote followed the statement."));
        assert_eq!(answer.label, Label::No);
    }

    #[test]
    fn no_marker_means_not_applicable() {
        let mut c = case("Anything.");
        c.markers.clear();
        assert_eq!(check(&c).0.label, Label::NotApplicable);
    }
}
