//! The freshness evaluator: each admitted part's declared date, measured
//! against the suite's declared as-of reference.
//!
//! There is no clock anywhere in this module. The reference date and the
//! staleness horizon are suite input, and every part's date comes from
//! whatever declared it — a supplier's own metadata, the operator's corpus
//! manifest, or the replay manifest's capture date where nothing closer to
//! the content declared one — so two replay runs of one suite produce
//! byte-identical freshness records, and every record names the provenance
//! of every date it trusted. A live acquisition is measurable exactly as far
//! as its supplier declared dates. The fraction it publishes exists only
//! when every admitted part carries a parseable declared date: an undated
//! part is unknown, not stale, and unknown never becomes a value
//! (`docs/FAIL-POLICY.md` §7).

use chrono::NaiveDate;
use commonmeasure_types::canonical::sha256_digest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evaluate::EVALUATION_VERSION;

pub const EVALUATOR_NAME: &str = "freshness";
pub const EVALUATOR_VERSION: &str = "0.2.0";

/// The canonical rule text behind the configuration digest, written by hand
/// under the same convention as the grounding evaluator's: a commit that
/// changes what this evaluator decides changes this text and
/// [`EVALUATOR_VERSION`] together, and the tests below pin the digest.
///
/// The as-of date and horizon are deliberately not part of this digest: they
/// are suite input, sealed by the run manifest, so a different reference
/// date is a different experiment by manifest hash while the evaluator
/// identity stands still.
const RULES: &str = "freshness/0.2.0: a suite may declare as_of — a reference date and a maximum \
     age in days. Every admitted part's declared date is measured against it: a part is \
     within the window when as_of minus maximum_age_days <= date <= as_of, so a part \
     dated after the reference is outside the window, not fresh. Dates are read as \
     ISO calendar dates (YYYY-MM-DD, longer timestamps by their date part) and trusted \
     from whatever declared them — the supplier's own metadata, the operator's corpus \
     manifest, or the replay manifest's capture date where nothing closer to the \
     content declared one — with each part naming that provenance; this runtime reads \
     no clock, so the record is a pure function of suite input and window content. \
     The published fraction is within-window parts over admitted parts, and it exists \
     only when every admitted part carries a parseable declared date. An undated part \
     is unknown: not stale, not fresh, and not excluded from the denominator — a \
     fraction computed over only the dated parts would rank a mostly undated window \
     as if it were measured, so a window holding any undated part publishes no \
     fraction and is unmeasured naming the undated parts, while every dated part's \
     own age and verdict still stand in the per-part record. An empty window, a \
     plan with no assembled window, or a suite with no as_of, is unmeasured with the \
     reason stated, never zero.";

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
    "Every date is declared metadata — a supplier's own claim, the operator's corpus \
     manifest, or a replay manifest's capture date — and is trusted as declared; \
     nothing here verifies when content was actually written.",
    "A part dated only by its capture date may be older than the day it was fetched, \
     and a supplier-declared date is still only the supplier's word.",
    "Within the declared window is binary; the record carries each part's age in days \
     for a reader who needs finer judgement.",
    "One undated part withholds the whole fraction: a window that is mostly dated \
     publishes no number, and its dated majority is visible only in the per-part \
     record.",
];

/// A suite's declared freshness reference: the date the measurement is taken
/// against, and the horizon that makes a fraction derivable from dates at
/// all. Both are suite input, sealed by the run manifest.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsOf {
    /// ISO calendar date, `YYYY-MM-DD`.
    pub date: String,
    /// How many days before `date` a part may be dated and still count as
    /// within the window. Declared, because a date alone yields no fraction:
    /// someone must say how old is too old, and that someone is the suite.
    pub maximum_age_days: u32,
}

impl AsOf {
    pub fn validate(&self) -> Result<(), String> {
        parse_date(&self.date).map(|_| ()).ok_or_else(|| {
            format!(
                "as_of.date {:?} is not an ISO calendar date (YYYY-MM-DD), so nothing \
                 could be measured against it",
                self.date
            )
        })
    }
}

/// One date as something declared it: the date text and who declared it, so
/// the record can name the provenance beside every figure derived from it.
/// The shared supply type, because the declaration is made where the supply
/// enters — an adapter mapping its provider's field, or the run wiring
/// falling back to the replay capture date — and measured here.
pub use commonmeasure_types::DeclaredDate;

/// One admitted window part with whatever date was declared for it.
pub struct DatedPart<'a> {
    pub reference: &'a str,
    pub content_hash: &'a str,
    pub declared: Option<&'a DeclaredDate>,
}

/// What the runtime hands the evaluator. `window: None` means no admitted
/// window was assembled for this plan; `Some(&[])` is an assembled window
/// that admitted nothing — which leaves nothing to date, so it too is
/// unmeasured, unlike coverage where the empty window measurably covers no
/// item.
pub struct FreshnessInput<'a> {
    pub as_of: Option<&'a AsOf>,
    pub window: Option<&'a [DatedPart<'a>]>,
}

/// A date as declared metadata carries it: a bare ISO date, or a longer
/// timestamp whose leading ten characters are one (`2025-07-14T00:00:00`).
pub(crate) fn parse_date(text: &str) -> Option<NaiveDate> {
    let date_part = text
        .get(..10)
        .filter(|_| text.len() == 10 || text.as_bytes().get(10) == Some(&b'T'))?;
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()
}

/// Evaluate one plan. Always returns a record — an unmeasured plan carries
/// the evaluator's identity and the stated reason, never a bare null.
pub fn freshness(input: &FreshnessInput) -> Value {
    // Ordered by how fundamental the obstacle is, plan before suite, as the
    // other evaluators order theirs.
    let mut unmeasured: Option<String> = match (input.window, input.as_of) {
        (None, _) => Some(
            "no admitted window was assembled for this plan, so there are no parts to \
             date"
                .to_owned(),
        ),
        (Some(_), None) => Some(
            "the suite declares no as_of reference date, and freshness is measured only \
             against one (set as_of)"
                .to_owned(),
        ),
        (Some([]), Some(_)) => {
            Some("the admitted window holds no parts, so there is nothing to date".to_owned())
        }
        _ => None,
    };
    let reference = input.as_of.and_then(|as_of| parse_date(&as_of.date));
    if unmeasured.is_none() && reference.is_none() {
        // Defensive: `execute` validates the suite before running, so this
        // records a caller that did not, rather than guessing a date.
        unmeasured = Some(format!(
            "the declared as_of date {:?} does not parse as an ISO calendar date, so \
             nothing can be measured against it",
            input.as_of.map(|as_of| as_of.date.as_str()).unwrap_or("")
        ));
    }

    let mut inputs = Vec::new();
    let mut parts = Vec::new();
    let mut undated: Vec<&str> = Vec::new();
    let mut fresh = 0usize;
    for part in input.window.into_iter().flatten() {
        inputs.push(json!({
            "reference": part.reference,
            "content_hash": part.content_hash,
        }));
        let declared = part.declared;
        let parsed = declared.and_then(|declared| parse_date(&declared.date));
        let (age_days, within) = match (parsed, reference, input.as_of) {
            (Some(date), Some(reference), Some(as_of)) => {
                let age = (reference - date).num_days();
                let within = age >= 0 && age <= i64::from(as_of.maximum_age_days);
                if within {
                    fresh += 1;
                }
                (json!(age), json!(within))
            }
            _ => (Value::Null, Value::Null),
        };
        if parsed.is_none() {
            undated.push(part.reference);
        }
        parts.push(json!({
            "reference": part.reference,
            "content_hash": part.content_hash,
            // The date exactly as declared, even where it does not parse: the
            // record shows what was trusted, or what could not be.
            "declared_date": declared.map(|declared| declared.date.clone()),
            "date_provenance": declared.map(|declared| declared.provenance.clone()),
            "age_days": age_days,
            "within_maximum_age": within,
        }));
    }
    if unmeasured.is_none() && !undated.is_empty() {
        unmeasured = Some(format!(
            "{} carr{} no parseable declared date, and an undated part is unknown \
             rather than stale, so no fraction exists for this plan",
            undated.join(", "),
            crate::carries(undated.len()),
        ));
    }

    let (fresh_count, part_count, fraction) = match (&unmeasured, input.window) {
        (None, Some(window)) => (
            json!(fresh),
            json!(window.len()),
            json!(fresh as f64 / window.len() as f64),
        ),
        // An unmeasured record carries no counts at all: a zero here would be
        // the fabricated measurement FAIL-POLICY §7 forbids.
        _ => (Value::Null, Value::Null, Value::Null),
    };

    json!({
        "record_version": EVALUATION_VERSION,
        "evaluator": identity(),
        "method": "deterministic comparison of each admitted part's declared date against \
                   the suite's as_of reference and maximum age; no clock is read",
        "as_of": input.as_of.map(|as_of| json!({
            "date": as_of.date,
            "maximum_age_days": as_of.maximum_age_days,
        })),
        "inputs": inputs,
        "parts": parts,
        "fresh_count": fresh_count,
        "part_count": part_count,
        "fraction": fraction,
        "unmeasured": unmeasured,
        // Every figure rests on dates declared by something else — named per
        // part in date_provenance — never on an observation this runtime made.
        "assurance": "declared",
        "blind_spots": BLIND_SPOTS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_of(date: &str, maximum_age_days: u32) -> AsOf {
        AsOf {
            date: date.to_owned(),
            maximum_age_days,
        }
    }

    fn declared(date: &str) -> DeclaredDate {
        DeclaredDate {
            date: date.to_owned(),
            provenance: "test manifest capture date".to_owned(),
        }
    }

    fn dated<'a>(reference: &'a str, declared: Option<&'a DeclaredDate>) -> DatedPart<'a> {
        DatedPart {
            reference,
            content_hash: "sha256:test",
            declared,
        }
    }

    #[test]
    fn parts_within_the_declared_window_are_fresh_and_the_record_names_provenance() {
        let recent = declared("2026-08-01");
        let stale = declared("2026-01-01");
        let parts = [
            dated("https://a", Some(&recent)),
            dated("https://b", Some(&stale)),
        ];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });

        assert_eq!(record["unmeasured"], Value::Null);
        assert_eq!(record["fresh_count"], 1);
        assert_eq!(record["part_count"], 2);
        assert_eq!(record["fraction"].as_f64(), Some(0.5));
        assert_eq!(record["parts"][0]["age_days"], 4);
        assert_eq!(record["parts"][0]["within_maximum_age"], true);
        assert_eq!(record["parts"][1]["within_maximum_age"], false);
        assert_eq!(
            record["parts"][0]["date_provenance"], "test manifest capture date",
            "every trusted date must name what declared it"
        );
        assert_eq!(record["as_of"]["date"], "2026-08-05");
    }

    /// A part dated after the reference is outside the frozen window, not
    /// fresh: the suite froze a time window and this content postdates it.
    #[test]
    fn a_part_dated_after_the_reference_is_outside_the_window() {
        let future = declared("2026-08-09");
        let parts = [dated("https://a", Some(&future))];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });
        assert_eq!(record["parts"][0]["age_days"], -4);
        assert_eq!(record["parts"][0]["within_maximum_age"], false);
        assert_eq!(record["fraction"].as_f64(), Some(0.0));
    }

    /// An undated part is unknown, not stale. The whole fraction goes rather
    /// than counting the silent part either way — the same discipline that
    /// keeps an undisclosed charge from passing a budget.
    #[test]
    fn an_undated_part_makes_the_plan_unmeasured_naming_the_part() {
        let dated_part = declared("2026-08-01");
        let parts = [
            dated("https://dated.example", Some(&dated_part)),
            dated("https://undated.example", None),
        ];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });
        let reason = record["unmeasured"].as_str().expect("a reason");
        assert!(
            reason.contains("https://undated.example") && !reason.contains("https://dated.example"),
            "the reason names the undated part and only it: {reason}"
        );
        assert_eq!(record["fraction"], Value::Null);
        assert_eq!(record["fresh_count"], Value::Null);
        // The dated part's own figures still stand in the per-part record.
        assert_eq!(record["parts"][0]["within_maximum_age"], true);
        assert_eq!(record["parts"][1]["declared_date"], Value::Null);
        // The undated part never reads as fresh or stale: its verdict and its
        // age are absent, not false and not zero.
        assert_eq!(record["parts"][1]["within_maximum_age"], Value::Null);
        assert_eq!(record["parts"][1]["age_days"], Value::Null);
    }

    /// One window may hold dates from different declarers — a
    /// supplier-declared part beside a capture-dated one — and each part's
    /// record names its own provenance rather than the window sharing one.
    #[test]
    fn mixed_provenances_are_named_per_part() {
        let supplier = DeclaredDate {
            date: "2026-08-01".to_owned(),
            provenance: "supplier-declared: publishedDate on the TollBit search item".to_owned(),
        };
        let capture = DeclaredDate {
            date: "2026-08-01".to_owned(),
            provenance: "replay recording capture date, declared by the replay manifest for \
                         exa/exa-search.json"
                .to_owned(),
        };
        let parts = [
            dated("https://a", Some(&supplier)),
            dated("https://b", Some(&capture)),
        ];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });
        assert_eq!(record["fraction"].as_f64(), Some(1.0));
        assert!(
            record["parts"][0]["date_provenance"]
                .as_str()
                .is_some_and(|p| p.contains("supplier-declared"))
        );
        assert!(
            record["parts"][1]["date_provenance"]
                .as_str()
                .is_some_and(|p| p.contains("replay recording capture date"))
        );
    }

    /// A date that does not parse is recorded verbatim and treated as
    /// undated, never guessed at.
    #[test]
    fn an_unparseable_date_is_carried_verbatim_and_unmeasured() {
        let odd = declared("last Tuesday");
        let parts = [dated("https://a", Some(&odd))];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });
        assert_eq!(record["parts"][0]["declared_date"], "last Tuesday");
        assert_eq!(record["parts"][0]["age_days"], Value::Null);
        assert!(record["unmeasured"].as_str().is_some());
    }

    /// Timestamps read by their date part: supplier metadata commonly carries
    /// `2025-07-14T00:00:00`.
    #[test]
    fn a_timestamp_is_read_by_its_date_part() {
        let stamped = declared("2026-08-01T12:12:04");
        let parts = [dated("https://a", Some(&stamped))];
        let record = freshness(&FreshnessInput {
            as_of: Some(&as_of("2026-08-05", 30)),
            window: Some(&parts),
        });
        assert_eq!(record["parts"][0]["age_days"], 4);
        assert_eq!(record["fraction"].as_f64(), Some(1.0));
        // But ten digits followed by anything else is not a date.
        assert!(parse_date("2026-08-01x").is_none());
        assert!(parse_date("2026-08").is_none());
    }

    /// The three unmeasured-window states, each with its own stated reason,
    /// none of them zero.
    #[test]
    fn absent_windows_and_absent_as_of_are_unmeasured_with_the_reason_stated() {
        let reference = as_of("2026-08-05", 30);
        let record = freshness(&FreshnessInput {
            as_of: Some(&reference),
            window: None,
        });
        assert!(
            record["unmeasured"]
                .as_str()
                .is_some_and(|reason| reason.contains("no admitted window was assembled"))
        );

        let record = freshness(&FreshnessInput {
            as_of: Some(&reference),
            window: Some(&[]),
        });
        assert!(
            record["unmeasured"]
                .as_str()
                .is_some_and(|reason| reason.contains("holds no parts")),
            "an empty window leaves nothing to date; unlike coverage, no zero is honest here"
        );

        let date = declared("2026-08-01");
        let parts = [dated("https://a", Some(&date))];
        let record = freshness(&FreshnessInput {
            as_of: None,
            window: Some(&parts),
        });
        assert!(
            record["unmeasured"]
                .as_str()
                .is_some_and(|reason| reason.contains("as_of")),
            "the reason must name the switch that would make it measurable"
        );
        assert_eq!(record["fraction"], Value::Null);
    }

    #[test]
    fn as_of_validation_refuses_a_date_nothing_could_be_measured_against() {
        assert!(as_of("2026-08-05", 30).validate().is_ok());
        assert!(as_of("last week", 30).validate().is_err());
        assert!(as_of("2026-13-40", 30).validate().is_err());
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
            "sha256:98aa6b492c8cbf2ff6a76aa5762b3f112041a03af998ac149c3674a6f034d103",
            "the rule text moved. That is a change of experiment identity: bump \
             EVALUATOR_VERSION, update this pin in the same commit, and expect sealed \
             runs recorded under the old digest to be incomparable"
        );
    }
}
