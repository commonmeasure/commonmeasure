//! The context optimiser: a deterministic `transform`-stage processor.
//!
//! Two operations, each recorded as its own transformation so a reviewer can
//! check one without the other (`docs/FAIL-POLICY.md` §12): whole-source
//! deduplication and repeated-phrase deduplication. Ordering is the provider's retrieval order, preserved and
//! said so. No ranking, no learned compression — those are later sidecars,
//! and a deterministic rule set is what makes this record checkable.
//!
//! The transformation record is re-derivable: excise the recorded spans from
//! the original text and the SHA-256 of what remains is the recorded output
//! hash. The original envelope stays addressable throughout — a transformation
//! never replaces provenance with a blob.

use commonmeasure_types::canonical::sha256_digest;
use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::Utc;
use commonmeasure_types::{ContextEnvelope, Decision};
use serde::Serialize;
use serde_json::json;

use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, NO_AMBIENT_AUTHORITY, ProcessorManifest, Stage,
};
use crate::policy::{TOKEN_BASIS, approximate_tokens};

pub const NAME: &str = "context-optimiser";
pub const VERSION: &str = "2";

/// A repeated phrase must span at least this many words before its repeat is
/// removed. Short repeats ("the cap", a heading) are left alone: removing
/// them saves almost nothing and risks damaging a sentence.
const MINIMUM_PHRASE_WORDS: usize = 4;

/// The rule set, stated canonically; the configuration digest covers it.
const RULES: &str = "\
whole-source-deduplication: a source whose lowercased, whitespace-collapsed text \
equals an earlier source's is removed whole, and the record names the source kept\n\
repeated-phrase-deduplication: within the surviving sources in retrieval order, a \
run of at least 4 consecutive words inside one block (text between newlines or \
runs of two or more whitespace characters) whose lowercased word sequence already \
appears inside one kept block closed before this block was reached is removed, \
together with the whitespace before it; the first occurrence is kept\n\
removal-breaks-adjacency: a removal parts the kept text either side of it, so no \
phrase is matched across a removal and every antecedent recorded is one unbroken \
stretch of the source it names\n\
ordering: the provider's retrieval order is preserved\n";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Transform,
    capability: "context-deduplication",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(RULES.as_bytes()),
    permissions: NO_AMBIENT_AUTHORITY,
    determinism: Determinism::Deterministic,
    limits: "in-process and synchronous; one pass over each source, no supervisor timeout",
    failure_by_mode: FailureByMode {
        // A transform that cannot run leaves the context as it was, with a
        // gap: unoptimised context is correct context.
        strict: FailBehaviour::FailOpen,
        prefer: FailBehaviour::FailOpen,
        observe: FailBehaviour::FailOpen,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "Deduplication precedes budget fitting: a phrase kept only in an earlier source \
     leaves the context entirely if that source is later dropped by the budget.",
    "Deterministic textual rules only: no relevance judgement is made, and \
     near-duplicates that differ in wording survive.",
];

/// Where a removed phrase was first kept, so the removal is a pointer rather
/// than a loss.
#[derive(Debug, Clone, Serialize)]
pub struct SpanOrigin {
    pub reference: String,
    pub start: usize,
    pub end: usize,
}

/// One removed phrase: exact byte offsets into the original text (preceding
/// whitespace included), so excising the recorded spans re-derives the output
/// text.
#[derive(Debug, Clone, Serialize)]
pub struct RemovedSpan {
    pub start: usize,
    pub end: usize,
    pub words: u64,
    pub content_hash: String,
    pub first_kept: SpanOrigin,
}

/// A source that survived, with what its text became.
#[derive(Debug)]
pub struct KeptSource<'a> {
    pub envelope: &'a ContextEnvelope,
    pub text: String,
    pub input_hash: String,
    pub output_hash: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub removed_spans: Vec<RemovedSpan>,
}

/// A source removed whole as a duplicate. Its envelope remains addressable;
/// only its admission to this plan's context changed.
#[derive(Debug)]
pub struct DroppedSource<'a> {
    pub envelope: &'a ContextEnvelope,
    pub duplicate_of: String,
    pub tokens: u64,
}

#[derive(Debug)]
pub struct Transformation<'a> {
    pub kept: Vec<KeptSource<'a>>,
    pub dropped: Vec<DroppedSource<'a>>,
}

impl Transformation<'_> {
    pub fn tokens_out(&self) -> u64 {
        self.kept.iter().map(|source| source.tokens_out).sum()
    }
}

/// One word of kept context: its canonical form, and where it stands in the
/// original text so an antecedent can be pointed at.
struct KeptWord {
    form: String,
    source: usize,
    start: usize,
    end: usize,
}

/// One indexed n-gram: where it starts, and the end of the stretch it belongs
/// to, past which a match may not be extended.
struct Occurrence {
    at: usize,
    stretch_end: usize,
}

/// The kept context so far, in stretches. A stretch is words that stood
/// adjacent in one block of one source with nothing taken out between them,
/// and it is the unit both sides of a match must lie inside: a phrase read
/// across a paragraph, a source boundary or a removal is a phrase no source
/// ever contained.
#[derive(Default)]
struct KeptContext {
    words: Vec<KeptWord>,
    /// Every matchable n-gram of `MINIMUM_PHRASE_WORDS` forms, keyed by the
    /// joined forms.
    index: HashMap<String, Vec<Occurrence>>,
    /// Where the stretch being read now began; `None` outside any block.
    stretch_start: Option<usize>,
    /// The stretches of the block being read now. They become matchable when
    /// that block closes and not before, because an antecedent has to be in a
    /// block already behind the reader.
    pending: Vec<(usize, usize)>,
}

impl KeptContext {
    fn open_block(&mut self) {
        self.stretch_start = Some(self.words.len());
    }

    /// A removal ends the stretch: the words either side of it were not
    /// adjacent in the original, so nothing may be read across the join.
    fn interrupt(&mut self) {
        if let Some(start) = self.stretch_start.replace(self.words.len()) {
            self.pending.push((start, self.words.len()));
        }
    }

    fn close_block(&mut self) {
        if let Some(start) = self.stretch_start.take() {
            self.pending.push((start, self.words.len()));
        }
        for (start, end) in std::mem::take(&mut self.pending) {
            for at in start..(end + 1).saturating_sub(MINIMUM_PHRASE_WORDS) {
                let key = self.ngram_key(at);
                self.index.entry(key).or_default().push(Occurrence {
                    at,
                    stretch_end: end,
                });
            }
        }
    }

    fn push(&mut self, word: KeptWord) {
        self.words.push(word);
    }

    fn ngram_key(&self, at: usize) -> String {
        self.words[at..at + MINIMUM_PHRASE_WORDS]
            .iter()
            .map(|word| word.form.as_str())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// The longest kept run matching `words` inside one stretch, of at least
    /// the minimum length. Returns the antecedent position and length.
    fn longest_match(&self, words: &[Word]) -> Option<(usize, usize)> {
        if words.len() < MINIMUM_PHRASE_WORDS {
            return None;
        }
        let key = words[..MINIMUM_PHRASE_WORDS]
            .iter()
            .map(|word| word.form.as_str())
            .collect::<Vec<_>>()
            .join("\u{1}");
        let mut best: Option<(usize, usize)> = None;
        for occurrence in self.index.get(&key)? {
            let mut length = MINIMUM_PHRASE_WORDS;
            while length < words.len()
                && occurrence.at + length < occurrence.stretch_end
                && self.words[occurrence.at + length].form == words[length].form
            {
                length += 1;
            }
            if best.is_none_or(|(_, held)| length > held) {
                best = Some((occurrence.at, length));
            }
        }
        best
    }
}

#[derive(Debug)]
struct Word {
    start: usize,
    end: usize,
    form: String,
}

/// Deduplicate the admitted sources, in retrieval order.
pub fn invoke<'a>(sources: &[(&'a ContextEnvelope, &'a str)]) -> (Invocation, Transformation<'a>) {
    let started_at = Utc::now();
    let mut kept: Vec<KeptSource<'a>> = Vec::new();
    let mut dropped: Vec<DroppedSource<'a>> = Vec::new();
    let mut seen_sources: Vec<(String, &str)> = Vec::new();
    let mut context = KeptContext::default();

    for &(envelope, text) in sources {
        let whole = canonical(text);
        if let Some((_, url)) = seen_sources.iter().find(|(held, _)| *held == whole) {
            dropped.push(DroppedSource {
                envelope,
                duplicate_of: (*url).to_owned(),
                tokens: approximate_tokens(text),
            });
            continue;
        }
        seen_sources.push((whole, &envelope.source_url));

        let source_index = kept.len();
        let mut removed_spans: Vec<RemovedSpan> = Vec::new();
        let mut previous_end = 0usize;
        for block in blocks(text) {
            context.open_block();
            let words = words_of(&text[block.start..block.end], block.start);
            let mut at = 0;
            while at < words.len() {
                match context.longest_match(&words[at..]) {
                    Some((antecedent, length)) => {
                        let span_start = previous_end;
                        let span_end = words[at + length - 1].end;
                        previous_end = span_end;
                        removed_spans.push(RemovedSpan {
                            start: span_start,
                            end: span_end,
                            words: length as u64,
                            content_hash: sha256_digest(&text.as_bytes()[span_start..span_end]),
                            first_kept: SpanOrigin {
                                reference: kept
                                    .get(context.words[antecedent].source)
                                    .map(|source| source.envelope.source_url.clone())
                                    .unwrap_or_else(|| envelope.source_url.clone()),
                                start: context.words[antecedent].start,
                                end: context.words[antecedent + length - 1].end,
                            },
                        });
                        context.interrupt();
                        at += length;
                    }
                    None => {
                        let word = &words[at];
                        previous_end = word.end;
                        context.push(KeptWord {
                            form: word.form.clone(),
                            source: source_index,
                            start: word.start,
                            end: word.end,
                        });
                        at += 1;
                    }
                }
            }
            context.close_block();
        }
        let output = excise(text, &removed_spans);
        kept.push(KeptSource {
            envelope,
            input_hash: sha256_digest(text.as_bytes()),
            output_hash: sha256_digest(output.as_bytes()),
            tokens_in: approximate_tokens(text),
            tokens_out: approximate_tokens(&output),
            text: output,
            removed_spans,
        });
    }

    let tokens_in: u64 = sources
        .iter()
        .map(|(_, text)| approximate_tokens(text))
        .sum();
    let transformation = Transformation { kept, dropped };
    let removed_phrases: usize = transformation
        .kept
        .iter()
        .map(|source| source.removed_spans.len())
        .sum();
    let invocation = Invocation::new(
        manifest(),
        started_at,
        Decision::Admit,
        if transformation.dropped.is_empty() && removed_phrases == 0 {
            "deterministic deduplication in retrieval order; no duplication found, the \
             context is unchanged"
                .to_owned()
        } else {
            format!(
                "deterministic deduplication in retrieval order; removed {} whole source{} \
                 and {removed_phrases} repeated phrase{}",
                transformation.dropped.len(),
                if transformation.dropped.len() == 1 {
                    ""
                } else {
                    "s"
                },
                if removed_phrases == 1 { "" } else { "s" },
            )
        },
        sources
            .iter()
            .map(|(envelope, text)| ArtefactRef {
                reference: envelope.source_url.clone(),
                content_hash: Some(sha256_digest(text.as_bytes())),
                tokens: Some(approximate_tokens(text)),
            })
            .collect(),
        transformation
            .kept
            .iter()
            .map(|source| ArtefactRef {
                reference: source.envelope.source_url.clone(),
                content_hash: Some(source.output_hash.clone()),
                tokens: Some(source.tokens_out),
            })
            .collect(),
        json!({
            "token_basis": TOKEN_BASIS,
            "tokens_in": tokens_in,
            "tokens_out": transformation.tokens_out(),
            "removed_sources": transformation
                .dropped
                .iter()
                .map(|source| json!({
                    "reference": source.envelope.source_url,
                    "duplicate_of": source.duplicate_of,
                    "tokens": source.tokens,
                }))
                .collect::<Vec<_>>(),
            "removed_spans": transformation
                .kept
                .iter()
                .filter(|source| !source.removed_spans.is_empty())
                .map(|source| json!({
                    "reference": source.envelope.source_url,
                    "input_hash": source.input_hash,
                    "output_hash": source.output_hash,
                    "spans": source.removed_spans,
                }))
                .collect::<Vec<_>>(),
            "ordering": {
                "basis": "provider retrieval order, preserved",
                "input": sources
                    .iter()
                    .map(|(envelope, _)| envelope.source_url.as_str())
                    .collect::<Vec<_>>(),
                "output": transformation
                    .kept
                    .iter()
                    .map(|source| source.envelope.source_url.as_str())
                    .collect::<Vec<_>>(),
            },
        }),
        Vec::new(),
        BLIND_SPOTS.to_vec(),
    );
    (invocation, transformation)
}

/// Case and whitespace folded away, so cosmetic differences do not defeat
/// equality.
fn canonical(text: &str) -> String {
    text.split_whitespace()
        .flat_map(|word| word.chars().flat_map(char::to_lowercase).chain(" ".chars()))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[derive(Debug, PartialEq, Eq)]
struct Block {
    start: usize,
    end: usize,
}

/// Split text into blocks. A whitespace run is a block boundary when it
/// contains a newline or spans two or more characters; single spaces stay
/// inside their block.
fn blocks(text: &str) -> Vec<Block> {
    let mut found = Vec::new();
    let mut block: Option<(usize, usize)> = None;
    let mut characters = text.char_indices().peekable();
    while let Some((at, character)) = characters.next() {
        if !character.is_whitespace() {
            block = Some(match block {
                None => (at, at + character.len_utf8()),
                Some((start, _)) => (start, at + character.len_utf8()),
            });
            continue;
        }
        let mut run_characters = 1;
        let mut run_has_newline = character == '\n';
        while let Some(&(_, next)) = characters.peek() {
            if !next.is_whitespace() {
                break;
            }
            run_characters += 1;
            run_has_newline |= next == '\n';
            characters.next();
        }
        if (run_characters >= 2 || run_has_newline)
            && let Some((start, end)) = block.take()
        {
            found.push(Block { start, end });
        }
    }
    if let Some((start, end)) = block {
        found.push(Block { start, end });
    }
    found
}

/// The words of one block, with byte offsets into the whole text.
fn words_of(block: &str, offset: usize) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (at, character) in block.char_indices() {
        if character.is_whitespace() {
            if let Some(from) = start.take() {
                words.push(word(block, from, at, offset));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        words.push(word(block, from, block.len(), offset));
    }
    words
}

fn word(block: &str, from: usize, to: usize, offset: usize) -> Word {
    Word {
        start: offset + from,
        end: offset + to,
        form: block[from..to]
            .chars()
            .flat_map(char::to_lowercase)
            .collect(),
    }
}

/// The text with the recorded spans excised — the exact operation a reviewer
/// repeats to check the output hash.
fn excise(text: &str, spans: &[RemovedSpan]) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for span in spans {
        output.push_str(&text[cursor..span.start]);
        cursor = span.end;
    }
    output.push_str(&text[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_types::LicenceState;
    use serde_json::Value;

    fn envelope(url: &str, text: &str) -> ContextEnvelope {
        ContextEnvelope {
            source_url: url.to_owned(),
            host: "example.org".to_owned(),
            title: None,
            text: Some(text.to_owned()),
            content_hash: Some(sha256_digest(text.as_bytes())),
            licence: LicenceState::Unknown,
            declared_date: None,
            native_metadata: Value::Null,
            retrieval_rank: 1,
        }
    }

    #[test]
    fn an_identical_source_is_removed_whole_and_the_kept_one_is_named() {
        let first = envelope("https://a.example/1", "The cap is set quarterly by Ofgem.");
        let second = envelope("https://a.example/2", "the cap  is set quarterly by ofgem.");
        let (_, transformation) = invoke(&[
            (&first, "The cap is set quarterly by Ofgem."),
            (&second, "the cap  is set quarterly by ofgem."),
        ]);
        assert_eq!(transformation.kept.len(), 1);
        assert_eq!(transformation.dropped.len(), 1);
        assert_eq!(
            transformation.dropped[0].duplicate_of,
            "https://a.example/1"
        );
    }

    #[test]
    fn a_repeated_phrase_is_removed_from_the_later_source_only() {
        let boilerplate = "Cookies are used on this site";
        let first_text = format!("The cap rises in October.\n{boilerplate}");
        let second_text = format!("The cap falls in January.\n{boilerplate}");
        let first = envelope("https://a.example/1", &first_text);
        let second = envelope("https://a.example/2", &second_text);
        let (_, transformation) = invoke(&[(&first, &first_text), (&second, &second_text)]);

        assert!(transformation.kept[0].removed_spans.is_empty());
        assert_eq!(transformation.kept[0].text, first_text);
        let later = &transformation.kept[1];
        assert_eq!(later.removed_spans.len(), 1);
        assert_eq!(later.text, "The cap falls in January.");
        assert_eq!(
            later.removed_spans[0].first_kept.reference,
            "https://a.example/1"
        );
        assert!(later.tokens_out < later.tokens_in);
    }

    #[test]
    fn a_phrase_does_not_match_across_a_block_boundary() {
        // "# Prices" then "The cap" would form a four-word run only by
        // crossing the newline; matching across it would remove the subject
        // of an unrelated sentence.
        let first_text = "# Prices\nThe cap rises in October.";
        let second_text = "# Prices\nThe cap falls in January.";
        let first = envelope("https://a.example/1", first_text);
        let second = envelope("https://a.example/2", second_text);
        let (_, transformation) = invoke(&[(&first, first_text), (&second, second_text)]);
        assert!(
            transformation
                .kept
                .iter()
                .all(|source| source.removed_spans.is_empty())
        );
    }

    #[test]
    fn the_output_hash_is_rederivable_by_excising_the_recorded_spans() {
        let boilerplate = "Sign up to our newsletter for more";
        let first_text = format!("Fact one stands alone.  {boilerplate}");
        let second_text = format!("Fact two stands alone.  {boilerplate}  Fact three.");
        let first = envelope("https://a.example/1", &first_text);
        let second = envelope("https://a.example/2", &second_text);
        let (invocation, transformation) =
            invoke(&[(&first, &first_text), (&second, &second_text)]);

        let later = &transformation.kept[1];
        // The reviewer's operation: excise the recorded byte ranges from the
        // original text and hash what remains.
        let mut rederived = String::new();
        let mut cursor = 0;
        for span in &later.removed_spans {
            rederived.push_str(&second_text[cursor..span.start]);
            cursor = span.end;
        }
        rederived.push_str(&second_text[cursor..]);
        assert_eq!(sha256_digest(rederived.as_bytes()), later.output_hash);
        assert_eq!(later.text, "Fact two stands alone.  Fact three.");

        let record = invocation.to_value();
        assert_eq!(record["detail"]["tokens_in"].as_u64().unwrap(), 24);
        assert!(
            record["detail"]["tokens_out"].as_u64().unwrap()
                < record["detail"]["tokens_in"].as_u64().unwrap()
        );
        assert_eq!(record["detail"]["token_basis"], TOKEN_BASIS);
    }

    #[test]
    fn a_phrase_repeated_within_one_source_is_removed_at_its_second_occurrence() {
        let text = "Delivery is free over fifty pounds.\nTerms apply to all orders.\n\
                    Delivery is free over fifty pounds.";
        let first = envelope("https://a.example/1", text);
        let (_, transformation) = invoke(&[(&first, text)]);
        let kept = &transformation.kept[0];
        assert_eq!(kept.removed_spans.len(), 1);
        assert_eq!(
            kept.text,
            "Delivery is free over fifty pounds.\nTerms apply to all orders."
        );
        assert_eq!(
            kept.removed_spans[0].first_kept.reference, "https://a.example/1",
            "the antecedent is earlier in the same source"
        );
    }

    #[test]
    fn a_repeat_inside_the_block_being_read_is_left_alone() {
        // Both occurrences are in one sentence: removing the second would
        // leave "and then again", splicing away the clause between them. The
        // antecedent has to be in a block the reader has already passed.
        let text = "the cap is set quarterly and then the cap is set quarterly again";
        let first = envelope("https://a.example/1", text);
        let (_, transformation) = invoke(&[(&first, text)]);
        let kept = &transformation.kept[0];
        assert!(kept.removed_spans.is_empty());
        assert_eq!(kept.text, text);
    }

    #[test]
    fn a_later_source_does_not_match_across_an_earlier_removal() {
        // The first source loses its inner repeat; "yellow" and "pink" are
        // left either side of the gap. "yellow pink orange purple" stands in
        // no source text and must not be taken from the second.
        let first_text = "alpha beta gamma delta\n\n\
                          red green blue yellow alpha beta gamma delta pink orange purple mauve";
        let second_text = "yellow pink orange purple";
        let first = envelope("https://a.example/1", first_text);
        let second = envelope("https://a.example/2", second_text);
        let sources = [(&first, first_text), (&second, second_text)];
        let (_, transformation) = invoke(&sources);

        assert_eq!(transformation.kept[0].removed_spans.len(), 1);
        let later = &transformation.kept[1];
        assert!(later.removed_spans.is_empty());
        assert_eq!(later.text, second_text);
        assert_antecedents_hold_their_phrase(&sources, &transformation);
    }

    #[test]
    fn every_recorded_antecedent_holds_the_phrase_it_stands_for() {
        let fixtures: &[&[&str]] = &[
            &[
                "alpha beta gamma delta\n\n\
                 red green blue yellow alpha beta gamma delta pink orange purple mauve",
                "yellow pink orange purple",
            ],
            &[
                "one two three four five six seven eight",
                "nine ten one two three four eleven twelve",
                "nine ten eleven twelve thirteen",
            ],
            &[
                "Fact one stands alone.  Sign up to our newsletter for more",
                "Fact two stands alone.  Sign up to our newsletter for more  Fact three.",
                "Sign up to our newsletter for more\nFact four stands alone.",
            ],
            &[
                "Delivery is free over fifty pounds.\nTerms apply to all orders.\n\
                 Delivery is free over fifty pounds.",
                "Terms apply to all orders.\nDelivery is free over fifty pounds.",
            ],
        ];
        let mut removals = 0;
        for texts in fixtures {
            let envelopes: Vec<_> = texts
                .iter()
                .enumerate()
                .map(|(at, text)| envelope(&format!("https://a.example/{at}"), text))
                .collect();
            let sources: Vec<_> = envelopes
                .iter()
                .zip(texts.iter())
                .map(|(envelope, text)| (envelope, *text))
                .collect();
            let (_, transformation) = invoke(&sources);
            removals += transformation
                .kept
                .iter()
                .map(|source| source.removed_spans.len())
                .sum::<usize>();
            assert_antecedents_hold_their_phrase(&sources, &transformation);
        }
        assert!(removals > 0, "the fixtures must exercise removals");
    }

    /// The claim every removal makes: the phrase is still in the context, at
    /// the named source and byte range. Dereference it and look.
    fn assert_antecedents_hold_their_phrase(
        sources: &[(&ContextEnvelope, &str)],
        transformation: &Transformation<'_>,
    ) {
        let original = |url: &str| {
            sources
                .iter()
                .find(|(envelope, _)| envelope.source_url == url)
                .map(|(_, text)| *text)
                .expect("a record names an input source")
        };
        for kept in &transformation.kept {
            let text = original(&kept.envelope.source_url);
            for span in &kept.removed_spans {
                let removed = canonical(&text[span.start..span.end]);
                let origin = &span.first_kept;
                let held = canonical(&original(&origin.reference)[origin.start..origin.end]);
                assert_eq!(
                    removed.split(' ').count() as u64,
                    span.words,
                    "the span covers the words it says it does"
                );
                assert!(
                    format!(" {held} ").contains(&format!(" {removed} ")),
                    "first_kept {}[{}..{}] is {held:?}, which does not hold {removed:?}",
                    origin.reference,
                    origin.start,
                    origin.end,
                );
            }
        }
    }

    #[test]
    fn ordering_is_preserved_and_recorded() {
        let first = envelope("https://a.example/1", "Alpha beta gamma delta.");
        let second = envelope("https://a.example/2", "Epsilon zeta eta theta.");
        let (invocation, _) = invoke(&[
            (&first, "Alpha beta gamma delta."),
            (&second, "Epsilon zeta eta theta."),
        ]);
        let record = invocation.to_value();
        assert_eq!(
            record["detail"]["ordering"]["input"],
            record["detail"]["ordering"]["output"]
        );
        assert_eq!(
            record["detail"]["ordering"]["basis"],
            "provider retrieval order, preserved"
        );
    }
}
