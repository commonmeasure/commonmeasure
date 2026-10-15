//! The display rule, both modes and the counts vector, over releases signed
//! here and read through the release verifier.
//!
//! The model stage is a fixed double: it establishes how the rule reads a
//! model's probabilities, not that any model stage integrates.

use base64::Engine as _;
use commonmeasure_types::PolicyMode;
use commonmeasure_types::release::{KeptSpan, Passage, SCHEME, Sidecar, Signed, signed_message};
use ed25519_dalek::{Signer as _, SigningKey};

use super::super::release::{self, Outcome as ReadOutcome};
use super::*;

const URL: &str = "https://source.example/harbour";

const PARAGRAPHS: [&str; 3] = [
    "The council approved the harbour plan on Tuesday after a long debate about parking and the cost of the new sea wall.",
    "Before the vote the mayor read a statement. \"We accept the inspector's findings in full and we will publish the costs of the scheme before the end of the year,\" said Jane Okafor. The council leader, Tom Reilly, disagreed.",
    "The harbour plan sets out a new sea wall, a wider slipway and two hundred parking spaces on the old timber yard, with work due to start in the spring.",
];

/// A release as the demo's stand-in source signs one: the paragraphs joined
/// by blank lines, the lead kept whole, and every quotation of at least six
/// words kept whole.
fn signed(mode: &str, paragraphs: &[&str]) -> Release {
    let mut text = String::new();
    let mut passages = Vec::new();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            text.push_str("\n\n");
        }
        let start = text.len();
        text.push_str(paragraph);
        passages.push(Passage {
            id: format!("p{index}"),
            start,
            end: text.len(),
        });
    }
    let mut markers = Vec::new();
    if let Some(lead) = passages.first() {
        markers.push(KeptSpan {
            start: lead.start,
            end: lead.end,
            label: "lead".into(),
            passage: lead.id.clone(),
        });
    }
    for passage in &passages {
        let body = &text[passage.start..passage.end];
        for quote in attribution::quotations(body) {
            if words(&quote.text).len() < 6 {
                continue;
            }
            if let Some(at) = body.find(&quote.text) {
                markers.push(KeptSpan {
                    start: passage.start + at,
                    end: passage.start + at + quote.text.len(),
                    label: "quotation".into(),
                    passage: passage.id.clone(),
                });
            }
        }
    }
    let signed = Signed {
        source: "a test source".into(),
        title: "Test".into(),
        revid: 1,
        url: URL.into(),
        licence: "CC BY-SA 4.0".into(),
        attribution: "Test, by its authors".into(),
        terms: vec!["Show a kept passage whole.".into()],
        display_mode: mode.into(),
        text_sha256: format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
        ),
        passages,
        markers,
        released_at: "2026-10-13T00:00:00Z".into(),
    };
    sign(text, signed)
}

fn sign(text: String, signed: Signed) -> Release {
    let key = SigningKey::from_bytes(&[7; 32]);
    let engine = base64::engine::general_purpose::STANDARD;
    Release {
        text,
        sidecar: Sidecar {
            signature: engine.encode(key.sign(&signed_message(&signed)).to_bytes()),
            signer_public_key: engine.encode(key.verifying_key().as_bytes()),
            signature_scheme: SCHEME.into(),
            signed,
        },
    }
}

/// The release as the verifier reads it, in `observe` so that a release
/// whose signature fails still reads.
fn read(release: &Release) -> Read {
    let body = serde_json::to_vec(release).unwrap();
    match release::invoke(PolicyMode::Observe, URL, &body, None).1 {
        ReadOutcome::Read(read) => *read,
        ReadOutcome::Unreadable { reason } => panic!("unreadable: {reason}"),
    }
}

fn release_in(mode: &str) -> Read {
    read(&signed(mode, &PARAGRAPHS))
}

/// A model stage that answers every case with fixed probabilities and keeps
/// the cases it was given. `contradicts: None` is a model with no
/// contradiction class; `error` makes every item unanswered.
struct FixedModel {
    supported: f64,
    contradicts: Option<f64>,
    error: bool,
    seen: Vec<Case>,
}

impl FixedModel {
    fn at(supported: f64) -> Self {
        Self::with(supported, Some(0.01))
    }

    fn with(supported: f64, contradicts: Option<f64>) -> Self {
        Self {
            supported,
            contradicts,
            error: false,
            seen: Vec::new(),
        }
    }

    fn failing() -> Self {
        Self {
            supported: 0.0,
            contradicts: Some(0.0),
            error: true,
            seen: Vec::new(),
        }
    }
}

impl ModelChecker for FixedModel {
    fn name(&self) -> String {
        "fixed-model".into()
    }

    fn manifest(&self) -> Value {
        json!({ "name": "fixed-model", "probability": self.supported })
    }

    fn questions(&self) -> Vec<Question> {
        match self.contradicts {
            Some(_) => vec![Question::Supported, Question::Contradicts],
            None => vec![Question::Supported],
        }
    }

    fn check_many(&mut self, cases: &[Case]) -> Vec<Vec<Answer>> {
        cases
            .iter()
            .map(|case| {
                self.seen.push(case.clone());
                let answer = |question, p: Option<f64>| Answer {
                    question,
                    label: match p {
                        Some(p) if p >= 0.5 => Label::Yes,
                        Some(_) => Label::No,
                        None => Label::NotApplicable,
                    },
                    probability: p,
                    basis: "fixed-model".into(),
                    reason: if p.is_some() {
                        "a fixed answer".into()
                    } else {
                        "the checker could not answer this item: mocked failure".into()
                    },
                    detail: Value::Null,
                };
                let mut out = vec![answer(
                    Question::Supported,
                    (!self.error).then_some(self.supported),
                )];
                if let Some(c) = self.contradicts {
                    out.push(answer(Question::Contradicts, (!self.error).then_some(c)));
                }
                out
            })
            .collect()
    }
}

fn from_source(text: &str, passages: &[&str]) -> Item {
    Item::FromSource {
        text: text.into(),
        passages: passages.iter().map(|p| p.to_string()).collect(),
    }
}

fn own(text: &str) -> Item {
    Item::Own { text: text.into() }
}

fn quote_of(passage: &str, sentence: Option<usize>) -> Item {
    Item::Quote {
        passage: passage.into(),
        sentence,
    }
}

/// Render with the fixed model at the demo's thresholds, and the record.
fn render_items(mode: &str, items: Vec<Item>, model: Option<&mut FixedModel>) -> (Rendered, Value) {
    let model = model.map(|checker| Model {
        checker,
        threshold: 0.816,
        temperature: 1.0,
        contradiction_threshold: 0.9,
    });
    let (invocation, rendered) = invoke_with(&release_in(mode), URL, &Reply { items }, model);
    (
        rendered.expect("a verified release renders"),
        invocation.to_value(),
    )
}

/// Render with no model stage, as the processor runs in this build.
fn deterministic(mode: &str, items: Vec<Item>) -> Rendered {
    let (_, rendered) = invoke(&release_in(mode), URL, &Reply { items });
    rendered.expect("a verified release renders")
}

fn one(mode: &str, item: Item, model: &mut FixedModel) -> Block {
    render_items(mode, vec![item], Some(model))
        .0
        .blocks
        .remove(0)
}

const PARAPHRASE: &str = "Work on the sea wall starts in the spring.";
const CONTRADICTION: &str = "The council rejected the harbour plan on Tuesday.";
/// Twelve words of the unmarked third paragraph, then an invented clause.
const TWELVE_WORDS_AND_AN_INVENTED_CLAUSE: &str = "The harbour plan sets out a new sea wall, a wider slipway and was drawn up in secret by a company the mayor owns.";
const MISATTRIBUTED: &str = "Tom Reilly said: \"We accept the inspector's findings in full and we will publish the costs of the scheme before the end of the year,\"";
const FRAGMENT: &str = "The mayor said \"we accept the inspector's findings in full\".";
const VERBATIM: &str = "The council leader, Tom Reilly, disagreed.";

fn six_word_run_yes(block: &Block) -> bool {
    block.answers.iter().any(|a| {
        a.question == Question::Supported
            && a.basis.starts_with("fidelity-verifier")
            && a.label == Label::Yes
    })
}

// --- the processor ---

#[test]
fn the_manifest_declares_no_ambient_authority_and_a_deterministic_rule() {
    let manifest = manifest();
    assert_eq!(manifest.name, "integrity-checker");
    assert_eq!(manifest.stage, Stage::Verify);
    assert_eq!(manifest.determinism, Determinism::Deterministic);
    let permissions = manifest.permissions;
    assert!(!permissions.network && !permissions.filesystem);
    assert!(!permissions.model && !permissions.credentials);
    assert!(permissions.raw_content);
    assert_eq!(
        manifest.configuration_digest,
        sha256_digest(RULES.as_bytes())
    );
    assert!(RULES.contains("withheld_kept") && RULES.contains("strict otherwise"));
    assert!(
        super::super::installed()
            .iter()
            .any(|installed| installed.name == NAME),
        "built in, so installed"
    );
}

#[test]
fn the_record_carries_raw_answers_per_item_the_outcomes_and_the_counts_vector() {
    let (invocation, rendered) = invoke(
        &release_in(LABELLED),
        URL,
        &Reply {
            items: vec![
                quote_of("p2", None),
                from_source(VERBATIM, &["p1"]),
                own("An own remark."),
            ],
        },
    );
    let rendered = rendered.unwrap();
    let record = invocation.to_value();
    assert_eq!(record["processor"]["name"], NAME);
    assert_eq!(record["stage"], "verify");
    assert_eq!(record["decision"], "abstain");
    assert_eq!(record["assurance"], "observed");
    assert_eq!(record["detail"]["model"], Value::Null);
    assert_eq!(record["detail"]["display_mode"], "labelled");
    assert_eq!(record["detail"]["count_names"], json!(Counts::NAMES));
    assert_eq!(
        record["detail"]["counts_vector"],
        json!([1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0])
    );
    let items = record["detail"]["items"].as_array().unwrap();
    let outcomes: Vec<&str> = items
        .iter()
        .map(|i| i["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["quote_shown", "shown_confirmed", "shown_own"]);
    let answers = items[1]["answers"].as_array().unwrap();
    let bases: Vec<&str> = answers
        .iter()
        .map(|a| a["basis"].as_str().unwrap())
        .collect();
    assert_eq!(
        bases,
        [
            "fidelity-verifier/1",
            "verbatim-whole/1",
            "attribution-lookup/1",
            "keep-intact-containment/1"
        ]
    );
    assert!(answers.iter().all(|a| a.get("label").is_some()));
    assert_eq!(answers[1]["probability"], 1.0);
    assert!(
        record.to_string().find("score").is_none(),
        "nothing combines the verdicts"
    );
    assert_eq!(
        record["inputs"][0]["content_hash"],
        release_in(LABELLED).content_hash
    );
    assert_eq!(
        record["inputs"][1]["content_hash"],
        digest_of(&release_in(LABELLED).release.sidecar)
    );
    let bytes = record.to_string();
    for words in [VERBATIM, "harbour plan", "Okafor", "An own remark"] {
        assert!(!bytes.contains(words), "{words:?} is in the record");
    }
    assert_eq!(
        record["outputs"][0]["content_hash"],
        digest_of(&rendered.blocks)
    );
    assert!(
        record["blind_spots"][0]
            .as_str()
            .unwrap()
            .starts_with("No model stage runs")
    );
}

#[test]
fn a_release_whose_signature_did_not_verify_is_not_displayed() {
    let mut release = signed(LABELLED, &PARAGRAPHS);
    release.text = release.text.replace("Tuesday", "TUESDAY");
    let read = read(&release);
    assert_eq!(read.record.verification, VerificationState::Failed);
    let (invocation, rendered) = invoke(
        &read,
        URL,
        &Reply {
            items: vec![from_source(
                "The council approved the harbour plan on TUESDAY.",
                &[],
            )],
        },
    );
    assert!(rendered.is_none());
    let record = invocation.to_value();
    assert_eq!(record["decision"], "abstain");
    assert_eq!(record["detail"]["displayed"], false);
    assert_eq!(record["gaps"][0]["reason"], "evidence_missing");
}

#[test]
fn a_display_mode_the_rule_does_not_know_reads_as_strict() {
    let r = deterministic("verbose", vec![own("An own remark.")]);
    assert_eq!(r.display_mode, STRICT);
    assert_eq!(r.blocks[0].outcome, Outcome::WithheldUnconfirmed);
}

#[test]
fn a_release_with_no_passages_withholds_with_nothing_in_place() {
    let r = deterministic_over(&signed(STRICT, &[]), vec![own("An own remark.")]);
    let b = &r.blocks[0];
    assert_eq!(b.outcome, Outcome::WithheldUnconfirmed);
    assert_eq!(b.passage, None);
    assert!(b.text.is_empty());
    assert!(
        b.reason
            .ends_with("the release has no passage to show instead"),
        "{}",
        b.reason
    );
}

fn deterministic_over(release: &Release, items: Vec<Item>) -> Rendered {
    invoke(&read(release), URL, &Reply { items }).1.unwrap()
}

// --- the cited passages ---

#[test]
fn the_model_reads_the_cited_passages_and_their_neighbours() {
    let read = release_in(LABELLED);
    let rel = &read.release;
    let c = cited(rel, &["p0".into()]);
    assert_eq!(c.valid, ["p0"]);
    assert_eq!(c.checked_against, ["p0", "p1"]);
    let (text, markers) = c.passage.unwrap();
    assert!(text.starts_with("The council approved") && text.contains("\n\nBefore the vote"));
    assert!(!text.contains("timber yard"), "p2 is not a neighbour of p0");
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].start, 0);
    assert_eq!(
        &text[markers[1].start..markers[1].end],
        "We accept the inspector's findings in full and we will publish the costs of the scheme before the end of the year,"
    );
    let c = cited(rel, &["p2".into(), "p9".into(), "p2".into()]);
    assert_eq!(c.valid, ["p2"]);
    assert_eq!(c.invalid, ["p9"]);
    assert_eq!(c.checked_against, ["p1", "p2"]);
    let c = cited(rel, &["p9".into(), "x".into()]);
    assert_eq!(c.valid, Vec::<String>::new());
    assert_eq!(c.invalid, ["p9", "x"]);
    assert_eq!(c.passage, None);
    assert_eq!(cited(rel, &[]).passage, None);

    // An own sentence is checked against what the reply cites, or the lead.
    let reply = Reply {
        items: vec![from_source("x", &["p2"]), own("y")],
    };
    assert_eq!(reply_passages(rel, &reply).checked_against, ["p1", "p2"]);
    let reply = Reply {
        items: vec![own("y")],
    };
    assert_eq!(reply_passages(rel, &reply).checked_against, ["p0", "p1"]);
    // A quoted passage is context too; a quote of a missing one is not.
    let reply = Reply {
        items: vec![quote_of("p2", None), quote_of("p9", None), own("y")],
    };
    let c = reply_passages(rel, &reply);
    assert_eq!(c.valid, ["p2"]);
    assert!(c.invalid.is_empty(), "{:?}", c.invalid);
    assert_eq!(c.checked_against, ["p1", "p2"]);
}

// --- labelled mode, row by row ---

#[test]
fn a_confirmed_from_source_sentence_is_shown_plain_with_its_passages() {
    let mut model = FixedModel::at(0.95);
    let (r, record) = render_items(
        LABELLED,
        vec![from_source(PARAPHRASE, &["p2"])],
        Some(&mut model),
    );
    let b = &r.blocks[0];
    assert_eq!(b.outcome, Outcome::ShownConfirmed, "{b:?}");
    assert_eq!(b.label, "confirmed from the article");
    assert_eq!(b.text, PARAPHRASE);
    assert_eq!(b.sources.len(), 1);
    assert_eq!(b.sources[0].passage, "p2");
    assert!(b.sources[0].text.starts_with("The harbour plan sets out"));
    assert_eq!(model.seen.len(), 1);
    assert!(
        model.seen[0].passage.starts_with("Before the vote"),
        "p1 and p2"
    );
    assert!(!model.seen[0].passage.contains("The council approved"));
    let answer = b.answers.iter().find(|a| a.basis == "fixed-model").unwrap();
    assert_eq!(answer.detail["cited_passages"], json!(["p2"]));
    assert_eq!(answer.detail["checked_against"], json!(["p1", "p2"]));
    assert_eq!(answer.detail["p_raw"], json!(0.95));
    assert_eq!(r.counts.shown_confirmed, 1);
    assert_eq!(r.display_mode, "labelled");
    assert_eq!(record["detail"]["model"]["threshold"], json!(0.816));
}

#[test]
fn a_verbatim_sentence_is_confirmed_without_a_model() {
    let r = deterministic(
        LABELLED,
        vec![from_source(VERBATIM, &[]), from_source(PARAPHRASE, &["p2"])],
    );
    assert_eq!(
        r.blocks[0].outcome,
        Outcome::ShownConfirmed,
        "{:?}",
        r.blocks[0]
    );
    assert_eq!(
        r.blocks[0].sources[0].passage, "p1",
        "the nearest passage when none is cited"
    );
    assert_eq!(
        r.blocks[1].outcome,
        Outcome::ShownUnconfirmed,
        "{:?}",
        r.blocks[1]
    );
    assert!(
        r.blocks[1]
            .reason
            .contains("no model checker is configured"),
        "{}",
        r.blocks[1].reason
    );
    assert_eq!(r.counts.shown_confirmed, 1);
    assert_eq!(r.counts.shown_unconfirmed, 1);
}

#[test]
fn a_sentence_below_the_threshold_is_shown_with_the_label_never_as_confirmed() {
    let b = one(
        LABELLED,
        from_source(PARAPHRASE, &["p2"]),
        &mut FixedModel::at(0.4),
    );
    assert_eq!(b.outcome, Outcome::ShownUnconfirmed, "{b:?}");
    assert_eq!(b.label, "not from the article");
    assert_eq!(b.text, PARAPHRASE);
    assert!(
        b.reason.contains("0.400") && b.reason.contains("below the threshold"),
        "{}",
        b.reason
    );
    assert!(b.sources.is_empty());
}

#[test]
fn a_checker_error_is_shown_with_the_label() {
    let b = one(
        LABELLED,
        from_source(PARAPHRASE, &["p2"]),
        &mut FixedModel::failing(),
    );
    assert_eq!(b.outcome, Outcome::ShownUnconfirmed, "{b:?}");
    assert!(b.reason.contains("could not answer"), "{}", b.reason);
}

#[test]
fn a_sentence_citing_no_valid_passage_is_not_put_to_the_model() {
    let mut model = FixedModel::at(0.99);
    let b = one(
        LABELLED,
        from_source(PARAPHRASE, &["p7", "intro"]),
        &mut model,
    );
    assert_eq!(b.outcome, Outcome::ShownUnconfirmed, "{b:?}");
    assert!(model.seen.is_empty(), "the model was not asked");
    assert!(b.reason.contains("2 ids it lacks"), "{}", b.reason);
    let cited = b
        .answers
        .iter()
        .find(|a| a.basis == CITED_NAME && a.label == Label::NotApplicable)
        .expect("the cited-passages answer");
    assert_eq!(
        cited.detail["invalid_cited_sha256"],
        json!([
            sha256_digest("p7".as_bytes()),
            sha256_digest("intro".as_bytes())
        ])
    );
}

// Catches: the six-word run admitting a sentence, under which twelve
// verbatim words carry an invented clause to "confirmed".
#[test]
fn twelve_verbatim_words_and_an_invented_clause_are_not_confirmed() {
    let b = one(
        LABELLED,
        from_source(TWELVE_WORDS_AND_AN_INVENTED_CLAUSE, &["p2"]),
        &mut FixedModel::at(0.14),
    );
    assert_eq!(b.outcome, Outcome::ShownUnconfirmed, "{b:?}");
    assert!(six_word_run_yes(&b), "the run is recorded as evidence");

    // Without a model, too.
    let r = deterministic(
        LABELLED,
        vec![from_source(TWELVE_WORDS_AND_AN_INVENTED_CLAUSE, &["p2"])],
    );
    assert_eq!(r.blocks[0].outcome, Outcome::ShownUnconfirmed);
    assert!(six_word_run_yes(&r.blocks[0]));
    let r = deterministic(
        STRICT,
        vec![from_source(TWELVE_WORDS_AND_AN_INVENTED_CLAUSE, &["p2"])],
    );
    assert_eq!(r.blocks[0].outcome, Outcome::WithheldUnconfirmed);
}

#[test]
fn an_own_sentence_beside_a_quote_is_checked_against_the_quoted_passage() {
    let mut model = FixedModel::with(0.0, Some(0.95));
    let (r, _) = render_items(
        LABELLED,
        vec![
            quote_of("p2", None),
            own("The plan includes no new parking."),
        ],
        Some(&mut model),
    );
    let own_case = model.seen.iter().find(|c| c.id == "item-1").unwrap();
    assert!(
        own_case.passage.contains("two hundred parking spaces"),
        "the quoted passage reaches the checker: {}",
        own_case.passage
    );
    let b = &r.blocks[1];
    assert_eq!(b.outcome, Outcome::WithheldContradiction, "{b:?}");
    assert!(b.reason.contains("(p1, p2)"), "{}", b.reason);
}

#[test]
fn an_own_sentence_is_shown_with_the_label_and_checked_for_contradiction() {
    let mut model = FixedModel::with(0.0, Some(0.2));
    let (r, _) = render_items(
        LABELLED,
        vec![
            from_source(PARAPHRASE, &["p2"]),
            own("That is about two million pounds in today's money."),
        ],
        Some(&mut model),
    );
    let b = &r.blocks[1];
    assert_eq!(b.outcome, Outcome::ShownOwn, "{b:?}");
    assert_eq!(b.label, "not from the article");
    assert!(
        b.reason
            .contains("contradiction probability against p1, p2 is 0.200"),
        "{}",
        b.reason
    );
    assert_eq!(model.seen.len(), 2);
    assert_eq!(model.seen[1].id, "item-1");
    assert!(
        model.seen[1].passage.starts_with("Before the vote"),
        "checked against what the reply cites"
    );
    assert_eq!(r.counts.shown_own, 1);
}

#[test]
fn an_own_contradiction_at_the_confident_threshold_is_withheld() {
    let b = one(
        LABELLED,
        own(CONTRADICTION),
        &mut FixedModel::with(0.0, Some(0.95)),
    );
    assert_eq!(b.outcome, Outcome::WithheldContradiction, "{b:?}");
    assert_eq!(b.label, "from the source");
    assert!(
        b.text.starts_with("The council approved the harbour plan"),
        "the lead, which the reply cites by default"
    );
    assert!(
        b.reason.contains("0.950") && b.reason.contains("own sentence was not shown"),
        "{}",
        b.reason
    );
}

#[test]
fn a_from_source_contradiction_at_the_threshold_shows_the_cited_passage() {
    let b = one(
        LABELLED,
        from_source(CONTRADICTION, &["p0"]),
        &mut FixedModel::with(0.05, Some(0.9)),
    );
    assert_eq!(b.outcome, Outcome::WithheldContradiction, "{b:?}");
    assert_eq!(b.passage.as_deref(), Some("p0"));
}

#[test]
fn an_unsure_checker_labels_rather_than_hides() {
    let b = one(
        LABELLED,
        own(CONTRADICTION),
        &mut FixedModel::with(0.0, Some(0.6)),
    );
    assert_eq!(b.outcome, Outcome::ShownOwn, "{b:?}");
    let b = one(
        LABELLED,
        from_source(CONTRADICTION, &["p0"]),
        &mut FixedModel::with(0.1, Some(0.6)),
    );
    assert_eq!(b.outcome, Outcome::ShownUnconfirmed, "{b:?}");
}

#[test]
fn a_model_without_a_contradiction_class_withholds_nothing_for_it() {
    let b = one(
        LABELLED,
        own(CONTRADICTION),
        &mut FixedModel::with(0.0, None),
    );
    assert_eq!(b.outcome, Outcome::ShownOwn, "{b:?}");
    assert!(b.reason.contains("no contradiction class"), "{}", b.reason);
    let (r, record) = render_items(
        LABELLED,
        vec![own(CONTRADICTION)],
        Some(&mut FixedModel::with(0.0, None)),
    );
    assert_eq!(record["detail"]["withholds_for_contradiction"], false);
    assert_eq!(r.counts.withheld_contradiction, 0);
}

#[test]
fn without_a_model_an_own_sentence_is_labelled_and_the_reason_says_so() {
    let r = deterministic(LABELLED, vec![own("An own remark.")]);
    assert_eq!(r.blocks[0].outcome, Outcome::ShownOwn);
    assert!(
        r.blocks[0]
            .reason
            .contains("no model checker is configured"),
        "{}",
        r.blocks[0].reason
    );
}

#[test]
fn a_misattributed_quotation_is_withheld_in_both_modes() {
    for mode in [LABELLED, STRICT] {
        let b = one(
            mode,
            from_source(MISATTRIBUTED, &["p1"]),
            &mut FixedModel::at(0.99),
        );
        assert_eq!(b.outcome, Outcome::WithheldAttribution, "{mode}: {b:?}");
        assert_eq!(b.passage.as_deref(), Some("p1"));
        let b = one(mode, own(MISATTRIBUTED), &mut FixedModel::at(0.99));
        assert_eq!(b.outcome, Outcome::WithheldAttribution, "{mode} own: {b:?}");
        let r = deterministic(mode, vec![from_source(MISATTRIBUTED, &[])]);
        assert_eq!(
            r.blocks[0].outcome,
            Outcome::WithheldAttribution,
            "{mode} no model"
        );
    }
}

#[test]
fn a_cut_kept_passage_is_withheld_in_both_modes() {
    for mode in [LABELLED, STRICT] {
        let b = one(
            mode,
            from_source(FRAGMENT, &["p1"]),
            &mut FixedModel::at(0.99),
        );
        assert_eq!(b.outcome, Outcome::WithheldKept, "{mode}: {b:?}");
        assert!(
            b.text
                .starts_with("We accept the inspector's findings in full and we will publish")
        );
        assert_eq!(b.label, "from the source, kept quotation shown whole");
        let b = one(
            mode,
            own("The council approved the harbour plan on Tuesday."),
            &mut FixedModel::at(0.0),
        );
        assert_eq!(
            b.outcome,
            Outcome::WithheldKept,
            "{mode} own lead fragment: {b:?}"
        );
        assert_eq!(b.passage.as_deref(), Some("p0"));
    }
}

// --- strict mode ---

#[test]
fn strict_mode_shows_only_confirmed_sentences_and_the_sources_words() {
    let mut model = FixedModel::at(0.95);
    let (r, _) = render_items(
        STRICT,
        vec![
            from_source(PARAPHRASE, &["p2"]),
            own("That is about two million pounds in today's money."),
        ],
        Some(&mut model),
    );
    assert_eq!(r.display_mode, "strict");
    assert_eq!(
        r.blocks[0].outcome,
        Outcome::ShownConfirmed,
        "{:?}",
        r.blocks[0]
    );
    assert_eq!(
        r.blocks[1].outcome,
        Outcome::WithheldUnconfirmed,
        "{:?}",
        r.blocks[1]
    );
    assert_eq!(r.blocks[1].label, "from the source");
    assert!(
        r.blocks[1]
            .reason
            .contains("strict display: the assistant's own sentence is not shown")
    );
    assert_eq!(r.counts.shown_own, 0);
    assert_eq!(r.counts.withheld_unconfirmed, 1);
}

#[test]
fn strict_mode_shows_the_cited_passage_for_an_unconfirmed_sentence() {
    let b = one(
        STRICT,
        from_source(PARAPHRASE, &["p2"]),
        &mut FixedModel::at(0.4),
    );
    assert_eq!(b.outcome, Outcome::WithheldUnconfirmed, "{b:?}");
    assert_eq!(
        b.passage.as_deref(),
        Some("p2"),
        "the cited passage is shown instead"
    );
    assert!(
        b.reason.contains("was not confirmed") && b.reason.contains("0.400"),
        "{}",
        b.reason
    );
    let b = one(
        STRICT,
        from_source(PARAPHRASE, &["p9"]),
        &mut FixedModel::at(0.99),
    );
    assert_eq!(b.outcome, Outcome::WithheldUnconfirmed, "{b:?}");
    let b = one(
        STRICT,
        from_source(PARAPHRASE, &["p2"]),
        &mut FixedModel::failing(),
    );
    assert_eq!(b.outcome, Outcome::WithheldUnconfirmed, "{b:?}");
}

#[test]
fn strict_mode_withholds_a_contradiction_as_a_contradiction() {
    let b = one(
        STRICT,
        own(CONTRADICTION),
        &mut FixedModel::with(0.0, Some(0.95)),
    );
    assert_eq!(b.outcome, Outcome::WithheldContradiction, "{b:?}");
}

#[test]
fn strict_mode_without_a_model_shows_verbatim_sentences_and_the_sources_words() {
    let r = deterministic(
        STRICT,
        vec![
            from_source(VERBATIM, &[]),
            from_source(PARAPHRASE, &["p2"]),
            own("An own remark."),
        ],
    );
    let outcomes: Vec<Outcome> = r.blocks.iter().map(|b| b.outcome).collect();
    assert_eq!(
        outcomes,
        [
            Outcome::ShownConfirmed,
            Outcome::WithheldUnconfirmed,
            Outcome::WithheldUnconfirmed
        ]
    );
}

// --- the counts and the batch ---

#[test]
fn the_counts_vector_has_one_entry_per_outcome_in_order() {
    let mut model = FixedModel::at(0.95);
    let (r, record) = render_items(
        LABELLED,
        vec![
            quote_of("p2", None),
            quote_of("p9", None),
            from_source(PARAPHRASE, &["p2"]),
            own("An own remark."),
            from_source(PARAPHRASE, &["p9"]),
        ],
        Some(&mut model),
    );
    assert_eq!(record["detail"]["count_names"], json!(Counts::NAMES));
    // fetch, quote_shown, expanded, refused, confirmed, own, unconfirmed,
    // contradiction, attribution, kept, unconfirmed-withheld
    assert_eq!(r.counts.as_vec(), [1, 1, 0, 1, 1, 1, 1, 0, 0, 0, 0]);
    assert_eq!(record["detail"]["counts_vector"], json!(r.counts.as_vec()));
    let ids: Vec<&str> = model.seen.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["item-2", "item-3"], "one batch, in item order");

    let (r, _) = render_items(
        STRICT,
        vec![
            own(CONTRADICTION),
            own("An own remark."),
            from_source(PARAPHRASE, &["p2"]),
        ],
        Some(&mut FixedModel::with(0.1, Some(0.95))),
    );
    assert_eq!(r.counts.as_vec(), [1, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0]);
    let (r, _) = render_items(
        STRICT,
        vec![own("An own remark."), from_source(PARAPHRASE, &["p2"])],
        Some(&mut FixedModel::at(0.1)),
    );
    assert_eq!(r.counts.as_vec(), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
}

#[test]
fn every_outcome_reaches_the_counts_without_a_model() {
    let r = deterministic(
        LABELLED,
        vec![
            quote_of("p2", None),
            quote_of("p0", Some(0)),
            quote_of("p9", None),
            from_source(VERBATIM, &[]),
            own("An own remark."),
            from_source(PARAPHRASE, &["p2"]),
            from_source(MISATTRIBUTED, &[]),
            from_source(FRAGMENT, &[]),
        ],
    );
    // The lead is one sentence, so sentence 0 of it covers the kept lead.
    assert_eq!(r.counts.as_vec(), [1, 2, 0, 1, 1, 1, 1, 0, 1, 1, 0]);
    let r = deterministic(
        STRICT,
        vec![own("An own remark."), from_source(PARAPHRASE, &[])],
    );
    assert_eq!(r.counts.as_vec(), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
}

#[test]
fn support_reads_only_the_whole_sentence_stage_and_the_named_model() {
    let answer = |basis: &str, label: Label, p: f64| Answer {
        question: Question::Supported,
        label,
        probability: Some(p),
        basis: basis.into(),
        reason: String::new(),
        detail: Value::Null,
    };
    let six_words_only = vec![
        answer("fidelity-verifier/1", Label::Yes, 1.0),
        answer(WHOLE_NAME, Label::No, 0.0),
    ];
    assert_eq!(support(&six_words_only, None, 0.8), None);
    assert_eq!(support(&six_words_only, Some("m"), 0.8), None);
    let mut with_other_stage = six_words_only.clone();
    with_other_stage.push(answer("other-stage", Label::Yes, 0.99));
    assert_eq!(support(&with_other_stage, Some("m"), 0.8), None);
    let mut with_model = six_words_only.clone();
    with_model.push(answer("m", Label::Yes, 0.81));
    assert_eq!(
        support(&with_model, Some("m"), 0.8),
        Some(Admitted::Model { probability: 0.81 })
    );
    assert_eq!(support(&with_model, Some("m"), 0.82), None);
    let whole = vec![answer(WHOLE_NAME, Label::Yes, 1.0)];
    assert_eq!(support(&whole, None, 0.8), Some(Admitted::Verbatim));
}

// --- quotes ---

#[test]
fn a_quote_is_rendered_from_the_signed_text() {
    let r = deterministic(LABELLED, vec![quote_of("p2", None)]);
    assert_eq!(r.blocks[0].outcome, Outcome::QuoteShown);
    assert!(r.blocks[0].text.starts_with("The harbour plan sets out"));
    assert_eq!(r.counts.quote_shown, 1);
}

#[test]
fn a_quote_given_as_words_is_rendered_from_the_signed_text() {
    let r = deterministic(
        LABELLED,
        vec![
            quote_of(VERBATIM, None),
            quote_of("The council leader, Tom Reilly, agreed.", None),
        ],
    );
    assert_eq!(r.blocks[0].outcome, Outcome::QuoteShown);
    assert_eq!(r.blocks[0].passage.as_deref(), Some("p1"));
    assert_eq!(r.blocks[1].outcome, Outcome::QuoteRefused);
}

#[test]
fn an_unknown_passage_is_refused_and_a_sentence_covering_a_kept_span_is_shown() {
    let r = deterministic(
        LABELLED,
        vec![
            quote_of("p9", None),
            quote_of("p1", Some(1)),
            quote_of("p1", Some(9)),
        ],
    );
    assert_eq!(r.blocks[0].outcome, Outcome::QuoteRefused);
    // Sentence 1 of p1 is the statement with its tag: it covers the kept
    // quotation, so it is shown as a quote.
    assert_eq!(
        r.blocks[1].outcome,
        Outcome::QuoteShown,
        "{:?}",
        r.blocks[1]
    );
    assert_eq!(r.blocks[2].outcome, Outcome::QuoteRefused);
    assert!(r.blocks[2].reason.contains("no sentence 9"));
    assert_eq!(r.counts.quote_refused, 2);
}

#[test]
fn a_quote_that_cuts_a_kept_span_is_expanded_to_the_whole_span() {
    let r = deterministic(
        LABELLED,
        vec![quote_of("we will publish the costs of the scheme", None)],
    );
    let b = &r.blocks[0];
    assert_eq!(b.outcome, Outcome::QuoteExpandedToKept, "{b:?}");
    assert!(b.text.starts_with("We accept the inspector's findings"));
    assert_eq!(b.passage.as_deref(), Some("p1"));
    assert_eq!(r.counts.quote_expanded_to_kept, 1);
}

/// The inputs of a display of the same reply against `release`: the text,
/// the signed sidecar, the reply.
fn input_hashes(release: &Release) -> Vec<Value> {
    let reply = Reply {
        items: vec![from_source(VERBATIM, &["p1"])],
    };
    invoke(&read(release), URL, &reply).0.to_value()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|input| input["content_hash"].clone())
        .collect()
}

// Catches: two releases with the same text and different signed display
// fields recorded as the same input.
#[test]
fn the_inputs_bind_the_signed_mode_passages_and_markers_beside_the_text() {
    let base = signed(LABELLED, &PARAGRAPHS);
    let resigned = |change: &dyn Fn(&mut Signed)| {
        let mut signed = base.sidecar.signed.clone();
        change(&mut signed);
        sign(base.text.clone(), signed)
    };
    let before = input_hashes(&base);
    for (what, release) in [
        (
            "display mode",
            resigned(&|s| s.display_mode = STRICT.into()),
        ),
        (
            "passages",
            resigned(&|s| {
                let last = s.passages.pop().unwrap();
                s.passages.last_mut().unwrap().end = last.end;
            }),
        ),
        (
            "markers",
            resigned(&|s| {
                s.markers.pop();
            }),
        ),
    ] {
        let after = input_hashes(&release);
        assert_eq!(after[0], before[0], "{what}: the text is the same");
        assert_ne!(after[1], before[1], "{what}: the sidecar input differs");
        assert_eq!(after[2], before[2], "{what}: the reply is the same");
    }
}
