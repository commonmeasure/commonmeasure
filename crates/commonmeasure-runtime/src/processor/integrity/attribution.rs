//! "Right person quoted?" as a lookup. Where a summary sentence attributes a
//! quotation to a named speaker, the quotation is found verbatim in the
//! passage and the passage's own attribution of it is read from the sentence
//! that carries it and its neighbours. The answer is yes where the summary's
//! speaker is named there, no where the quotation is in the passage but the
//! speaker is not, and not applicable where the sentence quotes nobody.
//!
//! Attribution in prose is a pattern, so the stage is a heuristic and says
//! so: it reads `X said`, `said X`, `according to X` and their common
//! variants, with the speaker as a run of capitalised words. A paraphrased
//! attribution ("the minister said that…") carries no quotation and is left
//! to the supported question.

use commonmeasure_types::canonical::sha256_digest;
use serde_json::json;

use super::words::{Word, find_run, key, words};
use super::{Answer, Case, Label, Question};

pub const NAME: &str = "attribution-lookup/1";

/// A quotation shorter than this is a phrase any sentence may share.
const MINIMUM_QUOTE_WORDS: usize = 3;

const SPEECH_VERBS: &[&str] = &[
    "said",
    "says",
    "say",
    "told",
    "tells",
    "wrote",
    "writes",
    "added",
    "adds",
    "stated",
    "states",
    "argued",
    "argues",
    "warned",
    "warns",
    "claimed",
    "claims",
    "announced",
    "announces",
    "insisted",
    "insists",
    "explained",
    "explains",
    "noted",
    "notes",
    "described",
    "describes",
    "called",
    "calls",
    "declared",
    "declares",
    "replied",
    "responded",
    "continued",
    "asked",
    "asks",
    "urged",
    "urges",
    "admitted",
    "admits",
    "confirmed",
    "confirms",
    "denied",
    "denies",
    "promised",
    "promises",
    "conceded",
    "suggested",
    "suggests",
    "maintained",
    "maintains",
    "recalled",
    "recalls",
    "reported",
    "reports",
    "put",
    "puts",
];

/// Words that start a name but are not one on their own.
const TITLES: &[&str] = &[
    "mr",
    "mrs",
    "ms",
    "dr",
    "sir",
    "dame",
    "lord",
    "lady",
    "professor",
    "prof",
    "president",
    "prime",
    "minister",
    "mayor",
    "councillor",
    "senator",
    "judge",
    "chief",
    "captain",
    "general",
    "the",
    "a",
    "an",
];

/// A quotation in the summary with the speaker the summary gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quotation {
    pub text: String,
    /// Byte offsets of `text` in the sentence it was read from.
    pub start: usize,
    pub end: usize,
    pub speaker: Option<Speaker>,
}

/// Quotations in `sentence`: text between double quotation marks, plain or
/// typographic, of at least `MINIMUM_QUOTE_WORDS` words.
pub fn quotations(sentence: &str) -> Vec<Quotation> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = sentence.char_indices().collect();
    let mut open: Option<usize> = None;
    for &(index, ch) in &chars {
        match ch {
            '"' | '\u{201C}' if open.is_none() => open = Some(index + ch.len_utf8()),
            '"' | '\u{201D}' => {
                if let Some(start) = open.take() {
                    let raw = &sentence[start..index];
                    let text = raw.trim();
                    if words(text).len() >= MINIMUM_QUOTE_WORDS {
                        let before = &sentence[..start];
                        let after = &sentence[index + ch.len_utf8()..];
                        let lead = raw.len() - raw.trim_start().len();
                        out.push(Quotation {
                            text: text.to_owned(),
                            start: start + lead,
                            end: start + lead + text.len(),
                            speaker: speaker_near(before, after),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Verbs whose subject may follow them in a quotation tag (`"…", said X`).
/// `told` is left out: the name after it is the listener.
const INVERTIBLE_VERBS: &[&str] = &[
    "said",
    "says",
    "added",
    "adds",
    "wrote",
    "writes",
    "stated",
    "states",
    "argued",
    "argues",
    "warned",
    "warns",
    "claimed",
    "claims",
    "announced",
    "insisted",
    "explained",
    "noted",
    "declared",
    "replied",
    "responded",
    "continued",
    "asked",
    "urged",
    "admitted",
    "confirmed",
    "denied",
    "promised",
    "conceded",
    "suggested",
    "maintained",
    "recalled",
    "reported",
    "commented",
    "remarked",
    "observed",
];

const PRONOUNS: &[&str] = &["he", "she", "they", "it", "who", "i", "we"];

/// Words that join a clause to the sentence's subject: `X disagreed, and
/// said "…"`.
const JOINERS: &[&str] = &["and", "but", "then", "later", "also", "who", "which"];

/// What a reading of the words around a quotation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Speaker {
    Named(String),
    /// The quotation is attributed by a pronoun the lookup cannot resolve.
    Pronoun,
}

fn is_verb(word: &str) -> bool {
    SPEECH_VERBS.contains(&key(word).as_str())
}

/// The speaker a sentence names around a quotation: `X said "…"`,
/// `X told Y: "…"`, `according to X, "…"`, `"…", X said`, `"…", said X`,
/// `"…", according to X`. A pronoun beside the verb is reported as such.
pub fn speaker_near(before: &str, after: &str) -> Option<Speaker> {
    let b = words(before);
    let a = words(after);
    // `according to X,` just before the quotation, or at the start of what
    // follows it. Further back it attributes something else.
    for (ws, from, limit) in [(&b, b.len().saturating_sub(6), b.len()), (&a, 0, 4usize)] {
        if let Some(to_at) = ws
            .iter()
            .enumerate()
            .filter(|(i, w)| *i >= from && *i <= limit && key(w.text) == "to")
            .map(|(i, _)| i)
            .find(|&i| i > 0 && key(ws[i - 1].text) == "according")
        {
            if let Some(name) = name_starting_at(ws, to_at + 1) {
                return Some(Speaker::Named(name));
            }
            if ws
                .get(to_at + 1)
                .is_some_and(|w| PRONOUNS.contains(&key(w.text).as_str()))
            {
                return Some(Speaker::Pronoun);
            }
        }
    }
    // The last speech verb before the quotation, with its subject before it.
    for verb_at in (0..b.len()).rev().filter(|&i| is_verb(b[i].text)) {
        if let Some(name) = name_ending_at(&b, verb_at) {
            return Some(Speaker::Named(name));
        }
        let previous = verb_at.checked_sub(1).map(|i| key(b[i].text));
        if previous.as_deref().is_some_and(|w| PRONOUNS.contains(&w)) {
            return Some(Speaker::Pronoun);
        }
        if previous.as_deref().is_some_and(|w| JOINERS.contains(&w)) {
            // The clause shares the sentence's subject: its leading name.
            if let Some(name) = name_starting_at(&b, 0) {
                return Some(Speaker::Named(name));
            }
            if b.first()
                .is_some_and(|w| PRONOUNS.contains(&key(w.text).as_str()))
            {
                return Some(Speaker::Pronoun);
            }
        }
    }
    // A quotation tag after the quotation: `", X said` or `", said X`.
    if let Some(verb_at) = a.iter().take(5).position(|w| is_verb(w.text)) {
        if let Some(name) = name_ending_at(&a, verb_at) {
            return Some(Speaker::Named(name));
        }
        if a[..verb_at]
            .iter()
            .any(|w| PRONOUNS.contains(&key(w.text).as_str()))
        {
            return Some(Speaker::Pronoun);
        }
        if INVERTIBLE_VERBS.contains(&key(a[verb_at].text).as_str()) {
            if let Some(name) = name_starting_at(&a, verb_at + 1) {
                return Some(Speaker::Named(name));
            }
            if a.get(verb_at + 1)
                .is_some_and(|w| PRONOUNS.contains(&key(w.text).as_str()))
            {
                return Some(Speaker::Pronoun);
            }
        }
    }
    None
}

/// Honorifics and post-nominals that follow a name and are not its surname.
const SUFFIXES: &[&str] = &[
    "mp", "qc", "kc", "jr", "sr", "obe", "mbe", "cbe", "phd", "md",
];

/// The surname of a speaker as the lookup compares it.
pub fn surname(name: &str) -> String {
    name.split_whitespace()
        .map(key)
        .rev()
        .find(|w| !SUFFIXES.contains(&w.as_str()))
        .unwrap_or_default()
}

fn is_name_word(word: &str) -> bool {
    let k = key(word);
    !k.is_empty()
        && word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
        && !TITLES.contains(&k.as_str())
        && !PRONOUNS.contains(&k.as_str())
}

/// The run of name words ending just before word `end`, not crossing a
/// comma or a clause boundary on the way back.
fn name_ending_at(ws: &[Word<'_>], end: usize) -> Option<String> {
    let mut start = end;
    while start > 0 && is_name_word(ws[start - 1].text) {
        start -= 1;
        if ws[start].text.ends_with([',', ':', ';']) && start < end - 1 {
            // `Stepping onto the Moon, Armstrong said`: the run stops at
            // the comma, keeping only the words after it.
            start += 1;
            break;
        }
    }
    (start < end).then(|| join(&ws[start..end]))
}

/// The run of name words starting at word `start`, titles skipped.
fn name_starting_at(ws: &[Word<'_>], mut start: usize) -> Option<String> {
    while start < ws.len() && TITLES.contains(&key(ws[start].text).as_str()) {
        start += 1;
    }
    let mut end = start;
    while end < ws.len() && is_name_word(ws[end].text) {
        end += 1;
        if ws[end - 1].text.ends_with([',', ':', ';', '.']) {
            break;
        }
    }
    (start < end).then(|| join(&ws[start..end]))
}

fn join(ws: &[Word<'_>]) -> String {
    ws.iter()
        .map(|w| w.text.trim_matches(|c: char| !c.is_alphanumeric()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A quotation as the record keeps it: its span in the sentence and the hash
/// of its text, never the words.
fn quote_ref(quote: &Quotation) -> serde_json::Value {
    json!({
        "start": quote.start,
        "end": quote.end,
        "sha256": sha256_digest(quote.text.as_bytes()),
    })
}

/// Byte spans of the passage's sentences: split where `.`, `!` or `?` is
/// followed by whitespace, closing quotes kept with the sentence.
pub fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?') {
            let mut j = i + 1;
            // Closing quotes and brackets stay with the sentence.
            while j < bytes.len() {
                let rest = &text[j..];
                let Some(c) = rest.chars().next() else { break };
                if matches!(c, '"' | '\u{201D}' | '\'' | ')' | ']') {
                    j += c.len_utf8();
                } else {
                    break;
                }
            }
            if j >= bytes.len() || (bytes[j] as char).is_whitespace() {
                out.push((start, j));
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

/// The answer to "right person quoted?" for the case's sentence against the
/// case's text.
pub fn check(case: &Case) -> Answer {
    let quotes = quotations(&case.summary);
    let passage_words = words(&case.passage);
    let bounds = sentences(&case.passage);
    let mut findings = Vec::new();
    let mut verdict: Option<(Label, String)> = None;
    let mut pronoun = false;
    for quote in &quotes {
        let qw = words(&quote.text);
        let Some(at) = find_run(&qw, &passage_words) else {
            findings.push(json!({ "quote": quote_ref(quote), "in_passage": false }));
            verdict.get_or_insert((
                Label::No,
                "the quotation does not occur in the passage".to_owned(),
            ));
            continue;
        };
        let quote_start = passage_words[at].start;
        let quote_end = passage_words[at + qw.len() - 1].end;
        let sentence_index = bounds
            .iter()
            .position(|&(s, e)| quote_start >= s && quote_start < e)
            .unwrap_or(0);
        // A quotation may run over a sentence boundary of its own; the
        // carrying span runs from the sentence it starts in to the one
        // it ends in.
        let end_index = bounds
            .iter()
            .position(|&(s, e)| quote_end > s && quote_end <= e)
            .unwrap_or(sentence_index)
            .max(sentence_index);
        let (lo, mut hi) = (bounds[sentence_index].0, bounds[end_index].1);
        // A quotation that closes its sentence is often followed by its
        // tag as the next: `"…." said X.` The tag is read with it.
        if case.passage[quote_end..hi]
            .trim_matches(|c: char| !c.is_alphanumeric())
            .is_empty()
            && let Some(&(_, next_end)) = bounds.get(end_index + 1)
        {
            hi = next_end;
        }
        let Some(Speaker::Named(speaker)) = &quote.speaker else {
            let reason = match quote.speaker {
                Some(Speaker::Pronoun) => "the sentence attributes the quotation by pronoun",
                _ => "the sentence quotes without naming a speaker",
            };
            findings.push(json!({
                "quote": quote_ref(quote),
                "in_passage": true,
                "passage_span": { "start": quote_start, "end": quote_end },
                "speaker": null,
                "reason": reason,
            }));
            continue;
        };
        let wanted = surname(speaker);
        // The passage's own attribution: the speaker named beside the
        // quotation in the sentence that carries it. A pronoun there is
        // left unresolved and recorded; where the sentence attributes
        // neither way, the previous sentence is read for the name and
        // the record says the match was by proximity.
        let attributed = speaker_near(&case.passage[lo..quote_start], &case.passage[quote_end..hi]);
        let (named, how) = match &attributed {
            Some(Speaker::Named(name)) => (Some(surname(name) == wanted), "passage attribution"),
            Some(Speaker::Pronoun) => (None, "pronoun in the passage"),
            None => {
                let from = bounds[sentence_index.saturating_sub(1)].0;
                let context = &case.passage[from..hi];
                (
                    Some(words(context).iter().any(|w| key(w.text) == wanted)),
                    "name near the quotation",
                )
            }
        };
        findings.push(json!({
            "quote": quote_ref(quote),
            "in_passage": true,
            "passage_span": { "start": quote_start, "end": quote_end },
            "speaker_sha256": sha256_digest(speaker.as_bytes()),
            "passage_attributes_to": attributed.as_ref().map(|s| match s {
                Speaker::Named(n) => json!({ "named_sha256": sha256_digest(n.as_bytes()) }),
                Speaker::Pronoun => json!("pronoun"),
            }),
            "sentence": { "start": lo, "end": hi },
            "matched_by": how,
            "speaker_named": named,
        }));
        let Some(named) = named else {
            pronoun = true;
            continue;
        };
        let (label, reason) = if named {
            (
                Label::Yes,
                "the passage names the sentence's speaker beside the quotation".to_owned(),
            )
        } else {
            (
                Label::No,
                "the passage does not name the sentence's speaker beside the quotation".to_owned(),
            )
        };
        match &verdict {
            Some((Label::No, _)) => {}
            _ => verdict = Some((label, reason)),
        }
    }
    let (label, probability, reason) = match verdict {
        Some((label, reason)) => {
            let p = match label {
                Label::Yes => Some(1.0),
                _ => Some(0.0),
            };
            (label, p, reason)
        }
        None if quotes.is_empty() => (
            Label::NotApplicable,
            None,
            "the sentence quotes nobody".to_owned(),
        ),
        None if pronoun => (
            Label::NotApplicable,
            None,
            "the passage attributes the quotation by a pronoun the lookup cannot resolve"
                .to_owned(),
        ),
        None => (
            Label::NotApplicable,
            None,
            "the sentence quotes without naming a speaker".to_owned(),
        ),
    };
    Answer {
        question: Question::RightPersonQuoted,
        label,
        probability,
        basis: NAME.to_owned(),
        reason,
        detail: json!({ "quotations": findings }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSAGE: &str = "The report was published on Monday. \
        \"The figures speak for themselves and the council must act,\" said Jane Okafor, \
        the committee's chair. Tom Reilly, the council leader, disagreed: \"We have acted \
        and we will keep acting.\" The meeting ended at noon.";

    fn case(summary: &str) -> Case {
        Case {
            id: "t".into(),
            passage: PASSAGE.into(),
            summary: summary.into(),
            markers: Vec::new(),
        }
    }

    #[test]
    fn reads_speakers_from_summaries() {
        let named = |s: &str| Some(Speaker::Named(s.to_owned()));
        let q = quotations("Jane Okafor said \"the figures speak for themselves\".");
        assert_eq!(q[0].speaker, named("Jane Okafor"));
        let q = quotations("\"We have acted and we will keep acting,\" Reilly said.");
        assert_eq!(q[0].speaker, named("Reilly"));
        let q = quotations("According to Mr Reilly, \"we have acted and we will keep acting\".");
        assert_eq!(q[0].speaker, named("Reilly"));
        let q = quotations("Stepping onto the Moon, Armstrong said: \"one small step for a man\".");
        assert_eq!(q[0].speaker, named("Armstrong"));
        let q = quotations("Hawking told the examiners: \"I expect you will give me a First\".");
        assert_eq!(q[0].speaker, named("Hawking"));
        let q = quotations("\"I would rather be right than rigorous\", he told Kip Thorne.");
        assert_eq!(q[0].speaker, Some(Speaker::Pronoun));
        let q =
            quotations("\"That is not a great look for the Government\" said Marion Fellows MP.");
        assert_eq!(q[0].speaker, named("Marion Fellows MP"));
        assert_eq!(surname("Marion Fellows MP"), "fellows");
    }

    #[test]
    fn the_right_speaker_is_yes() {
        let a = check(&case("Jane Okafor said \"the council must act\"."));
        assert_eq!(a.label, Label::Yes);
    }

    #[test]
    fn the_wrong_speaker_is_no() {
        let a = check(&case("Tom Reilly said \"the council must act\"."));
        assert_eq!(a.label, Label::No);
    }

    #[test]
    fn no_quotation_is_not_applicable() {
        let a = check(&case("The report was published on Monday."));
        assert_eq!(a.label, Label::NotApplicable);
    }

    #[test]
    fn a_clause_joined_to_the_subject_attributes_to_it() {
        let passage = "Aldrin and Collins described the crew as \"amiable strangers\". Armstrong \
                       did not agree with the assessment, and said \"all the crews I was on \
                       worked very well together.\"";
        let mut c = Case {
            id: "t".into(),
            passage: passage.into(),
            summary: "Collins said \"all the crews I was on worked very well together.\"".into(),
            markers: Vec::new(),
        };
        assert_eq!(check(&c).label, Label::No);
        c.summary = "Armstrong said \"all the crews I was on worked very well together.\"".into();
        assert_eq!(check(&c).label, Label::Yes);
    }

    #[test]
    fn a_tag_after_the_closing_sentence_is_read() {
        let passage = "Tom Cooper had resigned. \"That is not a great look for the Government.\" \
                       said Marion Fellows MP. Writing in the paper, Marina Hyde disagreed.";
        let mut c = Case {
            id: "t".into(),
            passage: passage.into(),
            summary: "\"That is not a great look for the Government.\" said Marion Fellows MP."
                .into(),
            markers: Vec::new(),
        };
        assert_eq!(check(&c).label, Label::Yes);
        c.summary = "\"That is not a great look for the Government.\" said Tom Cooper.".into();
        assert_eq!(check(&c).label, Label::No);
    }

    #[test]
    fn a_pronoun_in_the_passage_is_not_resolved() {
        let passage = "When asked whether he believes in God, he said: \"Not in the sense of \
                       most people. I'm atheist and Unitarian Universalist.\"";
        let c = Case {
            id: "t".into(),
            passage: passage.into(),
            summary: "Berners-Lee said: \"Not in the sense of most people. I'm atheist and \
                      Unitarian Universalist.\""
                .into(),
            markers: Vec::new(),
        };
        let a = check(&c);
        assert_eq!(a.label, Label::NotApplicable);
        assert!(a.reason.contains("pronoun"));
    }

    #[test]
    fn a_quotation_over_a_sentence_boundary_is_read_whole() {
        let passage = "Sullenberger said later: \"It was very quiet as we worked, my copilot \
            Jeff Skiles and I. We were a team.\" He left the aircraft last.";
        let c = Case {
            id: "t".into(),
            passage: passage.into(),
            summary: "Jeff Skiles said: \"It was very quiet as we worked, my copilot Jeff \
                      Skiles and I. We were a team.\""
                .into(),
            markers: Vec::new(),
        };
        let a = check(&c);
        assert_eq!(a.label, Label::No);
    }

    #[test]
    fn a_quotation_absent_from_the_passage_is_no() {
        let a = check(&case("Jane Okafor said \"the council has failed us all\"."));
        assert_eq!(a.label, Label::No);
    }
}
