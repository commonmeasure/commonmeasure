//! The dry run: what a draft policy would have done to the recorded history.
//!
//! A forecast takes the declaration as it stands and a candidate built from
//! it, resolves every recorded crossing's working directory through both with
//! the runtime's own loader, and puts the crossing through the runtime's own
//! admission check under each. The answer is the difference: which crossings
//! the draft would newly refuse, newly carry as a breach, or newly admit. No
//! second policy engine is consulted, so the forecast cannot promise a
//! refusal the runtime would not make.
//!
//! Grades stay apart. A witnessed crossing happened under this runtime's eye
//! and is the evidence a draft is really tested against; a reconstructed one
//! was read back from a transcript afterwards and may not be what entered the
//! model at all; a refused one never happened. Each is counted on its own
//! line and none is totalled with another.
//!
//! The forecast is a read. It saves nothing, and it says so.

use std::collections::BTreeMap;

use commonmeasure_harness::policy::{PolicyDocument, PolicyFile, SessionPolicy};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_types::ContextEnvelope;
use serde_json::{Value, json};

use crate::store::{CrossingFact, CrossingFacts};

/// How many example crossings each changed outcome lists, per grade. Enough
/// to recognise what a rule catches; the count carries the rest.
const EXAMPLES: usize = 8;

/// What the draft would have done, over every recorded crossing, with the
/// three grades kept separate. Served as JSON and rendered from the same
/// value, so the two cannot disagree.
///
/// Every crossing is resolved under the principal this console process
/// authenticates as, because a document resolves for the process that read
/// it. A mediated crossing recorded under another principal is therefore
/// judged under this principal's overlay and not its own; the forecast
/// states the principal it judged as and counts those crossings, so the
/// reader knows which lines that caveat covers.
pub fn forecast(
    document: &PolicyDocument,
    candidate: PolicyFile,
    recorded: &CrossingFacts,
) -> Value {
    let facts = &recorded.facts;
    let draft = document.with_file(candidate);
    let judged_as = document.resolve(None);
    let mut other_principal = 0u64;
    // Resolution clones the declaration, so each distinct directory is
    // resolved once under each document rather than once per crossing.
    let mut resolved: BTreeMap<Option<String>, (SessionPolicy, SessionPolicy)> = BTreeMap::new();
    let mut grades: BTreeMap<&'static str, Outcomes> = BTreeMap::new();
    for grade in ["witnessed", "reconstructed", "refused"] {
        grades.insert(grade, Outcomes::default());
    }
    for fact in facts {
        if fact
            .principal
            .as_deref()
            .is_some_and(|recorded| recorded != judged_as.principal())
        {
            other_principal += 1;
        }
        let (current, drafted) = resolved.entry(fact.cwd.clone()).or_insert_with(|| {
            (
                document.resolve(fact.cwd.as_deref()),
                draft.resolve(fact.cwd.as_deref()),
            )
        });
        let envelope = envelope_of(fact);
        let before = current.admit(&envelope);
        let after = drafted.admit(&envelope);
        let outcomes = grades.entry(fact.grade).or_default();
        outcomes.crossings += 1;
        let change = match (standing(&before), standing(&after)) {
            (from, to) if from == to => Change::Unchanged,
            (_, Standing::Refused) => Change::Refused,
            (_, Standing::Breach) => Change::Breach,
            (_, Standing::Admitted) => Change::Admitted,
        };
        outcomes.note(change, fact, drafted.scope(), &after);
    }
    json!({
        "saved": false,
        "crossings": facts.len(),
        "not_evaluated": recorded.not_evaluated,
        "principal": {
            "name": judged_as.principal(),
            "basis": judged_as.authentication_basis().as_str(),
            "other_principal_crossings": other_principal,
        },
        "grades": grades
            .into_iter()
            .map(|(grade, outcomes)| (grade.to_owned(), outcomes.into_json()))
            .collect::<serde_json::Map<String, Value>>(),
    })
}

/// The recorded crossing as the envelope admission judges: the URL and host
/// as recorded, the licence the record carries, and an empty text so the
/// no-excerpt refusal does not fire for a crossing whose bytes were never
/// kept. The same shape the mediated path checks a bare URL with.
fn envelope_of(fact: &CrossingFact) -> ContextEnvelope {
    ContextEnvelope {
        source_url: fact.url.clone(),
        host: fact.host.clone(),
        title: None,
        text: Some(String::new()),
        content_hash: None,
        licence: fact.licence.clone(),
        declared_date: None,
        native_metadata: json!({}),
        retrieval_rank: 1,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Admitted,
    Breach,
    Refused,
}

fn standing(ruling: &Ruling) -> Standing {
    match ruling {
        Ruling::Allowed => Standing::Admitted,
        Ruling::AllowedWithBreach { .. } => Standing::Breach,
        Ruling::Refused { .. } => Standing::Refused,
    }
}

#[derive(Clone, Copy)]
enum Change {
    Unchanged,
    Refused,
    Breach,
    Admitted,
}

/// One grade's tally: how many crossings the draft would treat differently,
/// and which, with the reason the runtime gave.
#[derive(Default)]
struct Outcomes {
    crossings: u64,
    unchanged: u64,
    newly_refused: u64,
    newly_breached: u64,
    newly_admitted: u64,
    examples: Vec<Value>,
}

impl Outcomes {
    fn note(&mut self, change: Change, fact: &CrossingFact, scope: Option<&str>, after: &Ruling) {
        let outcome = match change {
            Change::Unchanged => {
                self.unchanged += 1;
                return;
            }
            Change::Refused => {
                self.newly_refused += 1;
                "refused"
            }
            Change::Breach => {
                self.newly_breached += 1;
                "breach"
            }
            Change::Admitted => {
                self.newly_admitted += 1;
                "admitted"
            }
        };
        if self.examples.len() < EXAMPLES {
            self.examples.push(json!({
                "url": fact.url,
                "scope": scope,
                "outcome": outcome,
                "reason": after.reason(),
            }));
        }
    }

    fn into_json(self) -> Value {
        json!({
            "crossings": self.crossings,
            "unchanged": self.unchanged,
            "newly_refused": self.newly_refused,
            "newly_breached": self.newly_breached,
            "newly_admitted": self.newly_admitted,
            "examples": self.examples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_types::{AccessAction, HostPattern, LicenceState};

    fn fact(grade: &'static str, url: &str, cwd: Option<&str>) -> CrossingFact {
        CrossingFact {
            grade,
            url: url.to_owned(),
            host: commonmeasure_harness::grounding::host_of(url),
            cwd: cwd.map(str::to_owned),
            principal: None,
            licence: LicenceState::Unknown,
        }
    }

    fn recorded(facts: Vec<CrossingFact>) -> CrossingFacts {
        CrossingFacts {
            facts,
            not_evaluated: 0,
        }
    }

    fn document(policy: &str) -> (tempfile::TempDir, PolicyDocument) {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), policy).unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        (home, document)
    }

    /// A draft refusal in a strict scope is forecast over that scope's
    /// recorded work and nobody else's, with each grade on its own line and
    /// the runtime's own reason on each example.
    #[test]
    fn a_draft_rule_is_forecast_per_grade_over_the_scope_it_governs() {
        let (_home, document) = document(
            r#"{"policy_mode":"observe","scopes":[
                {"match":"code/client","policy_mode":"strict"},
                {"match":"code/personal"}]}"#,
        );
        let candidate = document
            .with_access_rule(
                Some("code/client"),
                HostPattern::parse("*.tracker.example").unwrap(),
                AccessAction::Refuse,
            )
            .unwrap();
        let facts = [
            fact(
                "witnessed",
                "https://cdn.tracker.example/a",
                Some("/home/op/code/client/api"),
            ),
            fact(
                "witnessed",
                "https://www.gov.uk/x",
                Some("/home/op/code/client/api"),
            ),
            fact(
                "reconstructed",
                "https://cdn.tracker.example/b",
                Some("/home/op/code/client/api"),
            ),
            fact(
                "witnessed",
                "https://cdn.tracker.example/c",
                Some("/home/op/code/personal"),
            ),
            fact("witnessed", "https://cdn.tracker.example/d", None),
        ];
        let forecast = forecast(&document, candidate, &recorded(facts.to_vec()));
        assert_eq!(forecast["saved"], false);
        assert_eq!(forecast["crossings"], 5);
        let witnessed = &forecast["grades"]["witnessed"];
        assert_eq!(witnessed["crossings"], 4);
        assert_eq!(witnessed["newly_refused"], 1, "{witnessed}");
        assert_eq!(witnessed["unchanged"], 3);
        assert_eq!(
            witnessed["examples"][0]["url"],
            "https://cdn.tracker.example/a"
        );
        assert_eq!(witnessed["examples"][0]["scope"], "code/client");
        assert!(
            witnessed["examples"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("*.tracker.example"),
            "the runtime's reason names the rule: {witnessed}"
        );
        let reconstructed = &forecast["grades"]["reconstructed"];
        assert_eq!(reconstructed["crossings"], 1);
        assert_eq!(reconstructed["newly_refused"], 1);
        assert_eq!(forecast["grades"]["refused"]["crossings"], 0);
    }

    /// Under observe the same draft is carried as a breach, and the forecast
    /// says breach, not refused: the mode the scope runs under is part of
    /// what would have happened.
    #[test]
    fn a_draft_under_observe_forecasts_a_breach_not_a_refusal() {
        let (_home, document) = document(r#"{"policy_mode":"observe"}"#);
        let (file, _) = document.with_denied_host(None, "tracker.example").unwrap();
        let facts = [
            fact("witnessed", "https://tracker.example/a", None),
            fact("refused", "https://tracker.example/b", Some("/anywhere")),
        ];
        let forecast = forecast(&document, file, &recorded(facts.to_vec()));
        let witnessed = &forecast["grades"]["witnessed"];
        assert_eq!(witnessed["newly_breached"], 1);
        assert_eq!(witnessed["newly_refused"], 0);
        assert_eq!(witnessed["examples"][0]["outcome"], "breach");
        let refused = &forecast["grades"]["refused"];
        assert_eq!(
            refused["crossings"], 1,
            "a refused crossing is its own grade"
        );
        assert_eq!(refused["newly_breached"], 1);
    }

    /// A draft that widens what may happen is forecast too: crossings the
    /// standing policy refuses and the draft would admit are counted as
    /// newly admitted, so the operator sees what a relaxation lets through.
    #[test]
    fn a_relaxing_draft_forecasts_what_it_newly_admits() {
        let (_home, document) = document(
            r#"{"policy_mode":"strict","constraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
        );
        let candidate = document
            .with_access_rule(
                None,
                HostPattern::parse("docs.rs").unwrap(),
                AccessAction::Allow,
            )
            .unwrap();
        let facts = [
            fact("witnessed", "https://docs.rs/serde", None),
            fact("witnessed", "https://example.com/x", None),
        ];
        let forecast = forecast(&document, candidate, &recorded(facts.to_vec()));
        let witnessed = &forecast["grades"]["witnessed"];
        assert_eq!(witnessed["newly_admitted"], 1);
        assert_eq!(witnessed["unchanged"], 1);
        assert_eq!(witnessed["examples"][0]["outcome"], "admitted");
    }

    /// The licence a record carries is what a licence rule is judged on: a
    /// crossing declared under the required licence is unchanged, and one
    /// with no licence is newly refused.
    #[test]
    fn a_licence_rule_is_forecast_on_the_recorded_licence() {
        let (_home, document) = document(r#"{"policy_mode":"strict"}"#);
        let candidate = document
            .with_access_rule(
                None,
                HostPattern::parse("publisher.example").unwrap(),
                AccessAction::RequireLicence {
                    licence: "rsl:publisher/2026".into(),
                },
            )
            .unwrap();
        let mut licensed = fact("witnessed", "https://publisher.example/a", None);
        licensed.licence = LicenceState::Declared {
            reference: "rsl:publisher/2026".into(),
        };
        let facts = [
            licensed,
            fact("witnessed", "https://publisher.example/b", None),
        ];
        let forecast = forecast(&document, candidate, &recorded(facts.to_vec()));
        let witnessed = &forecast["grades"]["witnessed"];
        assert_eq!(witnessed["unchanged"], 1);
        assert_eq!(witnessed["newly_refused"], 1);
        assert_eq!(
            witnessed["examples"][0]["url"],
            "https://publisher.example/b"
        );
    }

    /// The forecast states the principal it judged as and counts the
    /// crossings recorded under another principal, which it judged under
    /// this one's overlay; records it could not judge are counted too.
    #[cfg(unix)]
    #[test]
    fn a_forecast_states_its_principal_and_what_it_could_not_judge() {
        use std::os::unix::fs::MetadataExt;
        let home = tempfile::tempdir().unwrap();
        // A file this process creates is owned by its effective user, which
        // is the subject the runtime authenticates a principal from.
        let probe = home.path().join("probe");
        std::fs::write(&probe, b"").unwrap();
        let uid = std::fs::metadata(&probe).unwrap().uid();
        std::fs::write(
            home.path().join("policy.json"),
            format!(
                r#"{{"policy_mode":"strict","principals":[{{"principal":"alice","os_user":{uid}}}]}}"#
            ),
        )
        .unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let candidate = document
            .with_access_rule(None, HostPattern::parse("*").unwrap(), AccessAction::Refuse)
            .unwrap();
        let mut bobs = fact("witnessed", "https://a.example/x", None);
        bobs.principal = Some("bob".to_owned());
        let mut alices = fact("witnessed", "https://a.example/y", None);
        alices.principal = Some("alice".to_owned());
        let recorded = CrossingFacts {
            facts: vec![bobs, alices, fact("witnessed", "https://a.example/z", None)],
            not_evaluated: 2,
        };
        let forecast = forecast(&document, candidate, &recorded);
        assert_eq!(forecast["principal"]["name"], "alice");
        assert_eq!(forecast["principal"]["basis"], "os_user");
        assert_eq!(forecast["principal"]["other_principal_crossings"], 1);
        assert_eq!(forecast["not_evaluated"], 2);
        assert_eq!(forecast["crossings"], 3);
        assert_eq!(forecast["grades"]["witnessed"]["newly_refused"], 3);
    }
}
