//! Read-time engagement attribution: which engagement a session is read under.
//!
//! Attribution is a projection, not a partition. The evidence logs record the
//! fact — the `cwd` a crossing happened in — and this file resolves what that
//! directory means during aggregation, so editing a rule re-attributes history
//! without a byte of evidence changing.
//!
//! What it resolves is the **reported** engagement: reporting identity, the
//! name every console figure counts, groups and filters under. It is not the
//! governing engagement a policy scope declares, which is capture-time
//! enforcement identity and the only name the relay and the runtime can read —
//! this crate sits above them in the graph, so no enforcing seam can consult
//! these rules (`DECISIONS.md` §Session policy and egress). Where both name the same directory they are expected
//! to agree; the policy panel reads both and reports where they do not.
//!
//! Rules are ordered substring matches, re-read on every rollup; records join
//! on `session_id`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Writing this file is the same job as writing the session policy — atomic,
/// revision-checked, locked across processes — so it is the same code
/// (`commonmeasure_harness::declaration`), and only the rule file's own name, validity
/// rule and errors live here.
use commonmeasure_harness::declaration;
pub use commonmeasure_harness::declaration::{Lock as RuleLock, LockRefused};

/// The engagement for work no rule names. Shown plainly rather than hidden: a
/// console that only lists what it can label understates what it holds.
pub const UNATTRIBUTED: &str = "unattributed";

/// `$COMMONMEASURE_HOME/attribution.json`:
///
/// ```json
/// {"rules": [
///   {"match": "code/ozone", "engagement": "ozone"},
///   {"match": "common-measure", "engagement": "commonmeasure"}
/// ]}
/// ```
#[derive(Debug, Default, Deserialize, serde::Serialize)]
struct RuleFile {
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct Rule {
    /// Substring matched against a recorded cwd.
    #[serde(rename = "match")]
    matcher: String,
    engagement: String,
}

#[derive(Debug, Default)]
pub struct Attribution {
    rules: Vec<Rule>,
}

impl Attribution {
    /// No rules: everything resolves to [`UNATTRIBUTED`]. What an absent file
    /// means, and the state in which every aggregate is unchanged from a
    /// store that never heard of engagements.
    pub fn none() -> Self {
        Self::default()
    }

    /// Read `<home>/attribution.json` once, for everything one answer needs
    /// from it: the rules, and the token naming the bytes they were parsed
    /// from. A page that read the file twice could straddle an edit and then
    /// show an editor whose stated revision belongs to bytes it never
    /// displayed.
    ///
    /// `Err` means the bytes could not be read at all, so there is no token to
    /// state; `rules` is `Err` when the bytes are not a rule file, which is
    /// still a revision an editor can replace.
    pub fn read(home: &Path) -> Result<RuleSnapshot> {
        let source = home.join("attribution.json");
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RuleSnapshot {
                    rules: Ok(Self::none()),
                    revision: declaration::ABSENT.to_owned(),
                });
            }
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("read attribution rules {}", source.display()));
            }
        };
        Ok(RuleSnapshot {
            revision: declaration::revision_of(&encoded),
            rules: parse(&encoded, &source),
        })
    }

    /// Load `<home>/attribution.json`, re-read on every aggregation so an
    /// edit takes effect on the next read. Absent is fine — attribution is
    /// optional — but a malformed file is an error, not an empty rule set: a
    /// console that silently showed everything unattributed because of a
    /// typo would look exactly like one that was never configured.
    pub fn load(home: &Path) -> Result<Self> {
        Self::read(home)?.rules
    }

    /// The ordered rules as (match, engagement) pairs, for the console's
    /// editor. Order is precedence, so it is preserved exactly.
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.rules
            .iter()
            .map(|rule| (rule.matcher.clone(), rule.engagement.clone()))
            .collect()
    }

    /// Write `<home>/attribution.json`: validate first, then replace the file
    /// atomically, so a reader never sees a half-written rule set and a
    /// rejected set never touches the file at all
    /// (`commonmeasure_harness::declaration::replace`).
    pub fn save(home: &Path, pairs: &[(String, String)]) -> Result<()> {
        let rules: Vec<Rule> = pairs
            .iter()
            .map(|(matcher, engagement)| Rule {
                matcher: matcher.clone(),
                engagement: engagement.clone(),
            })
            .collect();
        validate(&rules)?;
        let encoded = serde_json::to_vec_pretty(&RuleFile { rules })?;
        declaration::replace(&home.join("attribution.json"), &encoded).map_err(anyhow::Error::msg)
    }

    /// Hold this home's rule file exclusively, across processes, for as long
    /// as the returned guard lives, or refuse rather than park the request
    /// (`commonmeasure_harness::declaration::lock`).
    ///
    /// The console's in-process mutex cannot see a second console on the same
    /// `COMMONMEASURE_HOME`, which is an ordinary way to run this. The revision
    /// check and the rename have to be one step against *any* writer, or a
    /// concurrent edit is lost silently rather than refused with both
    /// versions stated.
    pub fn lock(home: &Path) -> std::result::Result<RuleLock, LockRefused> {
        declaration::lock(&home.join("attribution.lock"))
    }

    /// A revision token over the current file bytes, so an editor can state
    /// what it read before asking to replace it. "absent" when no file exists.
    pub fn revision(home: &Path) -> Result<String> {
        Ok(Self::read(home)?.revision)
    }

    /// First rule whose `match` is a substring of the cwd wins.
    pub fn resolve(&self, cwd: &str) -> Option<&str> {
        self.rules
            .iter()
            .find(|rule| cwd.contains(&rule.matcher))
            .map(|rule| rule.engagement.as_str())
    }

    /// Resolve a possibly-absent cwd to an engagement name, never to silence.
    pub fn engagement_of(&self, cwd: Option<&str>) -> &str {
        cwd.and_then(|cwd| self.resolve(cwd))
            .unwrap_or(UNATTRIBUTED)
    }
}

/// The rule file as one read: see [`Attribution::read`].
pub struct RuleSnapshot {
    /// The rules these bytes parse to, or why they are not a rule file.
    pub rules: Result<Attribution>,
    /// A token over the bytes `rules` was parsed from; "absent" when there is
    /// no file.
    pub revision: String,
}

/// Rule-file bytes as rules. `source` only names the file in the error, so a
/// reader is told which file to fix.
fn parse(encoded: &[u8], source: &Path) -> Result<Attribution> {
    let file: RuleFile = serde_json::from_slice(encoded)
        .with_context(|| format!("{} is not a valid rule file", source.display()))?;
    validate(&file.rules).with_context(|| format!("{} holds an invalid rule", source.display()))?;
    Ok(Attribution { rules: file.rules })
}

/// The one validity rule, shared by [`Attribution::read`] and
/// [`Attribution::save`] so the console cannot write what a load would refuse.
fn validate(rules: &[Rule]) -> Result<()> {
    for (index, rule) in rules.iter().enumerate() {
        if rule.matcher.is_empty() || rule.engagement.is_empty() {
            bail!(
                "rule {} has an empty \"match\" or \"engagement\"",
                index + 1
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_substring_rules_first_match_wins() {
        let attribution = Attribution {
            rules: vec![
                Rule {
                    matcher: "code/ozone".into(),
                    engagement: "ozone".into(),
                },
                Rule {
                    matcher: "code".into(),
                    engagement: "personal".into(),
                },
            ],
        };
        assert_eq!(attribution.resolve("/home/x/code/ozone/api"), Some("ozone"));
        assert_eq!(attribution.resolve("/home/x/code/other"), Some("personal"));
        assert_eq!(attribution.resolve("/home/x/docs"), None);
        assert_eq!(attribution.engagement_of(None), UNATTRIBUTED);
    }

    #[test]
    fn an_absent_file_attributes_nothing_and_errors_never() {
        let home = tempfile::tempdir().unwrap();
        let attribution = Attribution::load(home.path()).expect("absent is fine");
        assert_eq!(attribution.engagement_of(Some("/anywhere")), UNATTRIBUTED);
    }

    #[test]
    fn save_round_trips_in_order_and_refuses_an_empty_field() {
        let home = tempfile::tempdir().unwrap();
        let pairs = vec![
            ("code/ozone".to_owned(), "ozone".to_owned()),
            ("code".to_owned(), "personal".to_owned()),
        ];
        Attribution::save(home.path(), &pairs).expect("valid rules save");
        let loaded = Attribution::load(home.path()).expect("saved file loads");
        assert_eq!(loaded.pairs(), pairs);

        let before = Attribution::revision(home.path()).unwrap();
        let invalid = vec![("".to_owned(), "x".to_owned())];
        assert!(Attribution::save(home.path(), &invalid).is_err());
        // A rejected set never touches the file, and leaves no temp file.
        assert_eq!(Attribution::revision(home.path()).unwrap(), before);
        assert!(!home.path().join("attribution.json.tmp").exists());
    }

    #[test]
    fn revision_names_an_absent_file_and_tracks_edits() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Attribution::revision(home.path()).unwrap(), "absent");
        Attribution::save(home.path(), &[("a".into(), "b".into())]).unwrap();
        let first = Attribution::revision(home.path()).unwrap();
        assert_ne!(first, "absent");
        Attribution::save(home.path(), &[("a".into(), "c".into())]).unwrap();
        assert_ne!(Attribution::revision(home.path()).unwrap(), first);
    }

    /// One read has to answer both questions the sessions page asks of this
    /// file, and it has to keep answering the second one when the first fails:
    /// a file that does not parse still has a revision, and that revision is
    /// what makes the repair editor's save a replacement rather than a clobber.
    #[test]
    fn one_read_carries_both_the_rules_and_the_token_over_their_bytes() {
        let home = tempfile::tempdir().unwrap();
        let absent = Attribution::read(home.path()).unwrap();
        assert_eq!(absent.revision, "absent");
        assert_eq!(
            absent.rules.unwrap().engagement_of(Some("/anywhere")),
            UNATTRIBUTED
        );

        Attribution::save(home.path(), &[("code/ozone".into(), "ozone".into())]).unwrap();
        let saved = Attribution::read(home.path()).unwrap();
        assert_eq!(saved.revision, Attribution::revision(home.path()).unwrap());
        assert_eq!(
            saved.rules.unwrap().resolve("/code/ozone/api"),
            Some("ozone")
        );

        std::fs::write(home.path().join("attribution.json"), "{ not json").unwrap();
        let broken = Attribution::read(home.path()).unwrap();
        assert!(broken.rules.is_err(), "a typo is not an empty rule set");
        assert_ne!(broken.revision, "absent", "bad bytes still have a token");
    }

    /// The lock runs on a served request, so it has to end. Held elsewhere, it
    /// refuses with the wait named rather than parking the handler that took
    /// it — enough parked handlers and the console answers nothing at all.
    #[test]
    fn a_lock_another_holder_keeps_is_refused_with_the_wait_named() {
        let home = tempfile::tempdir().unwrap();
        let held = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(home.path().join("attribution.lock"))
            .unwrap();
        held.lock().unwrap();

        let refusal = Attribution::lock(home.path()).unwrap_err();
        assert!(
            matches!(refusal, LockRefused::Busy(_)),
            "a held lock is busy, not broken: {refusal}"
        );
        assert!(
            refusal.to_string().contains("no edit was made"),
            "the refusal says what did not happen: {refusal}"
        );
        // That the wait is bounded at all is the shared module's own test
        // (`commonmeasure_harness::declaration`); what this one pins is that the rule
        // file's lock is the file beside it, so a second console on this home
        // meets it.

        // Released, the very next attempt takes it.
        drop(held);
        assert!(Attribution::lock(home.path()).is_ok());
    }

    /// A typo must not impersonate an unconfigured machine.
    #[test]
    fn a_malformed_rule_file_is_an_error_not_an_empty_rule_set() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("attribution.json"), "{ not json").unwrap();
        assert!(Attribution::load(home.path()).is_err());

        std::fs::write(
            home.path().join("attribution.json"),
            r#"{"rules":[{"match":"","engagement":"x"}]}"#,
        )
        .unwrap();
        assert!(Attribution::load(home.path()).is_err());
    }
}
