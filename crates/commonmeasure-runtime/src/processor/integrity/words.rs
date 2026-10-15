//! Word matching shared by the integrity checker's stages: case-insensitive,
//! leading and trailing punctuation ignored, typographic quotes and dashes
//! folded, whitespace flexible. Looser at a word's edges than the grounding
//! evaluator's comparison (`crate::evaluate`), because a quotation's opening
//! mark sits on its first word.

/// A word with its byte span in the text it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
}

/// The whitespace-delimited words of `text` with their byte offsets.
pub fn words(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (false, None) => start = Some(index),
            (true, Some(begin)) => {
                out.push(Word {
                    text: &text[begin..index],
                    start: begin,
                    end: index,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        out.push(Word {
            text: &text[begin..],
            start: begin,
            end: text.len(),
        });
    }
    out
}

/// A word reduced to what is compared: lower case, with leading and trailing
/// punctuation removed, and typographic quotes and dashes folded to their
/// plain forms.
pub fn key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The longest run of consecutive words of `needle` that occurs, in order,
/// in `haystack`: `(length, needle word index, haystack word index)`.
pub fn longest_common_run(needle: &[Word<'_>], haystack: &[Word<'_>]) -> (usize, usize, usize) {
    let n: Vec<String> = needle.iter().map(|w| key(w.text)).collect();
    let h: Vec<String> = haystack.iter().map(|w| key(w.text)).collect();
    let mut best = (0, 0, 0);
    // Dynamic programme over word pairs; the texts here are a passage and one
    // sentence, so the quadratic cost is small.
    let mut previous = vec![0usize; h.len() + 1];
    for (i, nw) in n.iter().enumerate() {
        let mut current = vec![0usize; h.len() + 1];
        for (j, hw) in h.iter().enumerate() {
            if !nw.is_empty() && nw == hw {
                current[j + 1] = previous[j] + 1;
                if current[j + 1] > best.0 {
                    best = (
                        current[j + 1],
                        i + 1 - current[j + 1],
                        j + 1 - current[j + 1],
                    );
                }
            }
        }
        previous = current;
    }
    best
}

/// Whether every word of `needle` occurs in `haystack` consecutively and in
/// order; the haystack word index where it starts.
pub fn find_run(needle: &[Word<'_>], haystack: &[Word<'_>]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let n: Vec<String> = needle.iter().map(|w| key(w.text)).collect();
    let h: Vec<String> = haystack.iter().map(|w| key(w.text)).collect();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&start| h[start..start + n.len()] == n[..])
}

/// Byte span in the original text covering haystack words `[from, to)`.
pub fn span(words: &[Word<'_>], from: usize, to: usize) -> (usize, usize) {
    (words[from].start, words[to - 1].end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_ignore_case_and_punctuation() {
        let passage = words("The minister said: “We will not raise taxes,” and left.");
        let sentence = words("we will not raise taxes");
        assert_eq!(longest_common_run(&sentence, &passage).0, 5);
        assert_eq!(find_run(&sentence, &passage), Some(3));
    }

    #[test]
    fn a_missing_word_breaks_the_run() {
        let passage = words("one two three four five six");
        let sentence = words("one two four five six");
        assert_eq!(longest_common_run(&sentence, &passage).0, 3);
        assert_eq!(find_run(&sentence, &passage), None);
    }
}
