//! The grounding evaluator: deterministic citation checks over a plan's
//! answer, against the exact text that entered the model's window.
//!
//! It verifies that what the answer cites is what the run supplied, and
//! measures nothing else. No evaluator here measures quality, coverage or
//! freshness, so an objective weighted on those is uncomputable and the
//! router says so rather than borrowing this record's verdicts. A citation
//! names a source URL and quotes it; the evaluator checks the URL against the
//! parts that entered the window and the quote against that part's retained
//! text, and keeps the four verdicts distinct — supported, contradicted,
//! uncovered, unavailable (`docs/contracts/run-output.md` §Plans) — because a
//! single count over them would be a score no measurement backs.
//!
//! Everything here is a pure function of the answer and the window content:
//! no clock, no sampling, no configuration beyond the versioned rule text
//! whose digest the record carries. Two replay runs of one suite produce
//! byte-identical evaluation records.

use commonmeasure_inference::ContextPart;
use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};

/// The evaluation-record contract version. Any change to the record shape
/// changes this.
pub const EVALUATION_VERSION: &str = "contextops-evaluation/v1";

pub const EVALUATOR_NAME: &str = "grounding";
pub const EVALUATOR_VERSION: &str = "0.4.0";

/// The instruction a suite opts into with `require_cited_answer`. It defines
/// the exact format this evaluator parses, so the two travel together: a
/// directive change is an evaluator change and moves the configuration
/// digest.
///
/// The placeholder is a bare word, never `<source URL>`: a model that copies
/// the shape of the template writes the brackets too, and markdown autolinking
/// makes that the likeliest decoration of all. And it shares no word with the
/// window's own `SOURCE <url> [<hash>]` labelling
/// (`commonmeasure_inference::window_block`): when the placeholder was `SOURCE_URL`, a
/// model reading the two side by side wrote the window's label into the URL
/// token, and four citations of admitted sources published as URLs the window
/// never saw.
pub const CITATION_DIRECTIVE: &str = "\n\nAfter your answer, cite the context you used: one line per citation, in \
     exactly this form, quoting the supplied source verbatim:\n\
     CITATION: CITED_URL :: \"the exact quote from that source\"\n\
     Replace CITED_URL with the source's URL alone, without the SOURCE label or the \
     bracketed hash beside it. Cite only sources named SOURCE in the supplied context.";

/// The directive's URL placeholder: a bare word (see [`CITATION_DIRECTIVE`]),
/// and not any word of the window's labelling — the seam test below holds
/// both properties against the production renderer. A citation carrying it
/// echoed the template rather than citing.
const URL_PLACEHOLDER: &str = "CITED_URL";

/// The directive's quote placeholder. A model that repeats it wrote no quote
/// at all, whatever URL stands beside it.
const QUOTE_PLACEHOLDER: &str = "the exact quote from that source";

/// The canonical rule text behind the configuration digest, written by hand.
///
/// Nothing derives it from the code, so nothing stops behaviour moving while
/// it stands still — and two runs that judged differently would then share one
/// digest and read as comparable. The convention that closes that gap is
/// editorial: a commit that changes what the evaluator decides changes this
/// text and `EVALUATOR_VERSION` in the same commit. The tests below pin the
/// digest and hold the version named here to the constant, so a behaviour
/// change that leaves either behind fails rather than publishes.
const RULES: &str = "grounding/0.4.0: parse `CITATION: CITED_URL :: \"QUOTE\"` lines from the answer. \
     The marker is read case-insensitively and behind any list, numbering, blockquote or \
     emphasis decoration the line carries; a line that names a citation the parser cannot \
     read is unavailable, never dropped. A URL wrapped in angle brackets or backticks is \
     read as the URL it wraps, and a URL copied with the window's own labelling — a \
     leading `SOURCE ` label, a trailing bracketed `sha256:` hash, or both — is read as \
     the URL it labels. A line that echoes a directive placeholder — `CITED_URL` in \
     place of a URL, or the template quote in place of a quote — is unavailable: the \
     template was repeated, so nothing was cited. supported = the URL is a window part \
     and the quote's word sequence occurs in the retained text of some part carrying \
     that URL, matching words case-insensitively, ignoring trailing punctuation, with \
     whitespace flexible; contradicted = the URL is a window part and no part carrying \
     it contains the sequence; uncovered = the URL is not a window part; unavailable = \
     the line does not parse or echoes the template. No answer, an empty answer, or a \
     suite that did not require a cited answer, leaves the record unevaluated with the \
     reason stated.";

pub fn configuration_digest() -> String {
    sha256_digest(format!("{RULES}{CITATION_DIRECTIVE}").as_bytes())
}

/// Every evaluator compiled into this binary, by identity. Sealed in the run
/// manifest because the evaluator set is fixed per experiment
/// (`docs/contracts/experiment.md`); like in-process processors, evaluators
/// are installed by being built in.
pub fn installed() -> Value {
    json!([
        identity(),
        crate::coverage::identity(),
        crate::freshness::identity(),
    ])
}

fn identity() -> Value {
    json!({
        "name": EVALUATOR_NAME,
        "version": EVALUATOR_VERSION,
        "configuration_digest": configuration_digest(),
    })
}

/// The evaluator's own limits, carried on every record so a reader weighs
/// the verdicts correctly.
const BLIND_SPOTS: &[&str] = &[
    "Verbatim quotes only: a paraphrase of the context is invisible to this check.",
    "Supported means the quote is in the supplied context, not that it is true.",
    "Contradicted means the cited source's retained text does not contain the quoted word \
     sequence. It is not a finding about the claim's truth, nor about the model's intent.",
    "Quotes are compared word by word, ignoring case entirely, whitespace, and trailing \
     punctuation. Case is ignored even where it carries meaning (Polish/polish, \
     March/march): supported is a coarse presence check — this word sequence entered \
     the window — and fidelity-grade matching (case-as-meaning, alteration marks, \
     emphasis) is deferred to a planned add-on. A quote that differs inside a word or \
     in leading punctuation is still contradicted even where the source says the same \
     thing, and part of a word is not a word.",
    "Claims the answer makes without a citation are not examined.",
];

/// What the runtime hands the evaluator: the answer, whether the suite asked
/// for citations, and the parts that entered the window, in order.
pub struct EvaluationInput<'a> {
    pub answer: Option<&'a str>,
    pub cited_answer_required: bool,
    pub parts: &'a [ContextPart],
}

/// Evaluate one plan. Always returns a record — an unevaluated plan carries
/// the evaluator's identity and the stated reason, never a bare null, so a
/// reader can tell "not checked, and why" from "nothing checked it".
pub fn grounding(input: &EvaluationInput) -> Value {
    // Ordered by how fundamental the obstacle is. A plan that produced no
    // answer is told that first: what a suite asked for is a lesser fact than
    // whether anything came back to check.
    let unevaluated = match input.answer {
        None => Some("no answer exists for this plan; inference did not complete"),
        // A gateway may return an empty string, which is an answer only in
        // shape. Scored, it would publish four zero counts and a citation
        // record — a measurement of nothing, indistinguishable from an answer
        // genuinely checked and found to declare no citations.
        Some(answer) if answer.trim().is_empty() => Some(
            "the answer is empty, so nothing was checkable; inference completed but \
             returned no text",
        ),
        Some(_) if !input.cited_answer_required => Some(
            "the suite did not require a cited answer, and citation checks need one \
             (set require_cited_answer)",
        ),
        Some(_) => None,
    };

    let citations: Vec<Value> = match (unevaluated, input.answer) {
        (None, Some(answer)) => answer
            .lines()
            .filter_map(|line| match citation_body(line) {
                Some(rest) => Some(judge(rest, input.parts)),
                // A line that names a citation the parser cannot read is a
                // malformed citation, not an absent one. Dropping it let an
                // answer that cited two sources — one of them fabricated —
                // publish as having declared none, with every count at zero.
                None if names_a_citation(line) => Some(unreadable_line(
                    line,
                    "no readable `CITATION: CITED_URL :: \"QUOTE\"` marker begins it",
                )),
                None => None,
            })
            .collect(),
        _ => Vec::new(),
    };

    let count = |verdict: &str| {
        citations
            .iter()
            .filter(|citation| citation["verdict"] == verdict)
            .count()
    };
    let mut inputs = Vec::new();
    if let Some(answer) = input.answer {
        inputs.push(json!({
            "reference": "answer",
            "content_hash": sha256_digest(answer.as_bytes()),
        }));
    }
    for part in input.parts {
        inputs.push(json!({
            "reference": part.source_ref,
            "content_hash": part.content_hash,
        }));
    }

    json!({
        "record_version": EVALUATION_VERSION,
        "evaluator": identity(),
        "method": "deterministic citation parsing and quote matching over the window \
                   parts (word sequences, case-insensitive, flexible in whitespace and \
                   trailing punctuation, otherwise exact)",
        "inputs": inputs,
        "verdict_counts": {
            "supported": count("supported"),
            "contradicted": count("contradicted"),
            "uncovered": count("uncovered"),
            "unavailable": count("unavailable"),
        },
        "citations": citations,
        "unevaluated": unevaluated,
        // Every verdict rests on the answer and window text this runtime
        // holds, never on a supplier's assurance about itself.
        "assurance": "observed",
        "blind_spots": BLIND_SPOTS,
    })
}

const MARKER: &str = "CITATION:";

/// The text after a line's `CITATION:` marker, if the line declares one.
///
/// A model formats a citation list the way it formats every other list, so
/// the marker arrives behind bullets, numbering, blockquotes and emphasis.
/// Reading only a bare line-leading marker meant an ordinary markdown list —
/// the most likely shape an answer takes — was seen as no citations at all.
fn citation_body(line: &str) -> Option<&str> {
    let mut rest = line.trim();
    loop {
        let stripped = rest
            .trim_start_matches(['-', '*', '+', '>', '#', '`', '_', ' ', '\t'])
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')', ' ', '\t']);
        if stripped == rest {
            break;
        }
        rest = stripped;
    }
    if !rest.get(..MARKER.len())?.eq_ignore_ascii_case(MARKER) {
        return None;
    }
    // Emphasis may close immediately after the marker: `**CITATION:** url`.
    Some(rest[MARKER.len()..].trim_start_matches(['*', '`', '_']))
}

/// Whether a line names a citation at all, however malformed. Crate-visible
/// because the fidelity verifier segments claims from the lines that are not
/// citations, and the two must agree on which lines those are.
pub(crate) fn names_a_citation(line: &str) -> bool {
    line.to_ascii_lowercase()
        .contains(MARKER.to_ascii_lowercase().as_str())
}

/// Judge one `CITATION:` line body (everything after the prefix).
fn judge(body: &str, parts: &[ContextPart]) -> Value {
    let Some((url_part, quote_part)) = body.split_once("::") else {
        return unparseable(body, "no `::` separates the URL from the quote");
    };
    let url = cited_url(url_part);
    if url.is_empty() {
        return unparseable(body, "the URL before `::` is empty");
    }
    let quote_part = quote_part.trim();
    let quote = match quote_part
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
    {
        Some(quote) if !quote.trim().is_empty() => quote.trim(),
        _ => return unparseable(body, "the quote is not enclosed in double quotes"),
    };

    // A line that repeats the directive's template cited nothing, so no
    // verdict about the window belongs on it — least of all `uncovered`,
    // which published the evaluator's own teaching material as a URL the
    // model invented.
    if url == URL_PLACEHOLDER {
        return template_echo(
            body,
            &format!(
                "the URL is the directive's own placeholder {URL_PLACEHOLDER}, repeated \
                 from the template instead of replaced with a source's URL"
            ),
        );
    }
    if quote == QUOTE_PLACEHOLDER {
        return template_echo(
            body,
            &format!(
                "the quote is the directive's own placeholder \"{QUOTE_PLACEHOLDER}\", \
                 repeated from the template instead of quoted from the source"
            ),
        );
    }

    // Two admitted envelopes can carry one URL with different retained text —
    // a page fetched by two providers, or twice. Every part under the cited
    // URL is a candidate, so a quote genuinely present in the window is found
    // wherever it sits, not only in whichever part was admitted first.
    let mut cited = parts
        .iter()
        .filter(|part| part.source_ref == url)
        .peekable();
    if cited.peek().is_none() {
        return json!({
            "url": url,
            "quote": quote,
            "verdict": "uncovered",
            "matched": Value::Null,
            "reason": "the cited URL is not among the sources that entered this plan's \
                       window, so the supplied context cannot back this citation",
        });
    }

    for part in cited {
        if let Some((start, end)) = find_quote(quote, &part.text) {
            return json!({
                "url": url,
                "quote": quote,
                "verdict": "supported",
                "matched": {
                    "content_hash": part.content_hash,
                    "start": start,
                    "end": end,
                },
                "reason": "the cited source's retained text contains this word sequence \
                           at the recorded span",
            });
        }
    }

    json!({
        "url": url,
        "quote": quote,
        "verdict": "contradicted",
        "matched": Value::Null,
        // What the evaluator saw, not why the model wrote it. The reader is
        // owed the observation; intent is not observable from here.
        "reason": "the cited source entered the window, but its retained text does not \
                   contain this word sequence",
    })
}

/// The URL a citation names, with markdown decoration and the window's own
/// labelling both taken off, in either order.
fn cited_url(url_part: &str) -> &str {
    undecorate_url(unlabel_url(undecorate_url(url_part)))
}

/// The URL under the window's labelling, when a model copied its header.
///
/// The window presents every part as `SOURCE <url> [<hash>]`
/// (`commonmeasure_inference::window_block`), and the 4 August 2026 live model copied
/// that header whole — label, hash and all — into its citations. The label
/// and the hash are the window's framing, not the source's identity, so both
/// come off before the parts lookup. The primary fix is upstream — the
/// directive's placeholder no longer shares a word with this label — and this
/// tolerance is the second lock on the same door, named in RULES because it
/// widens what the URL token admits.
fn unlabel_url(url: &str) -> &str {
    let url = url.strip_prefix("SOURCE ").map(str::trim).unwrap_or(url);
    match url.rsplit_once(char::is_whitespace) {
        Some((head, tail)) if tail.starts_with("[sha256:") && tail.ends_with(']') => {
            head.trim_end()
        }
        _ => url,
    }
}

/// The URL a citation names, with markdown decoration taken off.
///
/// A model writes URLs the way markdown writes them: autolinked in angle
/// brackets, or fenced in backticks. Read literally, the decoration becomes
/// part of the URL, and a citation of a source the window genuinely holds is
/// published as one the window never saw — while the same record's `inputs`
/// names that URL as a window part.
fn undecorate_url(url_part: &str) -> &str {
    let mut url = url_part.trim();
    loop {
        let inner = url
            .strip_prefix('`')
            .and_then(|inner| inner.strip_suffix('`'))
            .or_else(|| {
                url.strip_prefix('<')
                    .and_then(|inner| inner.strip_suffix('>'))
            });
        match inner {
            Some(inner) => url = inner.trim(),
            None => return url,
        }
    }
}

fn unparseable(body: &str, why: &str) -> Value {
    unreadable_line(&format!("{MARKER}{body}"), why)
}

/// A line that repeats the directive's template instead of citing. It parses,
/// but it names nothing: a placeholder stands where the source or the quote
/// should be, so there is no fact about the window to assert. The line is
/// carried verbatim, as every unavailable record carries what the model wrote.
fn template_echo(body: &str, why: &str) -> Value {
    json!({
        "url": Value::Null,
        "quote": Value::Null,
        "line": format!("{MARKER}{body}").trim(),
        "verdict": "unavailable",
        "matched": Value::Null,
        "reason": format!("the citation line echoes the directive's template ({why}), so \
                           nothing was cited"),
    })
}

/// A line that declares a citation the parser could not read as one. The line
/// is carried verbatim so a reader can see what the model actually wrote.
fn unreadable_line(line: &str, why: &str) -> Value {
    json!({
        "url": Value::Null,
        "quote": Value::Null,
        "line": line.trim(),
        "verdict": "unavailable",
        "matched": Value::Null,
        "reason": format!("the citation line does not parse ({why}), so nothing is checkable"),
    })
}

/// Find `quote` in `text`: words must match exactly, whitespace between them
/// is flexible, and punctuation hanging off the end of a word is ignored on
/// both sides. Returns byte offsets into the retained text, so a reader can
/// excise the span from the sealed source and see the quote.
///
/// Punctuation is ignored because a quote is cut out of running prose and the
/// cut lands where the quoter chose, not where the sentence ends. Comparing
/// whole words including their punctuation made "Article 53" a contradiction
/// of a window opening "Article 53: Obligations …", and turned a dropped
/// comma into a published accusation about a faithful citation.
///
/// Crate-visible because the coverage evaluator judges its rubric phrases
/// with exactly this matcher: one definition of "these words occur in the
/// window", however many evaluators lean on it.
pub(crate) fn find_quote(quote: &str, text: &str) -> Option<(usize, usize)> {
    let needle: Vec<&str> = quote.split_whitespace().collect();
    if needle.is_empty() {
        return None;
    }
    let haystack = words_with_offsets(text);
    haystack
        .windows(needle.len())
        .find(|window| {
            window
                .iter()
                .zip(&needle)
                .all(|((word, _, _), wanted)| same_word(word, wanted))
        })
        .map(|window| (window[0].1, window[window.len() - 1].2))
}

/// Whether the byte span `start..end` of `text` excises to exactly `quote`,
/// under the same word-sequence tolerance `supported` verdicts are judged
/// with: words compared case-insensitively, trailing punctuation ignored,
/// whitespace flexible. This is [`find_quote`]'s comparison replayed over a
/// recorded span, published so the dossier can recheck a span it restates
/// with the matcher that wrote it rather than a second definition that could
/// drift.
pub fn span_excises_to_quote(text: &str, start: usize, end: usize, quote: &str) -> bool {
    let Some(excised) = text.get(start..end) else {
        return false;
    };
    let excised: Vec<&str> = excised.split_whitespace().collect();
    let quoted: Vec<&str> = quote.split_whitespace().collect();
    !quoted.is_empty()
        && excised.len() == quoted.len()
        && excised
            .iter()
            .zip(&quoted)
            .all(|(word, wanted)| same_word(word, wanted))
}

/// Punctuation that ends a word without being part of it.
const TRAILING_PUNCTUATION: &[char] = &[
    '.', ',', ';', ':', '!', '?', '"', '\'', ')', ']', '}', '…', '”', '’', '»',
];

/// Two words, compared as words: identical but for case and what trails them.
///
/// Case is folded entirely. `supported` is a coarse presence check — this
/// word sequence entered the window — and the cost (Polish/polish) is
/// confessed in `BLIND_SPOTS`; fidelity-grade matching is the planned
/// add-on's business, not this core's.
pub(crate) fn same_word(left: &str, right: &str) -> bool {
    let left = trimmed_word(left);
    let right = trimmed_word(right);
    left == right || left.to_lowercase() == right.to_lowercase()
}

/// A word without its trailing punctuation — unless that is all it is, so a
/// stray dash is never equal to a stray comma.
fn trimmed_word(word: &str) -> &str {
    match word.trim_end_matches(TRAILING_PUNCTUATION) {
        "" => word,
        trimmed => trimmed,
    }
}

/// Every whitespace-delimited word of `text` with its byte span.
pub(crate) fn words_with_offsets(text: &str) -> Vec<(&str, usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(word_start) = start.take() {
                words.push((&text[word_start..index], word_start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(word_start) = start {
        words.push((&text[word_start..], word_start, text.len()));
    }
    words
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

    fn evaluate(answer: &str, parts: &[ContextPart]) -> Value {
        grounding(&EvaluationInput {
            answer: Some(answer),
            cited_answer_required: true,
            parts,
        })
    }

    #[test]
    fn a_true_quote_from_a_window_source_is_supported_with_its_span() {
        let parts = [part(
            "https://example.org/a",
            "The cap is set twice a year.\nIt applies per unit,   not per bill.",
        )];
        let answer = "It applies per unit.\nCITATION: https://example.org/a :: \"applies per unit,   not per bill.\"";
        let record = evaluate(answer, &parts);

        let citation = &record["citations"][0];
        assert_eq!(citation["verdict"], "supported");
        let (start, end) = (
            citation["matched"]["start"].as_u64().unwrap() as usize,
            citation["matched"]["end"].as_u64().unwrap() as usize,
        );
        // The recorded span excises to the quote, whitespace aside.
        let excised = &parts[0].text[start..end];
        assert_eq!(
            excised.split_whitespace().collect::<Vec<_>>(),
            ["applies", "per", "unit,", "not", "per", "bill."]
        );
        assert_eq!(record["verdict_counts"]["supported"], 1);
        assert_eq!(record["unevaluated"], Value::Null);
    }

    /// The published check and the public recheck are one comparison: a span
    /// `find_quote` records excises back to its quote through
    /// `span_excises_to_quote`, across the same case and punctuation
    /// tolerance — and a shifted span does not, so the recheck can actually
    /// fail.
    #[test]
    fn a_recorded_span_excises_back_to_its_quote_and_a_shifted_one_does_not() {
        let text = "Purpose: The cap is set twice a year, by Ofgem.";
        let quote = "the cap is set twice a year";
        let (start, end) = find_quote(quote, text).expect("the quote is present");
        assert!(span_excises_to_quote(text, start, end, quote));
        assert!(!span_excises_to_quote(text, start + 1, end, quote));
        assert!(!span_excises_to_quote(text, 0, end, quote));
        assert!(!span_excises_to_quote(text, start, text.len() + 1, quote));
    }

    #[test]
    fn a_quote_the_cited_source_does_not_contain_is_contradicted() {
        let parts = [part(
            "https://example.org/a",
            "The cap is set twice a year.",
        )];
        let record = evaluate(
            "CITATION: https://example.org/a :: \"the cap never changes\"",
            &parts,
        );
        assert_eq!(record["citations"][0]["verdict"], "contradicted");
        assert_eq!(record["citations"][0]["matched"], Value::Null);
    }

    #[test]
    fn a_url_outside_the_window_is_uncovered() {
        let parts = [part(
            "https://example.org/a",
            "The cap is set twice a year.",
        )];
        let record = evaluate(
            "CITATION: https://elsewhere.example :: \"the cap is set\"",
            &parts,
        );
        assert_eq!(record["citations"][0]["verdict"], "uncovered");
    }

    #[test]
    fn an_unparseable_citation_line_is_unavailable_not_a_crash() {
        let record = evaluate("CITATION: this line has no separator or quote", &[]);
        assert_eq!(record["citations"][0]["verdict"], "unavailable");
        assert!(
            record["citations"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("does not parse")
        );
    }

    #[test]
    fn word_matching_is_flexible_in_whitespace_but_not_in_words() {
        let parts = [part("u", "per  unit,\nnot per bill")];
        for quote in ["unit, not", "unit not"] {
            assert_eq!(
                evaluate(&format!("CITATION: u :: \"{quote}\""), &parts)["citations"][0]["verdict"],
                "supported",
                "{quote:?} is the source's own words, whitespace and punctuation aside"
            );
        }
        // Punctuation is not the word. A different word is still a different
        // word, and part of a word is not a word.
        for quote in ["unit or not", "uni not", "per units"] {
            assert_eq!(
                evaluate(&format!("CITATION: u :: \"{quote}\""), &parts)["citations"][0]["verdict"],
                "contradicted",
                "{quote:?} is not what the source says"
            );
        }
    }

    #[test]
    fn a_suite_that_did_not_require_citations_is_unevaluated_with_the_reason() {
        let record = grounding(&EvaluationInput {
            answer: Some("an answer"),
            cited_answer_required: false,
            parts: &[],
        });
        assert!(
            record["unevaluated"]
                .as_str()
                .unwrap()
                .contains("did not require a cited answer")
        );
        assert_eq!(record["citations"].as_array().unwrap().len(), 0);
        assert_eq!(record["evaluator"]["name"], "grounding");
    }

    #[test]
    fn a_plan_without_an_answer_is_unevaluated_with_the_reason() {
        let record = grounding(&EvaluationInput {
            answer: None,
            cited_answer_required: true,
            parts: &[],
        });
        assert!(
            record["unevaluated"]
                .as_str()
                .unwrap()
                .contains("no answer exists")
        );
    }

    #[test]
    fn an_answer_that_declares_no_citations_is_evaluated_with_zero_counts() {
        let record = evaluate("An answer with no citation lines at all.", &[]);
        assert_eq!(record["unevaluated"], Value::Null);
        assert_eq!(record["verdict_counts"]["supported"], 0);
        assert_eq!(record["verdict_counts"]["unavailable"], 0);
    }
    /// A model formats a citation list the way it formats any list. Reading
    /// only a bare line-leading marker meant an ordinary markdown list was
    /// seen as no citations at all, and an answer that fabricated a source
    /// published with every verdict count at zero.
    #[test]
    fn a_decorated_citation_marker_is_still_read() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        for line in [
            "CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "  CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "- CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "* CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "+ CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "1. CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "> CITATION: https://example.org/a :: \"the cap is set twice a year\"",
            "**CITATION:** https://example.org/a :: \"the cap is set twice a year\"",
            "`CITATION:` https://example.org/a :: \"the cap is set twice a year\"",
            "citation: https://example.org/a :: \"the cap is set twice a year\"",
            "Citation: https://example.org/a :: \"the cap is set twice a year\"",
        ] {
            let record = evaluate(&format!("The sources establish this.\n{line}\n"), &parts);
            assert_eq!(
                record["citations"].as_array().map(Vec::len),
                Some(1),
                "no citation read from {line:?}"
            );
            assert_eq!(
                record["verdict_counts"]["supported"], 1,
                "{line:?} was read but not supported"
            );
        }
    }

    /// The failure that made the drop dangerous: an answer citing a source
    /// that never entered the window must never publish as clean.
    #[test]
    fn a_fabricated_citation_in_a_bullet_list_is_reported() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        let answer = "The sources establish this.\n\
             - CITATION: https://example.org/a :: \"the cap is set twice a year\"\n\
             - CITATION: https://fabricated.example/nowhere :: \"words no source has\"\n";
        let record = evaluate(answer, &parts);
        assert_eq!(record["citations"].as_array().map(Vec::len), Some(2));
        assert_eq!(record["verdict_counts"]["supported"], 1);
        assert_eq!(record["verdict_counts"]["uncovered"], 1);
        assert!(
            record["unevaluated"].is_null(),
            "the plan was evaluated, so no unevaluated reason belongs on it"
        );
    }

    /// A line the parser cannot read is unavailable, never silently absent:
    /// all-zero counts assert "checked, nothing to report".
    #[test]
    fn a_line_naming_a_citation_it_cannot_present_is_unavailable() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        let record = evaluate("See the CITATION: mentioned above.\n", &parts);
        assert_eq!(record["citations"].as_array().map(Vec::len), Some(1));
        assert_eq!(record["verdict_counts"]["unavailable"], 1);
        assert_eq!(record["citations"][0]["verdict"], "unavailable");
        assert_eq!(
            record["citations"][0]["line"],
            "See the CITATION: mentioned above."
        );
    }

    /// Markdown autolinks a bare URL by wrapping it in angle brackets, and a
    /// model fences one in backticks. Read literally, that decoration made a
    /// citation of an admitted source read as uncovered — while the record's
    /// own `inputs` named the same URL as a window part.
    #[test]
    fn a_url_wearing_markdown_decoration_still_names_its_source() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        for url in [
            "https://example.org/a",
            "<https://example.org/a>",
            "`https://example.org/a`",
            "`<https://example.org/a>`",
            "< https://example.org/a >",
        ] {
            let record = evaluate(
                &format!("CITATION: {url} :: \"the cap is set twice a year\""),
                &parts,
            );
            let citation = &record["citations"][0];
            assert_eq!(citation["verdict"], "supported", "{url} was not resolved");
            assert_eq!(
                citation["url"], "https://example.org/a",
                "the record must name the source it looked up, as `inputs` does"
            );
        }
    }

    /// The directive is what the model is taught to imitate, so it must not
    /// print a shape this parser then refuses. A placeholder in angle brackets
    /// taught exactly the decoration that read as uncovered.
    #[test]
    fn the_directive_teaches_only_a_form_the_parser_reads() {
        let template = CITATION_DIRECTIVE
            .lines()
            .find(|line| line.trim_start().starts_with(MARKER))
            .expect("the directive shows the citation form it asks for");
        let body = citation_body(template).expect("the parser reads its own template");
        let (url, quote) = body.split_once("::").expect("the template shows `::`");
        assert_eq!(
            cited_url(url),
            url.trim(),
            "the directive prints a URL placeholder the parser would strip, so a model \
             copying its shape writes a URL this evaluator cannot resolve"
        );
        // The echo check knows the template by these two constants, so the
        // directive and the check must not drift apart.
        assert_eq!(
            url.trim(),
            URL_PLACEHOLDER,
            "the template's URL token is the placeholder the echo check looks for"
        );
        assert_eq!(
            quote.trim().trim_matches('"'),
            QUOTE_PLACEHOLDER,
            "the template's quote is the placeholder the echo check looks for"
        );
    }

    /// The seam itself, crossed in one test: a part rendered by the production
    /// window renderer in `commonmeasure-inference`, its header handed to this parser.
    /// The 4 August 2026 run crossed it unwatched — the directive's
    /// placeholder was `SOURCE_URL`, the window's label was `SOURCE`, and a
    /// model reading both wrote the label into the URL token of every
    /// citation. Neither crate's tests could see the collision alone.
    #[test]
    fn the_rendered_window_header_and_the_directive_cross_the_seam_intact() {
        let source = part("https://example.org/a", "the cap is set twice a year");
        let rendered = commonmeasure_inference::window_block(&source);
        let header = rendered
            .lines()
            .next()
            .expect("the rendered block opens with its header line");

        // No token of the rendered header is the directive's placeholder, so
        // a template echo can never read as a fact about the window.
        assert!(
            !header
                .split_whitespace()
                .any(|token| token == URL_PLACEHOLDER),
            "the window header {header:?} carries the directive's placeholder"
        );

        // And every URL-bearing shape a model can copy out of that header —
        // the whole header, the labelled URL, the URL with its hash, the bare
        // URL — resolves to the part it came from, never to a URL the
        // evaluator fails to find.
        let url = &source.source_ref;
        let hash = &source.content_hash;
        for token in [
            header.to_owned(),
            format!("SOURCE {url}"),
            format!("{url} [{hash}]"),
            url.clone(),
        ] {
            let record = evaluate(
                &format!("CITATION: {token} :: \"the cap is set\""),
                std::slice::from_ref(&source),
            );
            let citation = &record["citations"][0];
            assert_eq!(
                citation["verdict"], "supported",
                "{token:?} did not resolve against the window"
            );
            assert_eq!(
                citation["url"],
                source.source_ref.as_str(),
                "the record must name the source its own `inputs` names"
            );
        }
    }

    /// A model that repeats the directive's template cited nothing.
    /// `uncovered` asserts a fact about the window — the harshest verdict in
    /// the vocabulary, and it was published about a placeholder this evaluator
    /// itself taught.
    #[test]
    fn a_template_echo_is_unavailable_never_uncovered() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        let template = CITATION_DIRECTIVE
            .lines()
            .find(|line| line.trim_start().starts_with(MARKER))
            .expect("the directive shows the citation form it asks for");
        let record = evaluate(template, &parts);
        assert_eq!(record["citations"].as_array().map(Vec::len), Some(1));
        assert_eq!(record["verdict_counts"]["uncovered"], 0);
        let citation = &record["citations"][0];
        assert_eq!(citation["verdict"], "unavailable");
        assert!(
            citation["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(URL_PLACEHOLDER)),
            "the reason must name the placeholder the model repeated"
        );

        // A real admitted URL beside the template quote is still an echo:
        // no quote was written, so neither `contradicted` nor `supported`
        // is an observation anything backs.
        let record = evaluate(
            &format!("CITATION: https://example.org/a :: \"{QUOTE_PLACEHOLDER}\""),
            &parts,
        );
        assert_eq!(record["verdict_counts"]["unavailable"], 1);
        assert_eq!(record["verdict_counts"]["contradicted"], 0);
        assert!(
            record["citations"][0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("placeholder")),
        );
    }

    /// Lowercasing a quote's first word to seat it mid-sentence is ordinary
    /// prose, and 0.3.0 published it as a contradiction. 0.4.0 folds case
    /// entirely: supported is a presence check, and case-as-meaning belongs
    /// to the fidelity add-on (see BLIND_SPOTS).
    #[test]
    fn case_alone_never_costs_a_match() {
        let parts = [part(
            "u",
            "Purpose: The AI Act Code of Practice is a set of guidelines.",
        )];
        for quote in [
            "the AI Act Code of Practice",
            "THE AI ACT CODE OF PRACTICE",
            "The ai act code of practice",
        ] {
            assert_eq!(
                evaluate(&format!("CITATION: u :: \"{quote}\""), &parts)["citations"][0]["verdict"],
                "supported",
                "{quote:?} differs from the source only in case"
            );
        }
        // Case-insensitive is not word-insensitive.
        assert_eq!(
            evaluate("CITATION: u :: \"the AI Act Codes of Practice\"", &parts)["citations"][0]["verdict"],
            "contradicted",
            "a different word is still a different word, whatever its case"
        );
    }

    /// A quote is cut out of running prose, and the cut lands where the quoter
    /// chose. Judging punctuation as part of the word made a faithful citation
    /// of a heading, or one that dropped a trailing comma, into a published
    /// accusation.
    #[test]
    fn punctuation_at_a_word_boundary_is_not_a_misattribution() {
        let heading = [part(
            "u",
            "Article 53: Obligations for providers of general-purpose AI models | AI Act \
             Service Desk",
        )];
        assert_eq!(
            evaluate("CITATION: u :: \"Article 53\"", &heading)["citations"][0]["verdict"],
            "supported"
        );

        let prose = [part(
            "u",
            "The cap is set twice a year. It applies per unit, not per bill.",
        )];
        for quote in [
            "It applies per unit, not per bill.",
            "It applies per unit not per bill",
            "The cap is set twice a year..",
            "The cap is set twice a year",
        ] {
            assert_eq!(
                evaluate(&format!("CITATION: u :: \"{quote}\""), &prose)["citations"][0]["verdict"],
                "supported",
                "{quote:?} differs from the source only in punctuation"
            );
        }
    }

    /// Contradicted is an observation about text, and the record must read as
    /// one. The evaluator sees a word sequence absent; it cannot see intent.
    #[test]
    fn a_contradicted_record_states_the_observation_not_an_intent() {
        let parts = [part("u", "The cap is set twice a year.")];
        let record = evaluate("CITATION: u :: \"the cap never changes\"", &parts);
        let reason = record["citations"][0]["reason"].as_str().expect("a reason");
        assert_eq!(record["citations"][0]["verdict"], "contradicted");
        assert!(
            reason.contains("does not contain this word sequence"),
            "the reason must state what was observed: {reason}"
        );
        assert!(
            !reason.contains("misattribut"),
            "the reason must not accuse the model of intent: {reason}"
        );
        assert!(
            BLIND_SPOTS
                .iter()
                .any(|spot| spot.contains("trailing punctuation")),
            "the limit the matcher still has must be named on every record"
        );
    }

    /// One URL can enter the window twice with different retained text — the
    /// same page fetched by two providers. Stopping at the first part
    /// published a quote verbatim in the window as contradicted.
    #[test]
    fn a_quote_is_found_in_any_window_part_carrying_the_cited_url() {
        let parts = [
            part("https://ex.org/d", "alpha beta gamma"),
            part("https://ex.org/d", "delta epsilon zeta"),
        ];
        let record = evaluate("CITATION: https://ex.org/d :: \"delta epsilon\"", &parts);
        let citation = &record["citations"][0];
        assert_eq!(citation["verdict"], "supported");
        assert_eq!(
            citation["matched"]["content_hash"], parts[1].content_hash,
            "the record must name the part the quote was actually found in"
        );
    }

    /// A gateway may return `content: ""`. Scored, that published a plan
    /// completed with four zero counts — a measured zero standing in for
    /// nothing having been produced.
    #[test]
    fn an_empty_answer_is_unevaluated_rather_than_scored() {
        for answer in ["", "   \n\t"] {
            let record = grounding(&EvaluationInput {
                answer: Some(answer),
                cited_answer_required: true,
                parts: &[],
            });
            assert!(
                record["unevaluated"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("the answer is empty")),
                "{answer:?} was scored instead of declared uncheckable"
            );
            assert_eq!(record["citations"].as_array().map(Vec::len), Some(0));
        }
    }

    /// Two reasons can hold at once; the reader is owed the more fundamental.
    /// Whether a suite asked for citations matters less than whether anything
    /// came back to check.
    #[test]
    fn a_plan_with_no_answer_is_told_that_before_the_suite_setting() {
        let record = grounding(&EvaluationInput {
            answer: None,
            cited_answer_required: false,
            parts: &[],
        });
        assert!(
            record["unevaluated"]
                .as_str()
                .is_some_and(|reason| reason.contains("no answer exists")),
            "the absent answer is the more fundamental of the two facts"
        );
    }

    /// Nothing derives the sealed rule text from the code, so a behaviour
    /// change can leave it standing still and two runs that judged differently
    /// share one digest. These pins make that a failure, not a publication.
    #[test]
    fn the_rule_text_and_the_digest_move_with_the_evaluator_version() {
        assert!(
            RULES.starts_with(&format!("{EVALUATOR_NAME}/{EVALUATOR_VERSION}:")),
            "the rule text must name the version it describes"
        );
        assert_eq!(
            configuration_digest(),
            "sha256:4ea522440e2a930482325a0466347643b221853aa339ab4dc4d77cb48356e0e3",
            "the rule text or the directive moved. That is a change of experiment \
             identity: bump EVALUATOR_VERSION, update this pin in the same commit, and \
             expect sealed runs recorded under the old digest to be incomparable"
        );
    }

    /// Prose that never names a citation is not a malformed one.
    #[test]
    fn ordinary_prose_declares_no_citations() {
        let parts = [part("https://example.org/a", "the cap is set twice a year")];
        let record = evaluate("The sources establish this, plainly.\n", &parts);
        assert_eq!(record["citations"].as_array().map(Vec::len), Some(0));
    }
}
