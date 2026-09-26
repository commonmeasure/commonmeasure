//! One line of an inspection report, with how it stands.
//!
//! `commonmeasure doctor`, `status` and the relay's egress account each
//! say what they found in one sentence per fact. A reader scanning the
//! report needs to know which sentences call for action before reading
//! them, and a script needs the same without parsing prose, so each
//! sentence carries a [`Standing`] chosen where the fact is established,
//! by the code that knows whether "not running" is a stopped service or a
//! service that was never asked for. The text itself is unchanged by the
//! standing: the words are the finding, the standing is how to read it.

use serde::{Deserialize, Serialize};

/// How a finding reads: whether it needs the operator, whether it could be
/// determined at all, or whether it is a fact with no judgement attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// What was checked is as expected: a registration in place, a policy
    /// that loads, a delivery that was accepted.
    Ok,
    /// Something for the operator to act on: a registered binary that has
    /// gone, a policy that does not load, dead relay batches.
    Attention,
    /// Could not be determined, and the text says why: a file that does
    /// not read, a lock another user holds. Never reported as either of the
    /// two above (`docs/FAIL-POLICY.md` §7).
    Unknown,
    /// A fact with nothing to act on: a host that has no hook surface, a
    /// host nobody registered, a count of zero.
    Note,
}

/// One sentence of a report and its standing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub standing: Standing,
    pub text: String,
}

impl Finding {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            standing: Standing::Ok,
            text: text.into(),
        }
    }

    pub fn attention(text: impl Into<String>) -> Self {
        Self {
            standing: Standing::Attention,
            text: text.into(),
        }
    }

    pub fn unknown(text: impl Into<String>) -> Self {
        Self {
            standing: Standing::Unknown,
            text: text.into(),
        }
    }

    pub fn note(text: impl Into<String>) -> Self {
        Self {
            standing: Standing::Note,
            text: text.into(),
        }
    }

    /// The texts of a list of findings, one per line, each ended by a
    /// newline: the form the older text-only reports print.
    pub fn lines(findings: &[Finding]) -> String {
        findings
            .iter()
            .map(|finding| format!("{}\n", finding.text))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_serialises_its_standing_as_a_word() {
        let json = serde_json::to_value(Finding::attention("binary /x: not found")).unwrap();
        assert_eq!(json["standing"], "attention");
        assert_eq!(json["text"], "binary /x: not found");
        let back: Finding = serde_json::from_value(json).unwrap();
        assert_eq!(back.standing, Standing::Attention);
    }

    #[test]
    fn lines_joins_the_texts_and_ends_each_with_a_newline() {
        let text = Finding::lines(&[Finding::ok("one"), Finding::note("two")]);
        assert_eq!(text, "one\ntwo\n");
        assert_eq!(Finding::lines(&[]), "");
    }
}
