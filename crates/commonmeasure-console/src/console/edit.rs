//! The console's writes to the declared artefacts. Every console policy
//! write validates through the runtime's own loader, saves atomically behind
//! a revision check, shows the effect of a widening field before the save,
//! never creates a declaration where none exists, and is keyed by scope.
//!
//! One write path for the policy file, whatever field is edited. A write
//! holds the file's cross-process lock, reads the declaration, checks that the
//! revision the form was rendered from is still the revision on disk, builds
//! a candidate through the runtime's own document, and saves it through the
//! runtime's own loader. Each of those is a condition an editor could quietly
//! weaken, so an edit supplies only the step that is its own: the function
//! from the current document to the candidate. A later editor of another
//! field, such as an add-on's settings for a scope, supplies that function
//! and inherits every condition.
//!
//! What every outcome states back is text an operator reads on the page. A
//! conflict names both revisions; a refusal names what the loader refused;
//! a save names what changed and that the change governs the next crossing
//! and nothing already recorded. The console never creates a policy file:
//! with no declaration there is nothing to change, and inventing one from a
//! form would put a stance on every session on the machine that the operator
//! never wrote.

use std::path::Path;

use commonmeasure_harness::declaration::LockRefused;
use commonmeasure_harness::policy::{PolicyDocument, PolicyFile, PolicyMode};
use commonmeasure_types::{AccessAction, HostPattern};

use crate::attribution::Attribution;

/// The candidate an edit built from the current document.
pub struct Candidate {
    pub file: PolicyFile,
    /// False when the declaration already says what the edit asked for; the
    /// file is then left alone and the outcome says so.
    pub changed: bool,
    /// What changed, in words, for the page.
    pub notice: String,
}

/// What a write did, with the sentence the page shows for it. The HTTP status
/// follows the kind: a save and an unchanged declaration answer 200, a
/// conflict 409, and a refusal the status of its cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Saved(String),
    Unchanged(String),
    Conflict(String),
    Refused { status: u16, notice: String },
}

impl Outcome {
    pub fn status(&self) -> u16 {
        match self {
            Self::Saved(_) | Self::Unchanged(_) => 200,
            Self::Conflict(_) => 409,
            Self::Refused { status, .. } => *status,
        }
    }

    pub fn notice(&self) -> &str {
        match self {
            Self::Saved(notice)
            | Self::Unchanged(notice)
            | Self::Conflict(notice)
            | Self::Refused { notice, .. } => notice,
        }
    }

    /// The word the page marks the notice with.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Saved(_) => "saved",
            Self::Unchanged(_) => "unchanged",
            Self::Conflict(_) => "conflict",
            Self::Refused { .. } => "refused",
        }
    }
}

/// What a saved policy edit reaches, stated on every save because an operator
/// reading a fresh page has no other way to know it.
pub const REACH: &str = "This governs the next crossing and nothing already recorded; a mediated \
                         session already running keeps the policy it loaded and reads this one \
                         when it next starts.";

/// Write one edit to `<home>/policy.json`.
///
/// `revision` is the token the form was rendered from; `attempt` is the edit
/// in words, for the conflict and refusal notices; `edit` builds the
/// candidate from the document as it is on disk now.
pub fn write_policy(
    home: &Path,
    revision: &str,
    attempt: &str,
    edit: impl FnOnce(&PolicyDocument) -> Result<Candidate, String>,
) -> Outcome {
    let _lock = match PolicyDocument::lock(home) {
        Ok(lock) => lock,
        Err(LockRefused::Busy(reason)) => {
            return Outcome::Refused {
                status: 409,
                notice: format!("Not saved: {reason}."),
            };
        }
        Err(LockRefused::Failed(reason)) => {
            return Outcome::Refused {
                status: 500,
                notice: format!("Not saved: {reason}."),
            };
        }
    };
    let document = match PolicyDocument::read(home) {
        Ok(document) => document,
        Err(error) => {
            return Outcome::Refused {
                status: 500,
                notice: format!(
                    "Not saved: the policy file did not load, so there is no declaration to \
                     edit. {error}"
                ),
            };
        }
    };
    if !document.declared() {
        return Outcome::Refused {
            status: 409,
            notice: format!(
                "Not saved: no policy is declared at {}, and the console never creates one. \
                 Declare a policy there first.",
                home.join("policy.json").display()
            ),
        };
    }
    if document.revision() != revision {
        return Outcome::Conflict(format!(
            "Not saved: {attempt} was edited against revision {} of the policy file, and the \
             file is now revision {}. Another editor or process changed it since this page was \
             rendered. The page below shows the current declaration; repeat the edit against it \
             if it still applies.",
            short(revision),
            short(document.revision())
        ));
    }
    let candidate = match edit(&document) {
        Ok(candidate) => candidate,
        Err(reason) => {
            return Outcome::Refused {
                status: 400,
                notice: format!("Not saved: {reason}."),
            };
        }
    };
    if !candidate.changed {
        return Outcome::Unchanged(format!("{} Nothing was written.", candidate.notice));
    }
    match PolicyDocument::save(home, &candidate.file) {
        Ok(()) => Outcome::Saved(format!("{} {REACH}", candidate.notice)),
        Err(reason) => Outcome::Refused {
            status: 400,
            notice: format!(
                "Not saved: the runtime's policy loader refused the result, so the file was left \
                 as it was. {reason}"
            ),
        },
    }
}

/// The mode dial: one scope's mode, or the top-level mode for `None`.
pub fn set_mode(home: &Path, revision: &str, scope: Option<&str>, mode: PolicyMode) -> Outcome {
    let named = scope_name(scope);
    let attempt = format!("setting {named} to {}", mode_word(mode));
    write_policy(home, revision, &attempt, |document| {
        let top_level = document.file().policy_mode;
        let file = document.with_mode(scope, mode)?;
        let declared = match scope {
            Some(matcher) => document
                .scopes()
                .iter()
                .find(|declared| declared.matcher == matcher)
                .and_then(|declared| declared.policy_mode),
            None => Some(document.file().policy_mode),
        };
        let changed = declared != Some(mode);
        let notice = if changed {
            let pinned = if scope.is_some() && declared.is_none() {
                format!(
                    " It was following the top-level mode ({}) and now states its own.",
                    mode_word(top_level)
                )
            } else {
                String::new()
            };
            format!(
                "Saved: {named} now runs in {} mode.{pinned}",
                mode_word(mode)
            )
        } else {
            format!("{named} already runs in {} mode.", mode_word(mode))
        };
        Ok(Candidate {
            file,
            changed,
            notice,
        })
    })
}

/// A denied host added to one scope's denied-host set, or the top-level set
/// for `None`. Says when the scope stops inheriting.
pub fn deny_host(home: &Path, revision: &str, scope: Option<&str>, host: &str) -> Outcome {
    let named = scope_name(scope);
    let attempt = format!("blocking {host:?} in {named}");
    write_policy(home, revision, &attempt, |document| {
        let inherited = document.denies_by_inheritance(scope);
        let (file, changed) = document.with_denied_host(scope, host)?;
        let normalised = commonmeasure_runtime::policy::normalised_host(host);
        let notice = if changed {
            let stopped = if inherited {
                " This scope was inheriting the top-level constraints; it keeps them as its own \
                 list from this save on, and later top-level edits no longer reach it."
            } else {
                ""
            };
            format!("Saved: {named} now denies host {normalised}.{stopped}")
        } else {
            format!("{named} already denies host {normalised}.")
        };
        Ok(Candidate {
            file,
            changed,
            notice,
        })
    })
}

/// The attribution rules, replaced whole in the order given. Attribution is
/// a read-time projection: saving it changes what recorded work is reported
/// under and changes no evidence and no enforcement. Absent is a revision
/// like any other, so a first save creates the file and a save against a file
/// that appeared meanwhile is refused.
pub fn set_attribution(home: &Path, revision: &str, pairs: &[(String, String)]) -> Outcome {
    let _lock = match Attribution::lock(home) {
        Ok(lock) => lock,
        Err(LockRefused::Busy(reason)) => {
            return Outcome::Refused {
                status: 409,
                notice: format!("Not saved: {reason}."),
            };
        }
        Err(LockRefused::Failed(reason)) => {
            return Outcome::Refused {
                status: 500,
                notice: format!("Not saved: {reason}."),
            };
        }
    };
    let snapshot = match Attribution::read(home) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Outcome::Refused {
                status: 500,
                notice: format!("Not saved: the attribution rules could not be read. {error:#}"),
            };
        }
    };
    if snapshot.revision != revision {
        return Outcome::Conflict(format!(
            "Not saved: the attribution rules were edited against revision {} and the file is \
             now revision {}. Another editor or process changed it since this page was rendered. \
             The page below shows the current rules; repeat the edit against them if it still \
             applies.",
            short(revision),
            short(&snapshot.revision)
        ));
    }
    if let Ok(current) = &snapshot.rules
        && current.pairs() == pairs
    {
        return Outcome::Unchanged(
            "The attribution rules already read this way. Nothing was written.".to_owned(),
        );
    }
    match Attribution::save(home, pairs) {
        Ok(()) => Outcome::Saved(format!(
            "Saved: {} attribution rule{}. Every recorded session is reported under these rules \
             from the next read; no evidence changed and nothing is enforced by them.",
            pairs.len(),
            if pairs.len() == 1 { "" } else { "s" }
        )),
        Err(error) => Outcome::Refused {
            status: 400,
            notice: format!("Not saved: {error:#}."),
        },
    }
}

/// The draft an operator typed into the rule form, as the vocabulary reads it.
pub fn access_action(action: &str, licence: Option<&str>) -> Result<AccessAction, String> {
    match action {
        "allow" => Ok(AccessAction::Allow),
        "refuse" => Ok(AccessAction::Refuse),
        "require_mediation" => Ok(AccessAction::RequireMediation),
        "require_licence" => {
            let licence = licence.map(str::trim).unwrap_or_default();
            if licence.is_empty() {
                return Err("a require_licence rule must name the licence it requires".to_owned());
            }
            Ok(AccessAction::RequireLicence {
                licence: licence.to_owned(),
            })
        }
        other => Err(format!(
            "{other:?} is not an access rule action; the actions are allow, refuse, \
             require_licence and require_mediation"
        )),
    }
}

/// The host pattern an operator typed, validated as the loader validates it.
pub fn host_pattern(host: &str) -> Result<HostPattern, String> {
    HostPattern::parse(host)
}

/// The mode word an operator posted, as the vocabulary reads it.
pub fn mode_of(word: &str) -> Result<PolicyMode, String> {
    match word {
        "observe" => Ok(PolicyMode::Observe),
        "prefer" => Ok(PolicyMode::Prefer),
        "strict" => Ok(PolicyMode::Strict),
        other => Err(format!(
            "{other:?} is not a policy mode; the modes are observe, prefer and strict"
        )),
    }
}

pub fn mode_word(mode: PolicyMode) -> &'static str {
    match mode {
        PolicyMode::Observe => "observe",
        PolicyMode::Prefer => "prefer",
        PolicyMode::Strict => "strict",
    }
}

/// A form's `scope` field: empty names the top-level policy.
pub fn scope_field(value: Option<&str>) -> Option<&str> {
    value.filter(|scope| !scope.is_empty())
}

fn scope_name(scope: Option<&str>) -> String {
    match scope {
        Some(matcher) => format!("scope {matcher:?}"),
        None => "the top-level policy".to_owned(),
    }
}

/// The first characters of a revision token, enough to tell two apart on a
/// page without printing sixty-four hex digits twice.
fn short(revision: &str) -> &str {
    let end = revision
        .char_indices()
        .nth(12)
        .map(|(index, _)| index)
        .unwrap_or(revision.len());
    &revision[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_harness::declaration;

    const DECLARED: &str = r#"{
        "policy_mode": "observe",
        "constraints": [{"kind": "denied_source_host", "host": "tracker.example"}],
        "scopes": [
            {"match": "code/ozone", "engagement": "ozone", "policy_mode": "prefer"},
            {"match": "code/tessera"}
        ]}"#;

    fn home_with(policy: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), policy).unwrap();
        home
    }

    fn revision(home: &Path) -> String {
        PolicyDocument::read(home).unwrap().revision().to_owned()
    }

    #[test]
    fn a_mode_save_changes_the_mode_and_states_its_reach() {
        let home = home_with(DECLARED);
        let outcome = set_mode(
            home.path(),
            &revision(home.path()),
            Some("code/tessera"),
            PolicyMode::Strict,
        );
        assert_eq!(outcome.status(), 200, "{outcome:?}");
        assert!(outcome.notice().contains("now runs in strict mode"));
        assert!(
            outcome
                .notice()
                .contains("was following the top-level mode (observe)"),
            "a scope that inherited says it now states its own: {}",
            outcome.notice()
        );
        assert!(outcome.notice().contains(REACH));
        let reread = PolicyDocument::read(home.path()).unwrap();
        assert_eq!(
            reread.resolve(Some("/home/op/code/tessera")).mode(),
            PolicyMode::Strict
        );
        assert_eq!(
            reread.resolve(Some("/home/op/code/ozone")).mode(),
            PolicyMode::Prefer,
            "the other scope was carried through untouched"
        );
    }

    #[test]
    fn a_stale_revision_is_refused_with_both_versions_stated_and_nothing_written() {
        let home = home_with(DECLARED);
        let stale = revision(home.path());
        // The file changes underneath, as another console would change it.
        let edited = PolicyDocument::read(home.path())
            .unwrap()
            .with_mode(None, PolicyMode::Strict)
            .unwrap();
        PolicyDocument::save(home.path(), &edited).unwrap();
        let current = revision(home.path());

        let outcome = set_mode(home.path(), &stale, None, PolicyMode::Observe);
        assert_eq!(outcome.status(), 409);
        assert!(matches!(outcome, Outcome::Conflict(_)));
        assert!(outcome.notice().contains(short(&stale)));
        assert!(outcome.notice().contains(short(&current)));
        assert_eq!(
            revision(home.path()),
            current,
            "the refused save wrote nothing"
        );
    }

    #[test]
    fn no_declaration_is_never_created_by_a_write() {
        let home = tempfile::tempdir().unwrap();
        let outcome = set_mode(home.path(), declaration::ABSENT, None, PolicyMode::Strict);
        assert_eq!(outcome.status(), 409);
        assert!(outcome.notice().contains("never creates one"));
        assert!(!home.path().join("policy.json").exists());
    }

    #[test]
    fn an_unchanged_edit_writes_nothing_and_says_so() {
        let home = home_with(DECLARED);
        let before = revision(home.path());
        let outcome = set_mode(home.path(), &before, Some("code/ozone"), PolicyMode::Prefer);
        assert_eq!(
            outcome,
            Outcome::Unchanged(
                "scope \"code/ozone\" already runs in prefer mode. Nothing was written.".to_owned()
            )
        );
        assert_eq!(revision(home.path()), before);
    }

    #[test]
    fn a_denied_host_materialises_inheritance_and_the_notice_says_so() {
        let home = home_with(DECLARED);
        let outcome = deny_host(
            home.path(),
            &revision(home.path()),
            Some("code/tessera"),
            "Beacon.Example.",
        );
        assert_eq!(outcome.status(), 200, "{outcome:?}");
        assert!(outcome.notice().contains("now denies host beacon.example"));
        assert!(
            outcome
                .notice()
                .contains("was inheriting the top-level constraints")
        );
        let reread = PolicyDocument::read(home.path()).unwrap();
        let governed = reread.resolve(Some("/home/op/code/tessera"));
        assert!(
            governed
                .admit_host("https://beacon.example/")
                .gap()
                .is_some()
        );
        assert!(
            governed
                .admit_host("https://tracker.example/")
                .gap()
                .is_some(),
            "the inherited denial was kept"
        );

        let again = deny_host(
            home.path(),
            &revision(home.path()),
            Some("code/tessera"),
            "beacon.example",
        );
        assert!(matches!(again, Outcome::Unchanged(_)), "{again:?}");
        let top = deny_host(home.path(), &revision(home.path()), None, "");
        assert_eq!(top.status(), 400);
    }

    #[test]
    fn a_scope_the_declaration_lacks_is_refused_not_created() {
        let home = home_with(DECLARED);
        let outcome = deny_host(
            home.path(),
            &revision(home.path()),
            Some("code/invented"),
            "x.example",
        );
        assert_eq!(outcome.status(), 400);
        assert!(outcome.notice().contains("code/invented"));
        assert_eq!(PolicyDocument::read(home.path()).unwrap().scopes().len(), 2);
    }

    #[test]
    fn attribution_saves_in_order_and_conflicts_on_a_stale_revision() {
        let home = tempfile::tempdir().unwrap();
        let pairs = vec![
            ("code/ozone".to_owned(), "ozone".to_owned()),
            ("code".to_owned(), "personal".to_owned()),
        ];
        let outcome = set_attribution(home.path(), declaration::ABSENT, &pairs);
        assert_eq!(outcome.status(), 200, "{outcome:?}");
        assert!(outcome.notice().contains("2 attribution rules"));
        assert_eq!(Attribution::load(home.path()).unwrap().pairs(), pairs);

        let stale = set_attribution(home.path(), declaration::ABSENT, &pairs[..1]);
        assert_eq!(stale.status(), 409);
        assert!(stale.notice().contains("revision absent"));
        assert_eq!(Attribution::load(home.path()).unwrap().pairs(), pairs);

        let current = Attribution::revision(home.path()).unwrap();
        let same = set_attribution(home.path(), &current, &pairs);
        assert!(matches!(same, Outcome::Unchanged(_)));
        let invalid = set_attribution(home.path(), &current, &[("".to_owned(), "x".to_owned())]);
        assert_eq!(invalid.status(), 400);
    }

    #[test]
    fn the_form_words_are_read_as_the_vocabulary_and_nothing_else() {
        assert_eq!(mode_of("strict"), Ok(PolicyMode::Strict));
        assert!(mode_of("lenient").is_err());
        assert_eq!(access_action("refuse", None), Ok(AccessAction::Refuse));
        assert!(access_action("require_licence", Some(" ")).is_err());
        assert_eq!(
            access_action("require_licence", Some("rsl:p/1")),
            Ok(AccessAction::RequireLicence {
                licence: "rsl:p/1".into()
            })
        );
        assert!(access_action("escalate", None).is_err());
        assert!(host_pattern("a.*.example").is_err());
        assert_eq!(scope_field(Some("")), None);
        assert_eq!(scope_field(Some("code")), Some("code"));
    }
}
