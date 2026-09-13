//! The coverage evaluator: which of a suite's predeclared rubric items the
//! admitted window covered.
//!
//! Coverage here is a measurement, not a judgment: the suite declares named
//! items, each with one or more phrases, and the evaluator reports which
//! items' phrases occur in the retained text that actually entered the
//! plan's window — the same word-sequence matching and byte-span discipline
//! as the grounding evaluator, so every `covered` verdict names the part and
//! span a reader can excise. The fraction it publishes is covered items over
//! declared items: agreement with the rubric's author, never goodness, which
//! is why every record names the rubric identity it measured against.
//!
//! Everything here is a pure function of the rubric and the window content:
//! no clock, no sampling, no configuration beyond the versioned rule text
//! whose digest the record carries. Two replay runs of one suite produce
//! byte-identical coverage records. A suite that declares no rubric, or a
//! plan that assembled no window, is unmeasured with the reason stated —
//! never zero (`docs/FAIL-POLICY.md` §7).

use commonmeasure_inference::ContextPart;
use commonmeasure_types::canonical::sha256_digest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evaluate::{self, EVALUATION_VERSION};

pub const EVALUATOR_NAME: &str = "coverage";
pub const EVALUATOR_VERSION: &str = "0.1.0";

/// The canonical rule text behind the configuration digest, written by hand
/// under the same convention as the grounding evaluator's: a commit that
/// changes what this evaluator decides changes this text and
/// [`EVALUATOR_VERSION`] together, and the tests below pin the digest so a
/// behaviour change that leaves either behind fails rather than publishes.
///
/// The rubric itself is deliberately not part of this digest: it is suite
/// input, sealed by the run manifest, so a rubric change is a different
/// experiment by manifest hash while the evaluator identity stands still.
const RULES: &str = "coverage/0.1.0: a suite may declare coverage_rubric — named items, each \
     carrying one or more phrases. An item is covered when any of its phrases' word \
     sequences occurs in the retained text of a part admitted to this plan's window, \
     matching words case-insensitively, ignoring trailing punctuation, with whitespace \
     flexible; the record names the phrase, the part and the byte span it was found at. \
     The published fraction is covered items over declared items, and it measures \
     agreement with the rubric's author, never goodness. A suite with no rubric, or a \
     plan for which no admitted window was assembled, is unmeasured with the reason \
     stated, never zero.";

pub fn configuration_digest() -> String {
    sha256_digest(RULES.as_bytes())
}

pub(crate) fn identity() -> Value {
    json!({
        "name": EVALUATOR_NAME,
        "version": EVALUATOR_VERSION,
        "configuration_digest": configuration_digest(),
    })
}

/// The evaluator's own limits, carried on every record so a reader weighs
/// the fraction correctly.
const BLIND_SPOTS: &[&str] = &[
    "Coverage measures agreement with the rubric's author: covered means a declared \
     phrase's words entered the window, not that the rubric is complete or right, and \
     not that any answer used them.",
    "Phrase matching is verbatim word-sequence presence; a paraphrase of a rubric item \
     is invisible to this check.",
    "Items count equally in the fraction; the rubric declares no weights.",
];

/// A suite's predeclared coverage rubric: what a complete window must draw
/// on, named item by item. Versioned as part of the suite file and sealed by
/// the run manifest, so a rubric change is a different experiment by hash.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageRubric {
    pub name: String,
    pub version: String,
    pub items: Vec<RubricItem>,
}

/// One declared item. The item is covered when any one of its phrases occurs
/// in an admitted part's retained text.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RubricItem {
    pub name: String,
    pub any_of: Vec<String>,
}

impl CoverageRubric {
    /// Reject a rubric that could not be measured against before anything
    /// runs. An empty item list would make every fraction 0/0, a nameless or
    /// phraseless item could never be reported, and two items under one name
    /// would publish two verdicts a reader cannot tell apart.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(
                "a coverage rubric needs a name and a version; the record must \
                        say whose definition of coverage it measured against"
                    .to_owned(),
            );
        }
        if self.items.is_empty() {
            return Err(format!(
                "coverage rubric {}/{} declares no items, so there would be nothing to \
                 measure",
                self.name, self.version
            ));
        }
        let mut names: Vec<&str> = Vec::new();
        for item in &self.items {
            if item.name.trim().is_empty() {
                return Err(format!(
                    "coverage rubric {}/{} has an unnamed item; every verdict must name \
                     the item it is about",
                    self.name, self.version
                ));
            }
            if names.contains(&item.name.as_str()) {
                return Err(format!(
                    "coverage rubric {}/{} declares item {} twice; two verdicts under one \
                     name cannot be told apart",
                    self.name, self.version, item.name
                ));
            }
            names.push(&item.name);
            if item.any_of.is_empty()
                || item
                    .any_of
                    .iter()
                    .any(|phrase| phrase.split_whitespace().next().is_none())
            {
                return Err(format!(
                    "coverage rubric {}/{} item {} declares no matchable phrase; an item \
                     nothing could ever cover is not a measurement",
                    self.name, self.version, item.name
                ));
            }
        }
        Ok(())
    }

    /// The identity a record and a selection explanation name: whose
    /// definition of coverage the fraction was measured against.
    pub fn identity(&self) -> String {
        format!("{}/{}", self.name, self.version)
    }
}

/// What the runtime hands the evaluator: the suite's rubric, if declared, and
/// the parts that entered the window. `window: None` means no admitted window
/// was assembled for this plan (the acquisition never completed); an
/// assembled window that admitted nothing is `Some(&[])`, and covering zero
/// declared items with it is a measurement, not an absence.
pub struct CoverageInput<'a> {
    pub rubric: Option<&'a CoverageRubric>,
    pub window: Option<&'a [ContextPart]>,
}

/// Evaluate one plan. Always returns a record — an unmeasured plan carries
/// the evaluator's identity and the stated reason, never a bare null, so a
/// reader can tell "not measured, and why" from "nothing measured it".
pub fn coverage(input: &CoverageInput) -> Value {
    // Ordered by how fundamental the obstacle is: whether this plan
    // assembled a window at all is a lesser question than what the suite
    // declared, but it is the fact about *this plan*, so it speaks first —
    // the same order the grounding evaluator gives an absent answer.
    let unmeasured = match (input.window, input.rubric) {
        (None, _) => Some(
            "no admitted window was assembled for this plan, so there is nothing to \
             measure the rubric against",
        ),
        (Some(_), None) => Some(
            "the suite declares no coverage rubric, and coverage is measured only \
             against predeclared items (set coverage_rubric)",
        ),
        (Some(_), Some(_)) => None,
    };

    let mut inputs = Vec::new();
    for part in input.window.into_iter().flatten() {
        inputs.push(json!({
            "reference": part.source_ref,
            "content_hash": part.content_hash,
        }));
    }

    let items: Vec<Value> = match (unmeasured, input.window, input.rubric) {
        (None, Some(window), Some(rubric)) => rubric
            .items
            .iter()
            .map(|item| judge(item, window))
            .collect(),
        _ => Vec::new(),
    };
    let covered = items
        .iter()
        .filter(|item| item["covered"] == Value::Bool(true))
        .count();
    let (covered_count, item_count, fraction) = match unmeasured {
        // An unmeasured record carries no counts at all: a zero here would be
        // exactly the fabricated measurement FAIL-POLICY §7 forbids.
        Some(_) => (Value::Null, Value::Null, Value::Null),
        None => (
            json!(covered),
            json!(items.len()),
            json!(covered as f64 / items.len() as f64),
        ),
    };

    json!({
        "record_version": EVALUATION_VERSION,
        "evaluator": identity(),
        "method": "deterministic matching of predeclared rubric phrases over the retained \
                   text of the admitted window parts (word sequences, case-insensitive, \
                   flexible in whitespace and trailing punctuation, otherwise exact)",
        // The basis of the fraction, named on the record itself: the figure
        // travels with whose definition of coverage produced it.
        "rubric": input.rubric.map(|rubric| json!({
            "name": rubric.name,
            "version": rubric.version,
        })),
        "inputs": inputs,
        "items": items,
        "covered_count": covered_count,
        "item_count": item_count,
        "fraction": fraction,
        "unmeasured": unmeasured,
        // Every verdict rests on window text this runtime holds and items the
        // suite declared, never on a supplier's assurance about itself.
        "assurance": "observed",
        "blind_spots": BLIND_SPOTS,
    })
}

/// Judge one rubric item against the window: the first phrase found in any
/// part decides, and the record carries the span so a reader can excise it.
fn judge(item: &RubricItem, window: &[ContextPart]) -> Value {
    for phrase in &item.any_of {
        for part in window {
            if let Some((start, end)) = evaluate::find_quote(phrase, &part.text) {
                return json!({
                    "item": item.name,
                    "covered": true,
                    "matched": {
                        "phrase": phrase,
                        "reference": part.source_ref,
                        "content_hash": part.content_hash,
                        "start": start,
                        "end": end,
                    },
                    "reason": "a declared phrase's word sequence occurs in this part's \
                               retained text at the recorded span",
                });
            }
        }
    }
    json!({
        "item": item.name,
        "covered": false,
        "matched": Value::Null,
        "reason": "no declared phrase for this item occurs in the retained text of any \
                   admitted window part",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(url: &str, text: &str) -> ContextPart {
        ContextPart {
            source_ref: url.to_owned(),
            content_hash: sha256_digest(text.as_bytes()),
            text: text.to_owned(),
        }
    }

    fn rubric(items: &[(&str, &[&str])]) -> CoverageRubric {
        CoverageRubric {
            name: "test-rubric".into(),
            version: "1".into(),
            items: items
                .iter()
                .map(|(name, phrases)| RubricItem {
                    name: (*name).to_owned(),
                    any_of: phrases.iter().map(|p| (*p).to_owned()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn covered_items_carry_the_part_and_span_and_the_fraction_is_their_share() {
        let window = [
            part("https://a.example", "The cap is set twice a year by Ofgem."),
            part("https://b.example", "It applies per unit, not per bill."),
        ];
        let rubric = rubric(&[
            ("cadence", &["set twice a year"]),
            ("unit-basis", &["applies per unit"]),
            ("absent", &["words no part contains"]),
        ]);
        let record = coverage(&CoverageInput {
            rubric: Some(&rubric),
            window: Some(&window),
        });

        assert_eq!(record["unmeasured"], Value::Null);
        assert_eq!(record["covered_count"], 2);
        assert_eq!(record["item_count"], 3);
        assert_eq!(record["fraction"].as_f64(), Some(2.0 / 3.0));
        assert_eq!(record["rubric"]["name"], "test-rubric");

        let matched = &record["items"][1]["matched"];
        assert_eq!(matched["reference"], "https://b.example");
        let (start, end) = (
            matched["start"].as_u64().unwrap() as usize,
            matched["end"].as_u64().unwrap() as usize,
        );
        // The recorded span excises to the phrase, under the same tolerance
        // the match was made with — the grounding evaluator's own recheck.
        assert!(evaluate::span_excises_to_quote(
            &window[1].text,
            start,
            end,
            "applies per unit"
        ));
        assert_eq!(record["items"][2]["covered"], false);
        assert_eq!(record["items"][2]["matched"], Value::Null);
    }

    /// The matcher is the grounding evaluator's: case folds, trailing
    /// punctuation is not the word, whitespace is flexible — and a different
    /// word is still a different word.
    #[test]
    fn phrase_matching_shares_the_grounding_evaluators_tolerance() {
        let window = [part("u", "Article 53: Obligations for providers.")];
        for phrase in [
            "article 53: obligations",
            "Article 53 Obligations",
            "Article 53:  Obligations",
        ] {
            let rubric = rubric(&[("item", &[phrase])]);
            let record = coverage(&CoverageInput {
                rubric: Some(&rubric),
                window: Some(&window),
            });
            assert_eq!(
                record["items"][0]["covered"], true,
                "{phrase:?} is the part's own words, case and punctuation aside"
            );
        }
        let rubric = rubric(&[("item", &["Article 54: Obligations"])]);
        let record = coverage(&CoverageInput {
            rubric: Some(&rubric),
            window: Some(&window),
        });
        assert_eq!(record["items"][0]["covered"], false);
    }

    /// Any one declared phrase covers the item; the record names which.
    #[test]
    fn an_item_is_covered_by_any_of_its_phrases_and_the_record_names_which() {
        let window = [part("u", "the code helps industry comply with the AI Act")];
        let rubric = rubric(&[(
            "purpose",
            &["compliance with the AI Act", "comply with the AI Act"],
        )]);
        let record = coverage(&CoverageInput {
            rubric: Some(&rubric),
            window: Some(&window),
        });
        assert_eq!(record["items"][0]["covered"], true);
        assert_eq!(
            record["items"][0]["matched"]["phrase"],
            "comply with the AI Act"
        );
    }

    /// A suite with no rubric is unmeasured with the reason stated. No count,
    /// no fraction, and least of all a zero.
    #[test]
    fn no_rubric_is_unmeasured_never_zero() {
        let window = [part("u", "any text at all")];
        let record = coverage(&CoverageInput {
            rubric: None,
            window: Some(&window),
        });
        assert!(
            record["unmeasured"]
                .as_str()
                .is_some_and(|reason| reason.contains("coverage_rubric")),
            "the reason must name the switch that would make it measurable"
        );
        assert_eq!(record["fraction"], Value::Null);
        assert_eq!(record["covered_count"], Value::Null);
        assert_eq!(record["rubric"], Value::Null);
        assert_eq!(record["evaluator"]["name"], "coverage");
    }

    /// A plan that never assembled a window has nothing to measure — and that
    /// is the more fundamental fact, stated even when the suite also declared
    /// no rubric.
    #[test]
    fn an_unassembled_window_is_unmeasured_with_the_plan_level_reason_first() {
        let rubric = rubric(&[("item", &["anything"])]);
        for rubric in [None, Some(&rubric)] {
            let record = coverage(&CoverageInput {
                rubric,
                window: None,
            });
            assert!(
                record["unmeasured"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("no admitted window was assembled")),
                "the absent window is the more fundamental of the two facts"
            );
            assert_eq!(record["fraction"], Value::Null);
        }
    }

    /// An assembled window that admitted nothing genuinely covers no item:
    /// zero here is a measurement — the plan bought nothing usable — not a
    /// stand-in for an unknown.
    #[test]
    fn an_empty_assembled_window_measures_zero_rather_than_abstaining() {
        let rubric = rubric(&[("a", &["alpha"]), ("b", &["beta"])]);
        let record = coverage(&CoverageInput {
            rubric: Some(&rubric),
            window: Some(&[]),
        });
        assert_eq!(record["unmeasured"], Value::Null);
        assert_eq!(record["covered_count"], 0);
        assert_eq!(record["item_count"], 2);
        assert_eq!(record["fraction"].as_f64(), Some(0.0));
    }

    #[test]
    fn a_rubric_that_could_not_be_measured_against_is_refused() {
        let empty_items = CoverageRubric {
            name: "r".into(),
            version: "1".into(),
            items: vec![],
        };
        assert!(empty_items.validate().is_err());

        let duplicate = rubric(&[("twice", &["a"]), ("twice", &["b"])]);
        assert!(duplicate.validate().unwrap_err().contains("twice"));

        let phraseless = rubric(&[("blank", &["   "])]);
        assert!(phraseless.validate().is_err());

        let nameless = CoverageRubric {
            name: "  ".into(),
            version: "1".into(),
            items: vec![RubricItem {
                name: "a".into(),
                any_of: vec!["phrase".into()],
            }],
        };
        assert!(nameless.validate().is_err());

        assert!(rubric(&[("fine", &["a phrase"])]).validate().is_ok());
    }

    /// Nothing derives the sealed rule text from the code, so a behaviour
    /// change can leave it standing still. The pin makes that a failure.
    #[test]
    fn the_rule_text_and_the_digest_move_with_the_evaluator_version() {
        assert!(
            RULES.starts_with(&format!("{EVALUATOR_NAME}/{EVALUATOR_VERSION}:")),
            "the rule text must name the version it describes"
        );
        assert_eq!(
            configuration_digest(),
            "sha256:f53ba04a722c216a2b4f9993cc3c5cd02c65337e6938e9717e3450d07dc492d7",
            "the rule text moved. That is a change of experiment identity: bump \
             EVALUATOR_VERSION, update this pin in the same commit, and expect sealed \
             runs recorded under the old digest to be incomparable"
        );
    }
}
