//! The integrity checker: a deterministic `verify`-stage processor that
//! decides how a reply is displayed against a verified signed release.
//!
//! A reply's items say what they are ([`commonmeasure_types::display::Item`]):
//! a quote by passage id, a sentence drawn from the source with the passages
//! it draws on, or a sentence of the assistant's own. Each sentence is put
//! to fixed questions, each answered by its own stage: is it supported, is
//! the right person quoted, does it cut a kept passage, and, from a model
//! stage only, does it contradict the source. The answers stay separate, raw,
//! per item; nothing combines them into a score. The display rule then gives
//! each item one outcome.
//!
//! The rule labels rather than withholds. Under the `labelled` mode a
//! release asks for by default, a sentence from the source that the checker
//! confirms (the whole sentence verbatim in the release text, or a model at
//! its confident-yes threshold against the passages it cites) is shown plain
//! with its passages; the assistant's own sentence, and a sentence from the
//! source that is not confirmed, is shown labelled "not from the article".
//! The source's own words replace a sentence only where it cuts a kept
//! passage, attributes a quotation to someone the source does not, or
//! contradicts the source at the confident contradiction threshold. Under
//! `strict`, only confirmed sentences and the source's own words are shown.
//! The fidelity verifier's six-word run is recorded and admits nothing.
//!
//! The deterministic stages read the whole release text. A model stage reads
//! only the passages a sentence cites and their neighbours; an own sentence
//! is put to it against the passages the reply as a whole cites, or the lead.
//! No model stage is installed in this build: the processor runs the
//! deterministic stages only and its record says so.
//!
//! The invocation record keeps no text of the reply or the release. A
//! sentence is recorded by its SHA-256 and length, the release's words by
//! byte span into the release text, a quotation by its span in the sentence
//! and its hash, and an identifier the reply supplied that the release does
//! not have by its hash. Passage ids and marker labels the release signed are
//! kept as they are: the fetch record keeps the sidecar whole. The text
//! itself stays in the [`Rendered`] value returned to the caller.
//!
//! The processor reads a release the release verifier has verified, so every
//! passage and marker lies in the text and every marker within the passage
//! it names. It neither screens the release's strings nor checks its
//! signature again; a release whose signature did not verify is not
//! displayed as anything from the source.

mod attribution;
mod keep_intact;
mod verbatim;
mod words;

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::Utc;
use commonmeasure_types::canonical::{canonical_json, sha256_digest};
use commonmeasure_types::display::{Counts, Item, Outcome, Reply};
use commonmeasure_types::release::{KeptSpan, LABELLED, Release, STRICT, VerificationState};
use commonmeasure_types::{Decision, Gap, GapReason};
use serde::Serialize;
use serde_json::{Value, json};

use super::release::Read;
use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use verbatim::WHOLE_NAME;
use words::{find_run, longest_common_run, words};

pub const NAME: &str = "integrity-checker";
pub const VERSION: &str = "1";

/// The rules, stated canonically. The configuration digest covers this text.
const RULES: &str = "\
input: a signed release whose signature verified, and a reply of items: quote (a passage id, \
optionally a sentence index within it), from_source (a sentence and the passage ids it cites) \
and own (a sentence). A release whose verification is failed or missing is not displayed: \
the processor abstains with a gap.\n\
display mode: the release's signed display_mode; labelled where it says labelled, strict \
otherwise.\n\
words: whitespace-delimited, compared case-insensitively with leading and trailing \
punctuation removed and typographic quotes and dashes folded.\n\
stages, each over the sentence and the whole release text, answers yes, no or not \
applicable with probability 1, 0 or none: fidelity-verifier/1 (supported where a run of at \
least six consecutive words of the sentence occurs in the text; evidence only, admits \
nothing); verbatim-whole/1 (supported where every word of the sentence occurs in the text \
consecutively and in order); attribution-lookup/1 (a quotation of at least three words \
between double quotation marks that the sentence attributes to a named speaker by X said, \
said X or according to X is found verbatim in the text; yes where the speaker's surname is \
the text's own attribution beside it, or with no attribution there is named in the sentence \
carrying the quotation or the one before; no where it is not or the quotation is not in the \
text; not applicable where the sentence quotes nobody, names no speaker, or the text \
attributes by pronoun); keep-intact-containment/1 (a kept span is cut where at least six \
consecutive words of the sentence occur in it and the whole span does not occur in the \
sentence). A model stage, where one is configured, answers supported and contradicts over \
the cited passages and their neighbours; an own sentence over the passages the reply quotes \
or cites and their neighbours, or the lead.\n\
quote: rendered from the text by passage id, the whole passage or the indexed sentence of \
it, sentences split where . ! or ? is followed by whitespace with closing quotes and \
brackets kept; where the id names no passage and holds three or more words that occur \
whole in the text, those words are rendered; a span that touches a kept span without \
covering it is replaced by the whole kept span (quote_expanded_to_kept); a passage or \
sentence the release lacks is refused (quote_refused); otherwise quote_shown.\n\
sentence, first rule that applies: a cut kept span shows the whole span (withheld_kept); \
an attribution no shows the source's passage (withheld_attribution); a model contradiction \
probability, raw, at or above the contradiction threshold shows the source's passage \
(withheld_contradiction); a from_source sentence supported by verbatim-whole, or by the \
model's probability after its temperature at or above its threshold, is shown plain with \
its cited passages, or the nearest passage where none is cited (shown_confirmed); under \
strict, the source's passage is shown instead (withheld_unconfirmed); an own sentence is \
shown labelled not from the article (shown_own); otherwise the sentence is shown labelled \
not from the article (shown_unconfirmed). The source's passage is the cited passage sharing \
the longest word run with the sentence, or with none cited the passage sharing a run of at \
least six words, or the lead.\n\
record: no text of the reply or the release; a sentence by its sha256 and byte length, \
shown words by byte span into the release text, a quotation by its byte span in the sentence \
and its sha256, a speaker by its sha256, an id the release does not have by its sha256.\n\
counts: fetch is 1; each outcome adds one to its count; the vector is fetch, quote_shown, \
quote_expanded_to_kept, quote_refused, shown_confirmed, shown_own, shown_unconfirmed, \
withheld_contradiction, withheld_attribution, withheld_kept, withheld_unconfirmed.\n";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Verify,
    capability: "display-integrity",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: NO_AMBIENT_AUTHORITY,
    determinism: Determinism::Deterministic,
    limits: "in-process and synchronous; per sentence, word matching over the release text \
             quadratic in the words of the sentence and of each kept span or passage, no \
             supervisor timeout",
    // A reply the checker could not rule on is never displayed as confirmed
    // from the source, whatever the mode.
    failure_by_mode: FailureByMode {
        strict: FailBehaviour::FailClosed,
        prefer: FailBehaviour::FailClosed,
        observe: FailBehaviour::FailClosed,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "No model stage runs in this build: a paraphrase of the source is never confirmed, and \
     no sentence is withheld for contradicting it.",
    "Confirmed means the sentence occurs whole in the release text, not that it answers the \
     question or is true.",
    "Attribution is read from patterns (X said, said X, according to X); other patterns and \
     paraphrased attribution are not seen, and a speaker named by pronoun is not resolved.",
    "A paraphrase of a kept passage reproduces none of its words and is not seen as a cut.",
    "The release's passages and terms are the source's statements; the checker reads them as \
     signed and does not judge them.",
];

/// The questions the stages answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    Supported,
    RightPersonQuoted,
    CutsKeptPassage,
    /// The sentence states something the source says is otherwise, as
    /// opposed to something the source does not mention. Only a model stage
    /// answers it.
    Contradicts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Yes,
    No,
    /// The question does not arise for this sentence (no quotation, no
    /// marker) or the stage cannot decide it.
    NotApplicable,
}

/// One stage's answer to one question, kept raw.
#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub question: Question,
    pub label: Label,
    /// The probability of `yes` where the stage produces one: 1 or 0 from a
    /// deterministic stage, a model's own otherwise.
    pub probability: Option<f64>,
    /// The stage that answered, by name and version.
    pub basis: String,
    pub reason: String,
    /// Stage-specific evidence: spans, runs, a model's raw output.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

/// A kept span as a stage reads it: byte offsets into the case's text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Marker {
    start: usize,
    end: usize,
    label: String,
}

/// One sentence and the text it is checked against.
#[derive(Debug, Clone)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) passage: String,
    pub(crate) summary: String,
    markers: Vec<Marker>,
}

/// A model stage behind the display rule. The supervised model checker
/// implements it; no build installs one yet, so [`invoke`] runs without.
pub(crate) trait ModelChecker {
    /// The stage's name as its answers record it in `basis`.
    fn name(&self) -> String;
    /// What the stage declares about itself, for the record.
    fn manifest(&self) -> Value;
    fn questions(&self) -> Vec<Question>;
    /// Answers for each case, in the cases' order. A case the stage could
    /// not answer carries answers with no probability and the reason.
    ///
    /// The answers go into the invocation record whole, so neither `reason`
    /// nor `detail` may carry the case's sentence or passage text, or a
    /// model's output that quotes them.
    fn check_many(&mut self, cases: &[Case]) -> Vec<Vec<Answer>>;
}

/// A model stage and the thresholds the rule reads its probabilities at.
pub(crate) struct Model<'a> {
    pub(crate) checker: &'a mut dyn ModelChecker,
    /// The confident-yes threshold, applied after the temperature.
    pub(crate) threshold: f64,
    /// The temperature fitted for the stage; 1.0 leaves its probability.
    pub(crate) temperature: f64,
    /// The raw contradiction probability at or above which a sentence is
    /// withheld; below it an unsure stage labels rather than hides.
    pub(crate) contradiction_threshold: f64,
}

/// A passage a shown sentence links to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub passage: String,
    pub text: String,
}

/// A byte span: `start` and `end` offsets into the text it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// The span of `text[start..end]` with surrounding whitespace removed.
    fn trimmed(text: &str, start: usize, end: usize) -> Self {
        let slice = &text[start..end];
        let lead = slice.len() - slice.trim_start().len();
        Self {
            start: start + lead,
            end: start + lead + slice.trim().len(),
        }
    }
}

/// One displayed item.
#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub outcome: Outcome,
    /// What the reader is told the text is.
    pub label: String,
    /// The text shown: the release's words for a quote or a withheld
    /// sentence, the sentence itself otherwise.
    pub text: String,
    /// The passage the shown words come from, where they are the release's.
    pub passage: Option<String>,
    /// The byte span of the shown words in the release text, where they are
    /// the release's.
    pub span: Option<Span>,
    /// The passages a confirmed sentence links to.
    pub sources: Vec<Source>,
    pub reason: String,
    /// Every stage's answer for a sentence; empty for a quote.
    pub answers: Vec<Answer>,
}

/// A reply as displayed.
#[derive(Debug, Clone, Serialize)]
pub struct Rendered {
    /// The display mode applied: `labelled` or `strict`.
    pub display_mode: &'static str,
    pub blocks: Vec<Block>,
    pub counts: Counts,
}

/// Decide how `reply` is displayed against the verified release `read`
/// served at `reference`, with the deterministic stages only.
///
/// Returns the invocation record and, where the release verified, the
/// rendering. A release whose signature failed or is missing yields no
/// rendering and a record that abstains with a gap.
pub fn invoke(read: &Read, reference: &str, reply: &Reply) -> (Invocation, Option<Rendered>) {
    invoke_with(read, reference, reply, None)
}

pub(crate) fn invoke_with(
    read: &Read,
    reference: &str,
    reply: &Reply,
    model: Option<Model<'_>>,
) -> (Invocation, Option<Rendered>) {
    let started_at = Utc::now();
    let release = &read.release;
    let inputs = vec![
        ArtefactRef {
            reference: reference.to_owned(),
            content_hash: Some(read.content_hash.clone()),
            tokens: None,
        },
        // The text hash alone does not pin what changes the display: the
        // display mode, passages and markers are signed beside the text.
        ArtefactRef {
            reference: format!("{reference}#sidecar"),
            content_hash: Some(digest_of(&release.sidecar)),
            tokens: None,
        },
        ArtefactRef {
            reference: format!("{reference}#reply"),
            content_hash: Some(digest_of(reply)),
            tokens: None,
        },
    ];
    let verification = read.record.verification;
    if verification != VerificationState::Verified {
        let state = match verification {
            VerificationState::Missing => "missing",
            _ => "failed",
        };
        let gap = Gap::new(
            GapReason::EvidenceMissing,
            format!(
                "the release at {reference} has a {state} signature, so its text is not \
                 established as the source's; nothing of the reply is displayed as from \
                 the source."
            ),
        );
        let invocation = Invocation::new(
            manifest(),
            started_at,
            Decision::Abstain,
            "the release's verification state was read before any stage ran",
            inputs,
            Vec::new(),
            json!({ "verification": verification, "displayed": false }),
            vec![gap],
            BLIND_SPOTS.to_vec(),
        );
        return (invocation, None);
    }

    let (rendered, record) = render(release, reply, model);
    let invocation = Invocation::new(
        manifest(),
        started_at,
        // The verify stage admits nothing: the display stands beside the
        // reply, and its record says what was shown and why.
        Decision::Abstain,
        "each sentence put to the deterministic stages over the release text, then the \
         display rule the configuration digest pins",
        inputs,
        vec![ArtefactRef {
            reference: format!("{reference}#display"),
            content_hash: Some(digest_of(&rendered.blocks)),
            tokens: None,
        }],
        record,
        Vec::new(),
        BLIND_SPOTS.to_vec(),
    );
    (invocation, Some(rendered))
}

fn digest_of(value: &impl Serialize) -> String {
    let value = serde_json::to_value(value).expect("display values serialise");
    sha256_digest(canonical_json(&value).as_bytes())
}

fn hashes(values: &[String]) -> Vec<String> {
    values.iter().map(|v| sha256_digest(v.as_bytes())).collect()
}

/// The passage a block's shown words come from, as the record names it: a
/// refused quote's id is the reply's, kept only where the release signed it.
fn shown_passage(release: &Release, block: &Block) -> Value {
    match &block.passage {
        Some(id) if release.passage(id).is_some() => json!(id),
        _ => Value::Null,
    }
}

fn scaled(p: f64, temperature: f64) -> f64 {
    let p = p.clamp(1e-6, 1.0 - 1e-6);
    let logit = (p / (1.0 - p)).ln();
    1.0 / (1.0 + (-(logit / temperature)).exp())
}

/// Why a sentence from the source counts as supported for the display.
#[derive(Debug, Clone, PartialEq)]
enum Admitted {
    /// The whole sentence occurs verbatim in the release text.
    Verbatim,
    /// The model's probability, after the temperature, is at or above the
    /// threshold.
    Model { probability: f64 },
}

/// The support decision. Only the whole-sentence verbatim test and the model
/// named by `model` at or above `threshold` can admit a sentence; the
/// six-word run's answer, and any other stage's, admits nothing.
fn support(answers: &[Answer], model: Option<&str>, threshold: f64) -> Option<Admitted> {
    if answers.iter().any(|a| {
        a.question == Question::Supported && a.basis == WHOLE_NAME && a.label == Label::Yes
    }) {
        return Some(Admitted::Verbatim);
    }
    let model = model?;
    answers
        .iter()
        .filter(|a| a.question == Question::Supported && a.basis == model)
        .filter_map(|a| a.probability)
        .find(|&p| p >= threshold)
        .map(|probability| Admitted::Model { probability })
}

/// The contradiction probability the named model reported, raw: no
/// temperature is fitted for it.
fn contradiction(answers: &[Answer], model: Option<&str>) -> Option<f64> {
    let model = model?;
    answers
        .iter()
        .find(|a| a.question == Question::Contradicts && a.basis == model)
        .and_then(|a| a.probability)
}

/// The basis recorded when a sentence cites no passage the release has and
/// so is not put to the model stage.
const CITED_NAME: &str = "cited-passages/1";

/// Why a sentence from the source was not confirmed.
fn unconfirmed_why(answers: &[Answer], cited: &Cited, model: Option<&str>) -> String {
    let Some(model) = model else {
        return "no model checker is configured and the sentence is not whole in the signed text"
            .to_owned();
    };
    if cited.passage.is_none() {
        return answers
            .iter()
            .find(|a| a.basis == CITED_NAME)
            .map(|a| a.reason.clone())
            .unwrap_or_else(|| "the sentence cites no passage".to_owned());
    }
    match answers
        .iter()
        .find(|a| a.question == Question::Supported && a.basis == model)
    {
        Some(Answer {
            probability: Some(p),
            ..
        }) => format!(
            "the model checker's probability of support, {p:.3} after the temperature, is \
             below the threshold against the passages the sentence cites ({})",
            cited.checked_against.join(", ")
        ),
        Some(a) => a.reason.clone(),
        None => "the model checker gave no answer for the sentence".to_owned(),
    }
}

/// The kept span a quote span touches without covering, if any.
fn cut_marker(markers: &[KeptSpan], start: usize, end: usize) -> Option<&KeptSpan> {
    markers
        .iter()
        .find(|m| start < m.end && end > m.start && !(start <= m.start && end >= m.end))
}

/// The passage sharing the longest word run with `sentence`, where that run
/// reaches six words; the lead otherwise. None where the release has no
/// passage.
fn nearest_passage<'a>(release: &'a Release, sentence: &str) -> Option<(&'a str, &'a str)> {
    let sw = words(sentence);
    let mut best: Option<(&str, &str, usize)> = None;
    for (id, text) in passages(release) {
        let (run, _, _) = longest_common_run(&sw, &words(text));
        if best.is_none_or(|b| run > b.2) {
            best = Some((id, text, run));
        }
    }
    match best {
        Some((id, text, run)) if run >= keep_intact::MINIMUM_RUN_WORDS => Some((id, text)),
        _ => passages(release).next(),
    }
}

fn passages(release: &Release) -> impl Iterator<Item = (&str, &str)> {
    release
        .sidecar
        .signed
        .passages
        .iter()
        .filter_map(|p| Some((p.id.as_str(), release.text.get(p.start..p.end)?)))
}

/// What the model stage is given for one sentence: the cited passages the
/// release has plus their immediate neighbours, in text order, with the
/// kept spans that lie in them made relative to that text.
#[derive(Debug, Clone, PartialEq)]
struct Cited {
    /// The cited ids the release has, in text order.
    valid: Vec<String>,
    /// The cited ids it does not have.
    invalid: Vec<String>,
    /// The ids the model reads: the valid ones and their neighbours.
    checked_against: Vec<String>,
    /// The text and its markers, or none where nothing valid was cited.
    passage: Option<(String, Vec<Marker>)>,
}

fn cited(release: &Release, ids: &[String]) -> Cited {
    let signed = &release.sidecar.signed;
    let mut valid_at: Vec<usize> = Vec::new();
    let mut invalid = Vec::new();
    for id in ids {
        match signed.passages.iter().position(|p| &p.id == id) {
            Some(at) if !valid_at.contains(&at) => valid_at.push(at),
            Some(_) => {}
            None => invalid.push(id.clone()),
        }
    }
    valid_at.sort_unstable();
    cited_at(release, valid_at, invalid)
}

/// What an own sentence is checked against: every passage the reply quotes
/// or cites plus neighbours, or the lead where it names none. A quoted
/// passage counts because the reader sees it beside the own sentence; a
/// quote naming a passage the release lacks adds nothing.
fn reply_passages(release: &Release, reply: &Reply) -> Cited {
    let signed = &release.sidecar.signed;
    let ids: Vec<String> = reply
        .items
        .iter()
        .flat_map(|item| match item {
            Item::FromSource { passages, .. } => passages.clone(),
            Item::Quote { passage, .. } if signed.passages.iter().any(|p| &p.id == passage) => {
                vec![passage.clone()]
            }
            _ => Vec::new(),
        })
        .collect();
    let mut c = cited(release, &ids);
    if c.passage.is_none() && !signed.passages.is_empty() {
        c = cited_at(release, vec![0], Vec::new());
    }
    c
}

fn cited_at(release: &Release, valid_at: Vec<usize>, invalid: Vec<String>) -> Cited {
    let signed = &release.sidecar.signed;
    let mut read_at: Vec<usize> = Vec::new();
    for &at in &valid_at {
        for n in [at.saturating_sub(1), at, at + 1] {
            if n < signed.passages.len() && !read_at.contains(&n) {
                read_at.push(n);
            }
        }
    }
    read_at.sort_unstable();
    let ids = |at: &[usize]| -> Vec<String> {
        at.iter()
            .map(|&at| signed.passages[at].id.clone())
            .collect()
    };
    let passage = (!read_at.is_empty()).then(|| {
        let mut text = String::new();
        let mut markers = Vec::new();
        for &at in &read_at {
            let p = &signed.passages[at];
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            let offset = text.len();
            text.push_str(&release.text[p.start..p.end]);
            for m in signed.markers.iter().filter(|m| m.passage == p.id) {
                markers.push(Marker {
                    start: offset + (m.start - p.start),
                    end: offset + (m.end - p.start),
                    label: format!("{} ({})", m.label, m.passage),
                });
            }
        }
        (text, markers)
    });
    Cited {
        valid: ids(&valid_at),
        invalid,
        checked_against: ids(&read_at),
        passage,
    }
}

/// The passage shown in place of a sentence: the cited passage sharing the
/// longest word run with it, or, with nothing cited, the nearest passage.
fn instead<'a>(
    release: &'a Release,
    sentence: &str,
    valid: &[String],
) -> Option<(&'a str, &'a str)> {
    let sw = words(sentence);
    let mut best: Option<(&str, &str, usize)> = None;
    for id in valid {
        if let Some((p, text)) = release.passage(id) {
            let (run, _, _) = longest_common_run(&sw, &words(text));
            if best.is_none_or(|b| run > b.2) {
                best = Some((p.id.as_str(), text, run));
            }
        }
    }
    match best {
        Some((id, text, _)) => Some((id, text)),
        None => nearest_passage(release, sentence),
    }
}

/// A block of the release's own words, `release.text[start..end]`.
fn source_block(
    outcome: Outcome,
    label: impl Into<String>,
    release: &Release,
    (start, end): (usize, usize),
    passage: &str,
    reason: impl Into<String>,
) -> Block {
    let span = Span::trimmed(&release.text, start, end);
    Block {
        outcome,
        label: label.into(),
        text: release.text[span.start..span.end].to_owned(),
        passage: Some(passage.to_owned()),
        span: Some(span),
        sources: Vec::new(),
        reason: reason.into(),
        answers: Vec::new(),
    }
}

/// A refused quote. `passage` is the id the quote named, which the release
/// may not have.
fn refused(passage: &str, reason: String) -> Block {
    Block {
        outcome: Outcome::QuoteRefused,
        label: "not shown".to_owned(),
        text: String::new(),
        passage: Some(passage.to_owned()),
        span: None,
        sources: Vec::new(),
        reason,
        answers: Vec::new(),
    }
}

/// A quote item: rendered from the release text by passage id, expanded to a
/// kept span it cuts, or refused.
fn quote(release: &Release, passage: &str, sentence: Option<usize>) -> Block {
    let signed = &release.sidecar.signed;
    let kept_block = |kept: &KeptSpan, what: &str| {
        source_block(
            Outcome::QuoteExpandedToKept,
            format!("from the source, kept {} shown whole", kept.label),
            release,
            (kept.start, kept.end),
            &kept.passage,
            format!(
                "the {what} cut a passage the source keeps intact; the whole span is shown \
                 instead"
            ),
        )
    };
    // A model sometimes puts the words it wants to quote where the passage
    // id goes. Words that occur whole in the release text are still the
    // source's own words, so they are rendered from it.
    if release.passage(passage).is_none() {
        let wanted = words(passage);
        let text_words = words(&release.text);
        let found = (wanted.len() >= 3)
            .then(|| find_run(&wanted, &text_words))
            .flatten();
        if let Some(at) = found {
            let start = text_words[at].start;
            let end = text_words[at + wanted.len() - 1].end;
            if let Some(kept) = cut_marker(&signed.markers, start, end) {
                return kept_block(kept, "quoted words");
            }
            let id = signed
                .passages
                .iter()
                .find(|p| start >= p.start && start < p.end)
                .map(|p| p.id.as_str())
                .unwrap_or_default();
            return source_block(
                Outcome::QuoteShown,
                "from the source",
                release,
                (start, end),
                id,
                "the quoted words occur whole in the signed text and are rendered from it",
            );
        }
        return refused(
            passage,
            "the signed text has no passage by the id the quote names".to_owned(),
        );
    }
    let Some((p, text)) = release.passage(passage) else {
        return refused(
            passage,
            "the signed text has no passage by the id the quote names".to_owned(),
        );
    };
    let (start, end) = match sentence {
        Some(n) => match attribution::sentences(text).get(n) {
            Some(&(s, e)) => (p.start + s, p.start + e),
            None => return refused(passage, format!("passage {passage} has no sentence {n}")),
        },
        None => (p.start, p.end),
    };
    if let Some(kept) = cut_marker(&signed.markers, start, end) {
        return kept_block(kept, "requested span");
    }
    source_block(
        Outcome::QuoteShown,
        "from the source",
        release,
        (start, end),
        passage,
        "rendered from the signed text by passage id",
    )
}

/// The display rule over the deterministic stages and, where given, a model
/// stage. Returns the rendering and the invocation record's detail.
fn render(release: &Release, reply: &Reply, mut model: Option<Model<'_>>) -> (Rendered, Value) {
    let signed = &release.sidecar.signed;
    let mut counts = Counts::one_fetch();
    let mut blocks = Vec::new();
    let mut items = Vec::new();
    let model_name = model.as_ref().map(|m| m.checker.name());
    let mode = match signed.display_mode.as_str() {
        LABELLED => LABELLED,
        _ => STRICT,
    };
    let strict = mode == STRICT;

    // The model's cases first, all at once: each from_source sentence
    // against the passages it cites, each own sentence against the passages
    // the reply cites.
    let for_own = reply_passages(release, reply);
    let mut model_cases: Vec<(usize, Case)> = Vec::new();
    let mut citations: HashMap<usize, Cited> = HashMap::new();
    for (index, item) in reply.items.iter().enumerate() {
        let (text, c) = match item {
            Item::FromSource { text, passages } => (text, cited(release, passages)),
            Item::Own { text } => (text, for_own.clone()),
            Item::Quote { .. } => continue,
        };
        if let Some((passage, markers)) = &c.passage {
            model_cases.push((
                index,
                Case {
                    id: format!("item-{index}"),
                    passage: passage.clone(),
                    summary: text.clone(),
                    markers: markers.clone(),
                },
            ));
        }
        citations.insert(index, c);
    }
    let mut model_answers: HashMap<usize, Vec<Answer>> = HashMap::new();
    if let Some(model) = model.as_mut()
        && !model_cases.is_empty()
    {
        let cases: Vec<Case> = model_cases.iter().map(|(_, c)| c.clone()).collect();
        let answered = model.checker.check_many(&cases);
        for ((index, _), answers) in model_cases.iter().zip(answered) {
            model_answers.insert(*index, answers);
        }
    }
    let model_asks_contradiction = model
        .as_ref()
        .is_some_and(|m| m.checker.questions().contains(&Question::Contradicts));
    // Markers relative to the whole text, which the deterministic stages
    // read: a cut kept span is a cut wherever the span lies.
    let all_markers: Vec<Marker> = signed
        .markers
        .iter()
        .map(|m| Marker {
            start: m.start,
            end: m.end,
            label: format!("{} ({})", m.label, m.passage),
        })
        .collect();

    for (index, item) in reply.items.iter().enumerate() {
        let (text, own) = match item {
            Item::Quote { passage, sentence } => {
                let block = quote(release, passage, *sentence);
                counts.add(block.outcome);
                let mut record = json!({
                    "index": index,
                    "kind": "quote",
                    "sentence": sentence,
                    "outcome": block.outcome,
                    "shown_passage": shown_passage(release, &block),
                    "shown_span": block.span,
                    "reason": block.reason,
                });
                // The id field sometimes holds the words a model wanted to
                // quote; an id the release did not sign is kept by hash.
                if release.passage(passage).is_some() {
                    record["passage"] = json!(passage);
                } else {
                    record["passage_sha256"] = json!(sha256_digest(passage.as_bytes()));
                }
                items.push(record);
                blocks.push(block);
                continue;
            }
            Item::FromSource { text, .. } => (text, false),
            Item::Own { text } => (text, true),
        };
        let case = Case {
            id: format!("item-{index}"),
            passage: release.text.clone(),
            summary: text.clone(),
            markers: all_markers.clone(),
        };
        let (kept_answer, cut_at) = keep_intact::check(&case);
        let mut answers = vec![
            verbatim::six_word_run(&case),
            verbatim::whole(&case),
            attribution::check(&case),
            kept_answer,
        ];
        let cited = citations.remove(&index).unwrap_or_else(|| Cited {
            valid: Vec::new(),
            invalid: Vec::new(),
            checked_against: Vec::new(),
            passage: None,
        });
        if let Some(model) = &model {
            match model_answers.remove(&index) {
                Some(model_answers) => {
                    for mut answer in model_answers {
                        // The raw probability stays in the detail; the scaled
                        // one is what the support threshold reads. The
                        // contradiction probability stays raw.
                        if answer.detail.is_null() {
                            answer.detail = json!({});
                        }
                        if answer.question == Question::Supported
                            && let Some(p) = answer.probability
                        {
                            answer.detail["p_raw"] = json!(p);
                            answer.detail["temperature"] = json!(model.temperature);
                            answer.probability = Some(scaled(p, model.temperature));
                        }
                        answer.detail["cited_passages"] = json!(cited.valid);
                        answer.detail["checked_against"] = json!(cited.checked_against);
                        if !cited.invalid.is_empty() {
                            answer.detail["invalid_cited_sha256"] = json!(hashes(&cited.invalid));
                        }
                        answers.push(answer);
                    }
                }
                None => answers.push(Answer {
                    question: Question::Supported,
                    label: Label::NotApplicable,
                    probability: None,
                    basis: CITED_NAME.to_owned(),
                    reason: if cited.invalid.is_empty() {
                        "the sentence cites no passage, so the model checker was not asked"
                            .to_owned()
                    } else {
                        format!(
                            "the sentence cites no passage the signed text has ({} ids it \
                             lacks), so the model checker was not asked",
                            cited.invalid.len()
                        )
                    },
                    detail: json!({ "invalid_cited_sha256": hashes(&cited.invalid) }),
                }),
            }
        }
        let threshold = model.as_ref().map_or(f64::INFINITY, |m| m.threshold);
        let contradiction_threshold = model
            .as_ref()
            .map_or(f64::INFINITY, |m| m.contradiction_threshold);
        let admitted = if own {
            None
        } else {
            support(&answers, model_name.as_deref(), threshold)
        };
        let contradicts = contradiction(&answers, model_name.as_deref());
        let attribution_no = answers
            .iter()
            .find(|a| a.question == Question::RightPersonQuoted && a.label == Label::No)
            .map(|a| a.reason.clone());
        let fallback = instead(release, text, &cited.valid);
        let fallback_text = fallback.map_or("", |(_, text)| text);
        let no_passage = if fallback.is_none() {
            "; the release has no passage to show instead"
        } else {
            ""
        };
        let kind = if own { "own" } else { "from_source" };
        let withheld = |outcome, reason: String, answers| Block {
            outcome,
            label: "from the source".to_owned(),
            text: fallback_text.trim().to_owned(),
            passage: fallback.map(|(id, _)| id.to_owned()),
            span: fallback
                .and_then(|(id, _)| release.passage(id))
                .map(|(p, _)| Span::trimmed(&release.text, p.start, p.end)),
            sources: Vec::new(),
            reason: format!("{reason}{no_passage}"),
            answers,
        };

        let block = if let Some(kept) = cut_at.and_then(|at| signed.markers.get(at)) {
            let cut_reason = &answers[3].reason;
            let mut block = source_block(
                Outcome::WithheldKept,
                format!("from the source, kept {} shown whole", kept.label),
                release,
                (kept.start, kept.end),
                &kept.passage,
                format!(
                    "the {kind} sentence was not shown: {cut_reason}; the whole span is shown \
                     instead"
                ),
            );
            block.answers = answers;
            block
        } else if let Some(reason) = attribution_no {
            withheld(
                Outcome::WithheldAttribution,
                format!(
                    "the {kind} sentence was not shown: {reason}; the source's own passage is \
                     shown instead"
                ),
                answers,
            )
        } else if let Some(c) = contradicts.filter(|&c| c >= contradiction_threshold) {
            withheld(
                Outcome::WithheldContradiction,
                format!(
                    "the {kind} sentence was not shown: the model checker's probability that it \
                     contradicts the passages it was checked against ({}), {c:.3}, is at or \
                     above the contradiction threshold {contradiction_threshold}; the source's \
                     own passage is shown instead",
                    cited.checked_against.join(", "),
                ),
                answers,
            )
        } else if let Some(admitted) = admitted {
            let basis = match admitted {
                Admitted::Verbatim => {
                    "the whole sentence occurs verbatim in the signed text".to_owned()
                }
                Admitted::Model { probability } => format!(
                    "the model checker's probability of support, {probability:.3} after the \
                     temperature, is at or above the threshold {threshold} against the \
                     passages the sentence cites"
                ),
            };
            let mut sources: Vec<Source> = cited
                .valid
                .iter()
                .filter_map(|id| release.passage(id))
                .map(|(p, t)| Source {
                    passage: p.id.clone(),
                    text: t.to_owned(),
                })
                .collect();
            if sources.is_empty()
                && let Some((id, text)) = fallback
            {
                sources.push(Source {
                    passage: id.to_owned(),
                    text: text.to_owned(),
                });
            }
            Block {
                outcome: Outcome::ShownConfirmed,
                label: "confirmed from the article".to_owned(),
                text: text.clone(),
                passage: None,
                span: None,
                sources,
                reason: basis,
                answers,
            }
        } else if strict {
            let reason = if own {
                "the source asks for strict display: the assistant's own sentence is not shown; \
                 the source's nearest passage is shown instead"
                    .to_owned()
            } else {
                format!(
                    "the source asks for strict display and the sentence was not confirmed \
                     ({}); the source's passage is shown instead",
                    unconfirmed_why(&answers, &cited, model_name.as_deref())
                )
            };
            withheld(Outcome::WithheldUnconfirmed, reason, answers)
        } else if own {
            let contradiction_note = match &model {
                None => "no model checker is configured, so nothing is withheld for contradiction"
                    .to_owned(),
                Some(_) if !model_asks_contradiction => "the configured checker has no \
                     contradiction class, so nothing is withheld for contradiction"
                    .to_owned(),
                Some(_) => match contradicts {
                    Some(c) => format!(
                        "the checker's contradiction probability against {} is {c:.3}, below \
                         the threshold {contradiction_threshold}",
                        cited.checked_against.join(", "),
                    ),
                    None => {
                        "the checker could not answer the contradiction question for it".to_owned()
                    }
                },
            };
            Block {
                outcome: Outcome::ShownOwn,
                label: "not from the article".to_owned(),
                text: text.clone(),
                passage: None,
                span: None,
                sources: Vec::new(),
                reason: format!(
                    "the assistant's own sentence, shown with the label; {contradiction_note}"
                ),
                answers,
            }
        } else {
            Block {
                outcome: Outcome::ShownUnconfirmed,
                label: "not from the article".to_owned(),
                text: text.clone(),
                passage: None,
                span: None,
                sources: Vec::new(),
                reason: format!(
                    "shown with the label, not as confirmed: {}",
                    unconfirmed_why(&answers, &cited, model_name.as_deref())
                ),
                answers,
            }
        };
        counts.add(block.outcome);
        let mut record = json!({
            "index": index,
            "kind": kind,
            "text_sha256": sha256_digest(text.as_bytes()),
            "text_bytes": text.len(),
            "outcome": block.outcome,
            "shown_passage": block.passage,
            "shown_span": block.span,
            "sources": block.sources.iter().map(|s| &s.passage).collect::<Vec<_>>(),
            "reason": block.reason,
            "answers": block.answers,
        });
        if !own {
            record["cited_passages"] = json!(cited.valid);
            record["invalid_cited_sha256"] = json!(hashes(&cited.invalid));
        }
        items.push(record);
        blocks.push(block);
    }

    let stages = [
        (verbatim::six_word_name(), Question::Supported),
        (WHOLE_NAME.to_owned(), Question::Supported),
        (attribution::NAME.to_owned(), Question::RightPersonQuoted),
        (keep_intact::NAME.to_owned(), Question::CutsKeptPassage),
    ];
    let record = json!({
        "verification": VerificationState::Verified,
        "display_mode_requested": signed.display_mode,
        "display_mode": mode,
        "stages": stages
            .iter()
            .map(|(name, question)| json!({ "name": name, "questions": [question] }))
            .collect::<Vec<_>>(),
        "model": model.as_ref().map(|m| json!({
            "manifest": m.checker.manifest(),
            "questions": m.checker.questions(),
            "threshold": m.threshold,
            "temperature": m.temperature,
            "contradiction_threshold": m.contradiction_threshold,
        })),
        "withholds_for_contradiction": model_asks_contradiction,
        "items": items,
        "count_names": Counts::NAMES,
        "counts_vector": counts.as_vec(),
    });
    (
        Rendered {
            display_mode: mode,
            blocks,
            counts,
        },
        record,
    )
}

#[cfg(test)]
mod tests;
