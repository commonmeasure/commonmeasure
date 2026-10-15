//! A reply to be displayed against a signed release, and what the display
//! rule did with each of its items.
//!
//! The reply's items say what they are: a quote of the release by passage
//! id, a sentence drawn from the source with the passages it draws on, or a
//! sentence of the assistant's own. The display rule
//! (`commonmeasure_runtime::processor::integrity`) gives each item one
//! [`Outcome`], and one display produces one [`Counts`] vector.
//!
//! The outcome vocabulary and the counts are a working rule offered to the
//! open standards bodies, not Common Measure's to define
//! (`docs/contracts/display-counts.md`).

use serde::{Deserialize, Serialize};

/// What a reply asks to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    /// The release's own words, by passage id: the whole passage, or one
    /// sentence of it by index.
    Quote {
        passage: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sentence: Option<usize>,
    },
    /// A sentence drawn from the source. `summary` is the name the trusted
    /// news demo's replies use.
    #[serde(alias = "summary")]
    FromSource {
        text: String,
        /// The ids of the passages the sentence draws on; empty where none
        /// were cited.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        passages: Vec<String>,
    },
    /// The assistant's own reasoning or knowledge, not drawn from the
    /// source: a conversion, a comparison, an inference.
    Own { text: String },
}

/// A reply: its items in the order they are shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub items: Vec<Item>,
}

/// What the display rule did with one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// A quote rendered from the signed text.
    QuoteShown,
    /// A quote that cut a kept span, shown as the whole span.
    QuoteExpandedToKept,
    /// A quote naming a passage or sentence the release does not have.
    QuoteRefused,
    /// A sentence from the source that passed the support check, shown
    /// plain with its passages.
    ShownConfirmed,
    /// The assistant's own sentence, shown labelled as not from the source.
    ShownOwn,
    /// A sentence from the source that was not confirmed, shown labelled as
    /// not from the source.
    ShownUnconfirmed,
    /// A sentence that contradicts the source; the source's words are shown.
    WithheldContradiction,
    /// A sentence that attributes a quotation to someone the source does
    /// not; the source's words are shown.
    WithheldAttribution,
    /// A sentence that cuts a kept span; the whole span is shown.
    WithheldKept,
    /// Under strict display, a sentence not confirmed or the assistant's
    /// own; the source's words are shown.
    WithheldUnconfirmed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuoteShown => "quote_shown",
            Self::QuoteExpandedToKept => "quote_expanded_to_kept",
            Self::QuoteRefused => "quote_refused",
            Self::ShownConfirmed => "shown_confirmed",
            Self::ShownOwn => "shown_own",
            Self::ShownUnconfirmed => "shown_unconfirmed",
            Self::WithheldContradiction => "withheld_contradiction",
            Self::WithheldAttribution => "withheld_attribution",
            Self::WithheldKept => "withheld_kept",
            Self::WithheldUnconfirmed => "withheld_unconfirmed",
        }
    }
}

/// The counts one display produces: the release fetched once, then one per
/// outcome, in [`Counts::NAMES`] order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub fetch: u32,
    pub quote_shown: u32,
    pub quote_expanded_to_kept: u32,
    pub quote_refused: u32,
    pub shown_confirmed: u32,
    pub shown_own: u32,
    pub shown_unconfirmed: u32,
    pub withheld_contradiction: u32,
    pub withheld_attribution: u32,
    pub withheld_kept: u32,
    pub withheld_unconfirmed: u32,
}

impl Counts {
    /// The counts' names in vector order.
    pub const NAMES: [&'static str; 11] = [
        "fetch",
        "quote_shown",
        "quote_expanded_to_kept",
        "quote_refused",
        "shown_confirmed",
        "shown_own",
        "shown_unconfirmed",
        "withheld_contradiction",
        "withheld_attribution",
        "withheld_kept",
        "withheld_unconfirmed",
    ];

    /// The counts for one display of one fetched release, before any item.
    pub fn one_fetch() -> Self {
        Self {
            fetch: 1,
            ..Self::default()
        }
    }

    pub fn add(&mut self, outcome: Outcome) {
        let count = match outcome {
            Outcome::QuoteShown => &mut self.quote_shown,
            Outcome::QuoteExpandedToKept => &mut self.quote_expanded_to_kept,
            Outcome::QuoteRefused => &mut self.quote_refused,
            Outcome::ShownConfirmed => &mut self.shown_confirmed,
            Outcome::ShownOwn => &mut self.shown_own,
            Outcome::ShownUnconfirmed => &mut self.shown_unconfirmed,
            Outcome::WithheldContradiction => &mut self.withheld_contradiction,
            Outcome::WithheldAttribution => &mut self.withheld_attribution,
            Outcome::WithheldKept => &mut self.withheld_kept,
            Outcome::WithheldUnconfirmed => &mut self.withheld_unconfirmed,
        };
        *count += 1;
    }

    /// The counts as one vector in [`Counts::NAMES`] order.
    pub fn as_vec(&self) -> [u32; 11] {
        [
            self.fetch,
            self.quote_shown,
            self.quote_expanded_to_kept,
            self.quote_refused,
            self.shown_confirmed,
            self.shown_own,
            self.shown_unconfirmed,
            self.withheld_contradiction,
            self.withheld_attribution,
            self.withheld_kept,
            self.withheld_unconfirmed,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outcome_counts_under_its_own_name() {
        let outcomes = [
            Outcome::QuoteShown,
            Outcome::QuoteExpandedToKept,
            Outcome::QuoteRefused,
            Outcome::ShownConfirmed,
            Outcome::ShownOwn,
            Outcome::ShownUnconfirmed,
            Outcome::WithheldContradiction,
            Outcome::WithheldAttribution,
            Outcome::WithheldKept,
            Outcome::WithheldUnconfirmed,
        ];
        for (position, outcome) in outcomes.into_iter().enumerate() {
            let mut counts = Counts::one_fetch();
            counts.add(outcome);
            let mut expected = [0; 11];
            expected[0] = 1;
            expected[position + 1] = 1;
            assert_eq!(counts.as_vec(), expected, "{outcome:?}");
            assert_eq!(Counts::NAMES[position + 1], outcome.as_str());
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                serde_json::json!(outcome.as_str())
            );
        }
    }

    #[test]
    fn the_reply_items_say_what_they_are_and_the_demo_name_still_parses() {
        let reply: Reply = serde_json::from_str(
            r#"{"items":[{"kind":"quote","passage":"p1","sentence":2},{"kind":"from_source","text":"a","passages":["p1"]},{"kind":"own","text":"b"},{"kind":"summary","text":"c"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            &reply.items[0],
            Item::Quote {
                sentence: Some(2),
                ..
            }
        ));
        assert!(
            matches!(&reply.items[1], Item::FromSource { passages, .. } if passages == &["p1"])
        );
        assert!(matches!(&reply.items[2], Item::Own { text } if text == "b"));
        assert!(
            matches!(&reply.items[3], Item::FromSource { passages, .. } if passages.is_empty())
        );
        assert!(
            serde_json::from_str::<Reply>(r#"{"items":[{"kind":"opinion","text":"x"}]}"#).is_err()
        );
    }
}
