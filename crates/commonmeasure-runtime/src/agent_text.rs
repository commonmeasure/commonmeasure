//! Text the agent reads about a crossing that was refused or failed.
//!
//! A refused crossing's source record says the source's text never entered
//! context (`grounded` is false). That claim holds only if the refusal the
//! agent reads carries nothing the source chose: not a redirect's host or
//! path, a header value, a licence URL, a certificate name, a search
//! result's title, snippet or URL, nor an error that quotes any of these.
//! [`AgentText`] is the one type such a refusal is built from, and it can
//! hold only:
//!
//! - text written in this repository (`&'static str`);
//! - another [`AgentText`];
//! - a value of fixed form, which spells nothing: a count, a position, a
//!   status code, a number of seconds, an instant, an amount of money in a
//!   three-letter currency;
//! - a value the operator's policy or the agent's own call gave, through
//!   [`Given`]: the URL the agent asked for, a rule, a licence identifier
//!   the operator wrote, a provider name, a principal, a file under the
//!   operator home.
//!
//! There is no way in for a `String` or a `&str` of any lifetime but
//! `'static`, so a value read from a source cannot be formatted into the
//! text: the compiler refuses it. The three doors that take a runtime
//! value ([`Given::text`], [`Given::path`] and the `Part` impls for foreign
//! types) are named for the provenance they admit, and a test in this
//! module pins where each is called, so opening a new one is a change a
//! reviewer sees. It reads production code by its brackets, not by a text
//! split, counts a door named as a function path as a call, and refuses
//! renaming, re-exporting or aliasing [`Given`]. What the type cannot stop
//! is a deliberate lie: a source value passed to [`Given::text`], or a
//! `String` leaked to `&'static str`. The same test refuses `leak`,
//! `transmute`, a static `String` and an accessor that hands out the
//! text's buffer in the product crates.
//!
//! The source record keeps every value whole in its own sentences. Where a
//! refusal has a record sentence and an agent sentence, they are two
//! strings: [`crate::policy::Ruling`] carries both.
//!
//! A `String` does not compile into the text:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text;
//! let host = String::from("a host the source chose");
//! let _ = agent_text!["The job denies host ", host, "."];
//! ```
//!
//! nor does a `&str` borrowed from one:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text;
//! let host = String::from("a host the source chose");
//! let _ = agent_text!["The job denies host ", host.as_str(), "."];
//! ```
//!
//! nor a `format!`:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text;
//! let path = "/a/path/the/source/chose";
//! let _ = agent_text!["refused: ", format!("the path {path}")];
//! ```
//!
//! nor a `&String`, a `Cow` or a boxed string:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text;
//! let host = String::from("x");
//! let _ = agent_text!["refused: ", &host];
//! ```
//!
//! and there is no `From<String>`:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text::AgentText;
//! let text: AgentText = String::from("x").into();
//! ```
//!
//! Nothing hands out the buffer, so a `String` cannot be written into the
//! text through a `&mut String` either; [`AgentText::push`] takes a [`Part`]
//! and nothing else:
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text::AgentText;
//! let url = String::from("https://a-redirect.the-source.chose/path");
//! let mut text = AgentText::fixed("the target of redirect 1 answered 404");
//! text.as_string_mut().push_str(&url);
//! ```
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text::AgentText;
//! let url = String::from("https://a-redirect.the-source.chose/path");
//! let mut text = AgentText::empty();
//! text.push(url);
//! ```
//!
//! ```compile_fail
//! use commonmeasure_runtime::agent_text::AgentText;
//! let mut url = String::from("https://a-redirect.the-source.chose/path");
//! let mut text = AgentText::empty();
//! text.push(&mut url);
//! ```

use std::fmt;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use commonmeasure_types::Money;

/// Text the agent reads about a refused or failed crossing. See the module
/// documentation for what it may hold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentText(String);

impl AgentText {
    /// Text with nothing in it yet.
    pub const fn empty() -> Self {
        Self(String::new())
    }

    /// Text written in this repository.
    pub fn fixed(text: &'static str) -> Self {
        Self(text.to_owned())
    }

    /// Append one part.
    pub fn push<P: Part>(&mut self, part: P) -> &mut Self {
        part.append(&mut self.0);
        self
    }

    /// `items` written one after another with `separator` between them.
    pub fn list<I, P>(items: I, separator: &'static str) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Part,
    {
        let mut text = Self::empty();
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                text.push(separator);
            }
            text.push(item);
        }
        text
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text as a `String`, for the boundary that returns it to the host.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for AgentText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<AgentText> for String {
    fn from(text: AgentText) -> Self {
        text.0
    }
}

// Serialised as the string it is, so a result field may carry it. There is
// no `Deserialize`: reading one back from bytes would be a way in.
impl serde::Serialize for AgentText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Text built from parts, each of a kind [`AgentText`] may hold:
///
/// ```
/// use commonmeasure_runtime::agent_text;
/// let hops: usize = 2;
/// let told = agent_text!["The target of redirect ", hops, " was refused."];
/// assert_eq!(told.as_str(), "The target of redirect 2 was refused.");
/// ```
///
/// Each part goes through [`Part`]; a `String` or a borrowed `&str` is not
/// one, so the call does not compile.
#[macro_export]
macro_rules! agent_text {
    ($($part:expr),* $(,)?) => {{
        let mut text = $crate::agent_text::AgentText::empty();
        $( $crate::agent_text::AgentText::push(&mut text, $part); )*
        text
    }};
}

mod sealed {
    pub trait Sealed {}
}

/// A value [`AgentText`] may hold. Sealed: the implementations are the ones
/// in this module, and nothing outside it can add one.
pub trait Part: sealed::Sealed {
    /// Write the part.
    fn append(self, into: &mut String);
}

macro_rules! part {
    ($type:ty, |$value:ident, $into:ident| $write:expr) => {
        impl sealed::Sealed for $type {}
        impl Part for $type {
            fn append(self, $into: &mut String) {
                let $value = self;
                $write
            }
        }
    };
}

part!(&'static str, |text, into| into.push_str(text));
part!(AgentText, |text, into| into.push_str(&text.0));
part!(&AgentText, |text, into| into.push_str(&text.0));
part!(u8, |n, into| into.push_str(&n.to_string()));
part!(u16, |n, into| into.push_str(&n.to_string()));
part!(u32, |n, into| into.push_str(&n.to_string()));
part!(u64, |n, into| into.push_str(&n.to_string()));
part!(usize, |n, into| into.push_str(&n.to_string()));
part!(i64, |n, into| into.push_str(&n.to_string()));
// An instant is written as the record writes one.
part!(DateTime<Utc>, |at, into| into.push_str(&at.to_rfc3339()));
// A duration is written as seconds with up to three decimals, as the
// harness's pacing text writes one.
part!(Duration, |duration, into| {
    let ms = duration.as_millis();
    let (whole, fraction) = (ms / 1000, ms % 1000);
    if fraction == 0 {
        into.push_str(&whole.to_string());
    } else {
        let written = format!("{whole}.{fraction:03}");
        into.push_str(written.trim_end_matches('0'));
    }
});
// An amount is of fixed form where its currency is a three-letter code; a
// licence's own text for a currency is not, and is named by position.
part!(&Money, |money, into| {
    let currency = money.currency();
    if currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase()) {
        into.push_str(&money.as_decimal_string());
        into.push(' ');
        into.push_str(currency);
    } else {
        into.push_str("an amount in a currency that is not a three-letter code");
    }
});
part!(Given, |given, into| into.push_str(&given.0));
part!(&Given, |given, into| into.push_str(&given.0));

/// A value the operator's policy or the agent's own call gave, which agent
/// text may repeat: the URL the agent asked for, an access rule, a licence
/// identifier the operator wrote, a provider name, a principal, a file
/// under the operator home. This is the one door by which a runtime string
/// enters [`AgentText`]; a value a source chose is never passed through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Given(String);

impl Given {
    /// A value the operator's policy or the agent's own call gave.
    pub fn text(given: &str) -> Self {
        Self(given.to_owned())
    }

    /// A file under the operator home, as the caller may be told it.
    pub fn path(path: &Path) -> Self {
        Self(path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn parts_are_written_in_their_fixed_forms() {
        let at = DateTime::parse_from_rfc3339("2026-09-30T10:00:00Z")
            .expect("an instant")
            .with_timezone(&Utc);
        let told = agent_text![
            "hop ",
            3usize,
            " answered ",
            503u16,
            " at ",
            at,
            "; wait ",
            Duration::from_millis(1500),
            "s, budget ",
            Duration::from_secs(20),
            "s; price ",
            &Money::new("EUR", 1_250_000),
            "; ",
            Given::text("access rule 1 (*.example)"),
            "; ",
            AgentText::fixed("fixed"),
        ];
        assert_eq!(
            told.as_str(),
            "hop 3 answered 503 at 2026-09-30T10:00:00+00:00; wait 1.5s, budget 20s; price \
             1.250000 EUR; access rule 1 (*.example); fixed"
        );
        assert_eq!(
            agent_text![&Money::new("zqxv", 1)].as_str(),
            "an amount in a currency that is not a three-letter code"
        );
        assert_eq!(AgentText::list(["a", "b", "c"], ", ").as_str(), "a, b, c");
        assert!(AgentText::empty().is_empty());
    }

    /// Every file under the product crates' `src`, with its text.
    fn product_sources() -> BTreeMap<String, String> {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crates directory");
        let mut sources = BTreeMap::new();
        fn walk(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("a readable directory") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(&path, into);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    into.push(path);
                }
            }
        }
        let mut files = Vec::new();
        for entry in std::fs::read_dir(crates).expect("the crates directory") {
            let src = entry.expect("a crate").path().join("src");
            if src.is_dir() {
                walk(&src, &mut files);
            }
        }
        for file in files {
            let relative = file
                .strip_prefix(crates)
                .expect("under crates")
                .to_string_lossy()
                .replace('\\', "/");
            sources.insert(relative, std::fs::read_to_string(&file).expect("readable"));
        }
        assert!(sources.contains_key("commonmeasure-runtime/src/agent_text.rs"));
        sources
    }

    /// A source file as the lint reads it.
    struct Read {
        /// The file with its comments, its literals' contents, every
        /// `#[cfg(test)]` item and every `mod tests` block blanked, the
        /// last two found by matching brackets, so production code after a
        /// test module is still read and a doc comment, a doctest or a
        /// string naming a door is not a call.
        production: String,
        /// The modules those test items declare (`mod NAME;` or `mod NAME
        /// { … }`), whose files are test code too.
        test_modules: Vec<String>,
    }

    /// Whether `byte` may continue an identifier.
    fn ident(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_'
    }

    /// The length of the string or character literal at `at`, or `None`
    /// where none starts there. Raw strings close on their own count of
    /// `#`; a quote that opens no character literal is a lifetime's.
    fn literal_at(bytes: &[u8], at: usize) -> Option<usize> {
        let after_ident = at > 0 && ident(bytes[at - 1]);
        match bytes[at] {
            b'"' => {
                let mut end = at + 1;
                while end < bytes.len() && bytes[end] != b'"' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                Some(end + 1 - at)
            }
            b'\'' => {
                let next = *bytes.get(at + 1)?;
                if next == b'\\' {
                    let mut end = at + 3;
                    while end < bytes.len() && bytes[end] != b'\'' {
                        end += 1;
                    }
                    return Some(end + 1 - at);
                }
                let width = match next {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(width + 2)
            }
            b'r' if !after_ident || (at == 1 || !ident(bytes[at - 2])) && bytes[at - 1] == b'b' => {
                let hashes = bytes[at + 1..].iter().take_while(|&&b| b == b'#').count();
                if bytes.get(at + 1 + hashes) != Some(&b'"') {
                    return None;
                }
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let body = at + 2 + hashes;
                let end = bytes[body..]
                    .windows(close.len())
                    .position(|window| window == close.as_slice())
                    .map_or(bytes.len(), |found| body + found + close.len());
                Some(end - at)
            }
            _ => None,
        }
    }

    /// `source` with its comments and its literals' contents blanked
    /// (newlines kept), and a mask of the bytes inside a literal. Block
    /// comments nest.
    fn blanked(source: &str) -> (Vec<u8>, Vec<bool>) {
        let bytes = source.as_bytes();
        let mut code = bytes.to_vec();
        let mut literal = vec![false; bytes.len()];
        let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
            for byte in &mut code[from..to] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
        };
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at..].starts_with(b"//") {
                let end = bytes[at..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |found| at + found);
                blank(&mut code, at, end);
                at = end;
            } else if bytes[at..].starts_with(b"/*") {
                let (mut depth, mut end) = (0usize, at);
                while end < bytes.len() {
                    if bytes[end..].starts_with(b"/*") {
                        depth += 1;
                        end += 2;
                    } else if bytes[end..].starts_with(b"*/") {
                        depth -= 1;
                        end += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        end += 1;
                    }
                }
                blank(&mut code, at, end.min(bytes.len()));
                at = end;
            } else if let Some(length) = literal_at(bytes, at) {
                let end = (at + length).min(bytes.len());
                literal[at..end].fill(true);
                blank(&mut code, at, end);
                at = end;
            } else {
                at += 1;
            }
        }
        (code, literal)
    }

    /// The index after the bracket that closes the one at `at`.
    fn closing(code: &[u8], literal: &[bool], mut at: usize) -> usize {
        let mut depth = 0usize;
        while at < code.len() {
            if !literal[at] {
                match code[at] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return at + 1;
                        }
                    }
                    _ => {}
                }
            }
            at += 1;
        }
        at
    }

    /// The end of the item that starts at `at`: after its block where a
    /// `{` opens one outside any bracket, else at the `;` or `,` that ends
    /// it, or before the bracket that closes what holds it.
    fn item_end(code: &[u8], literal: &[bool], mut at: usize) -> usize {
        let mut depth = 0usize;
        while at < code.len() {
            if !literal[at] {
                match code[at] {
                    b'{' if depth == 0 => return closing(code, literal, at),
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' if depth == 0 => return at,
                    b')' | b']' | b'}' => depth -= 1,
                    b';' | b',' if depth == 0 => return at + 1,
                    _ => {}
                }
            }
            at += 1;
        }
        at
    }

    /// The name of the module an item declares, where it declares one.
    fn module_named(item: &str) -> Option<String> {
        let mut words = item
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|word| !word.is_empty())
            .skip_while(|word| *word == "pub" || *word == "crate" || *word == "super");
        (words.next() == Some("mod")).then(|| words.next().unwrap_or_default().to_owned())
    }

    fn read_source(source: &str) -> Read {
        let (code, literal) = blanked(source);
        let mut production = code.clone();
        let mut test_modules = Vec::new();
        let mut at = 0;
        while at < code.len() {
            let attribute = code[at..].starts_with(b"#[cfg(test)]");
            let tests_block = code[at..].starts_with(b"mod tests")
                && (at == 0 || !ident(code[at - 1]))
                && code.get(at + 9).is_none_or(|&b| !ident(b));
            if literal[at] || !(attribute || tests_block) {
                at += 1;
                continue;
            }
            // The item under the attribute starts after any further
            // attributes on it.
            let mut start = if attribute { at + 12 } else { at };
            loop {
                while start < code.len() && code[start].is_ascii_whitespace() {
                    start += 1;
                }
                if code[start..].starts_with(b"#[") {
                    start = closing(&code, &literal, start + 1);
                } else {
                    break;
                }
            }
            let end = item_end(&code, &literal, start);
            let item = String::from_utf8_lossy(&code[start..end]);
            if let Some(name) = module_named(&item) {
                test_modules.push(name);
            }
            for byte in &mut production[at..end] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
            at = end;
        }
        if code.windows(13).any(|window| window == b"#![cfg(test)]") {
            production.fill(b' ');
        }
        Read {
            production: String::from_utf8(production).expect("blanking keeps UTF-8"),
            test_modules,
        }
    }

    /// The directory a file's `mod NAME;` declarations resolve under.
    fn module_directory(file: &str) -> String {
        let (parent, name) = file.rsplit_once('/').unwrap_or(("", file));
        match name {
            "lib.rs" | "main.rs" | "mod.rs" => parent.to_owned(),
            _ => format!("{parent}/{}", name.trim_end_matches(".rs")),
        }
    }

    /// Calls of [`Given::text`] and [`Given::path`] in `production`, with or
    /// without a parenthesis after them, so a function path
    /// (`.map(Given::text)`) counts as a call. Read with whitespace removed,
    /// so a path split by spaces or lines (as a macro body may keep it)
    /// counts too.
    fn doors_in(production: &str) -> usize {
        let unspaced: String = production.chars().filter(|c| !c.is_whitespace()).collect();
        ["Given::text", "Given::path", "Given>::text", "Given>::path"]
            .iter()
            .map(|door| unspaced.matches(door).count())
            .sum()
    }

    /// Each statement in `code` that starts with the keyword `word` (`use`,
    /// `type`), up to its `;`, with whatever visibility precedes it.
    fn statements<'a>(code: &'a str, word: &str) -> Vec<&'a str> {
        let bytes = code.as_bytes();
        let mut found = Vec::new();
        for (at, _) in code.match_indices(word) {
            let after = at + word.len();
            if (at > 0 && ident(bytes[at - 1])) || bytes.get(after).is_none_or(|&b| b != b' ') {
                continue;
            }
            let line = code[..at].rfind('\n').map_or(0, |newline| newline + 1);
            let end = code[at..]
                .find(';')
                .map_or(code.len(), |semi| at + semi + 1);
            found.push(&code[line..end]);
        }
        found
    }

    /// Whether `text` names `Given` as a word.
    fn names_given(text: &str) -> bool {
        text.match_indices("Given").any(|(at, _)| {
            let bytes = text.as_bytes();
            (at == 0 || !ident(bytes[at - 1])) && bytes.get(at + 5).is_none_or(|&b| !ident(b))
        })
    }

    /// What production code outside this module may not do with the door:
    /// rename it, re-export it or alias its type, any of which would let a
    /// call escape [`doors_in`].
    fn renamed_doors(production: &str) -> Vec<String> {
        let mut found = Vec::new();
        if production.contains("Given as ") {
            found.push("`Given as`".to_owned());
        }
        for statement in statements(production, "use") {
            if names_given(statement) && statement.trim_start().starts_with("pub") {
                found.push(format!("a re-export: `{}`", statement.trim()));
            }
        }
        for statement in statements(production, "type") {
            if names_given(statement) {
                found.push(format!("a type alias: `{}`", statement.trim()));
            }
        }
        found
    }

    /// The lint reads past a test module, counts a door named as a path,
    /// and refuses a renamed door; a door in a comment or in test code is
    /// not counted.
    #[test]
    fn the_lint_reads_what_a_text_match_missed() {
        let after_tests = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { \
                           Given::text(\"test\"); }\n}\nfn c(u: &str) { Given::text(u); }\n";
        assert_eq!(doors_in(&read_source(after_tests).production), 1);
        let path = "fn a(v: Vec<&str>) { v.into_iter().map(Given::text); }";
        assert_eq!(doors_in(&read_source(path).production), 1);
        let qualified = "fn a(u: &str) { <Given>::text(u); }";
        assert_eq!(doors_in(&read_source(qualified).production), 1);
        let spaced = "macro_rules! door { ($u:expr) => { Given\n    :: text($u) }; }";
        assert_eq!(doors_in(&read_source(spaced).production), 1);
        let commented = "/// [`Given::text`]\n// Given::path(\nfn a() { let _ = \"}\"; }\n\
                         #[cfg(test)]\nfn b() { Given::text(\"{\"); }\n";
        assert_eq!(doors_in(&read_source(commented).production), 0);
        let nested = "#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    mod inner { \
                      /* { */ fn b() { let _ = r#\"}\"#; let _ = '}'; } }\n}\n\
                      fn c<'a>(u: &'a str) { Given::path(u); }\n";
        let nested = read_source(nested);
        assert_eq!(doors_in(&nested.production), 1);
        assert_eq!(nested.test_modules, ["tests"]);
        assert_eq!(
            read_source("#[cfg(test)]\nmod corpus;\n").test_modules,
            ["corpus"]
        );
        for renamed in [
            "use commonmeasure_runtime::agent_text::Given as Door;",
            "use commonmeasure_runtime::agent_text::{AgentText, Given as Door};",
            "pub use commonmeasure_runtime::agent_text::Given;",
            "pub(crate) use crate::agent_text::{AgentText, Given};",
            "type Door = commonmeasure_runtime::agent_text::Given;",
        ] {
            assert_eq!(
                renamed_doors(&read_source(renamed).production).len(),
                1,
                "{renamed}"
            );
        }
        let imported = "use commonmeasure_runtime::agent_text::{AgentText, Given};";
        assert!(renamed_doors(&read_source(imported).production).is_empty());
        assert_eq!(
            module_directory("commonmeasure-harness/src/mcp.rs"),
            "commonmeasure-harness/src/mcp"
        );
        assert_eq!(
            module_directory("commonmeasure-harness/src/lib.rs"),
            "commonmeasure-harness/src"
        );
    }

    /// The lint: no way into [`AgentText`] exists outside this module, and
    /// the doors that take a runtime value are called only where this test
    /// says. A new call site is a change to this list, which a reviewer
    /// then reads.
    #[test]
    fn agent_text_has_no_way_in_but_the_doors_this_test_names() {
        let sources = product_sources();
        let this_module = "commonmeasure-runtime/src/agent_text.rs";
        let read: BTreeMap<&String, Read> = sources
            .iter()
            .map(|(file, text)| (file, read_source(text)))
            .collect();
        // Files declared as test modules are test code throughout.
        let test_directories: Vec<String> = read
            .iter()
            .flat_map(|(file, read)| {
                let directory = module_directory(file);
                read.test_modules
                    .iter()
                    .map(move |name| format!("{directory}/{name}"))
            })
            .collect();
        let is_test_file = |file: &str| {
            test_directories.iter().any(|module| {
                file == format!("{module}.rs") || file.starts_with(&format!("{module}/"))
            })
        };
        assert!(
            is_test_file("commonmeasure-harness/src/mcp/tests/refusal_corpus.rs"),
            "the corpus is test code: {test_directories:?}"
        );

        // `Part` is sealed, so an impl outside this module is refused by the
        // compiler; the check is on the seal itself.
        for (file, text) in &sources {
            if file == this_module {
                continue;
            }
            for pattern in ["impl Part for", "impl Sealed for", "agent_text::sealed"] {
                assert!(!text.contains(pattern), "{file} contains `{pattern}`");
            }
            // A `String` made `'static` would pass the seal, and the buffer
            // handed out would bypass it.
            for pattern in [
                ".leak()",
                "::leak(",
                "transmute",
                "OnceLock<String>",
                "LazyLock<String>",
                "OnceCell<String>",
                "as_string_mut",
            ] {
                assert!(!text.contains(pattern), "{file} contains `{pattern}`");
            }
            if !is_test_file(file) {
                let renamed = renamed_doors(&read[file].production);
                assert!(renamed.is_empty(), "{file} renames the door: {renamed:?}");
            }
        }

        // This module hands out no `&mut String` into the text: the buffer
        // goes only to a `Part`'s `append` (the trait's signature and the
        // one `part!` writes), from `push`.
        let own = &read[&this_module.to_owned()].production;
        for pattern in [
            "as_string_mut",
            "AsMut<",
            "BorrowMut<",
            "DerefMut",
            "-> &mut String",
            "as_mut_str",
            "as_mut_vec",
        ] {
            assert!(!own.contains(pattern), "{this_module} contains `{pattern}`");
        }
        assert_eq!(
            own.matches("&mut String").count(),
            2,
            "{this_module}: `&mut String`"
        );
        assert_eq!(
            own.matches("&mut self.0").count(),
            1,
            "{this_module}: `&mut self.0`"
        );

        // Where a runtime value is passed to the door. Test code is
        // excluded: a test may lie to show that the boundary holds.
        let mut doors: BTreeMap<String, usize> = BTreeMap::new();
        for (file, read) in &read {
            if is_test_file(file) {
                continue;
            }
            let count = doors_in(&read.production);
            if count > 0 {
                doors.insert((*file).clone(), count);
            }
        }
        let expected: BTreeMap<String, usize> = [
            // The back-off store's directory, in a remedy.
            ("commonmeasure-harness/src/crawl_delay/backoff.rs", 2),
            // The URL the agent asked for (`hop_named`); a file under the
            // home; the delivery marker, consent, policy-load and
            // relay-load sentences; the instance registration's reason;
            // the enrolment's signing fault; the receiver `relay.json`
            // names, by origin and digest, and the enrolment-load fault
            // (`reporting_route`); the operator's own reference for an
            // agreement whose reporting duty is unmet (`terms[].reference`
            // in the source policy).
            ("commonmeasure-harness/src/mcp.rs", 11),
            // The principal-authority reason, this edge's own.
            ("commonmeasure-harness/src/policy.rs", 2),
            // The principal.
            ("commonmeasure-runtime/src/allowance.rs", 3),
            // A provider name, an access rule and a licence identifier
            // the operator wrote.
            ("commonmeasure-runtime/src/policy.rs", 5),
            // The principal.
            ("commonmeasure-runtime/src/run.rs", 1),
        ]
        .into_iter()
        .map(|(file, count)| (file.to_owned(), count))
        .collect();
        assert_eq!(doors, expected, "the door sites changed; read each one");
    }
}
