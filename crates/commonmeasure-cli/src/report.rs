//! How the commands that report on the edge print: `doctor`, `status`,
//! `credentials` and `console` draw their marks, headings and tables from
//! here, so a finding looks the same wherever it appears.
//!
//! A report is read two ways. A person reads it in a terminal, where a mark
//! before each finding says which lines need them before they read the
//! words, colour separates the marks, and a long sentence wraps to the
//! terminal's width. A script reads it through `--json`, or through a pipe,
//! where every finding is one line with no escape codes, so `grep` and a
//! test see the words alone. The choice between the two is made once, from
//! `--color` and whether stdout is a terminal, and every command draws from
//! the same [`Palette`].
//!
//! The words of a finding never depend on the palette: colour and wrapping
//! are presentation, and a sentence copied out of a terminal into an issue
//! is the sentence the pipe would have printed.

use std::fmt::Write as _;
use std::io::IsTerminal as _;

use anstyle::{AnsiColor, Color, Style};
use commonmeasure_types::{Finding, Standing};

/// The mark before a finding, one per standing. Characters every monospace
/// font carries, and the same in a pipe as in a terminal, so a report
/// pasted from either reads alike.
pub fn mark(standing: Standing) -> &'static str {
    match standing {
        Standing::Ok => "✓",
        Standing::Attention => "!",
        Standing::Unknown => "?",
        Standing::Note => "·",
    }
}

/// The width a report wraps to when the terminal does not say, and the
/// bounds a reported width is kept within so a very wide or a very narrow
/// window still reads as a report.
const DEFAULT_WIDTH: usize = 100;
const MIN_WIDTH: usize = 60;
const MAX_WIDTH: usize = 140;

/// Whether output carries colour, and the width it wraps to. Decided once
/// per command from `--color` and stdout ([`Palette::for_stdout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    colour: bool,
    /// The wrap width; `None` prints each finding on one line, which is
    /// what a pipe gets.
    width: Option<usize>,
}

impl Palette {
    /// No escape codes and one finding per line: what a pipe gets, and
    /// what a test renders with.
    #[cfg(test)]
    pub const PLAIN: Palette = Palette {
        colour: false,
        width: None,
    };

    /// Colour, and findings wrapped to `width`. Tests render with this to
    /// see the terminal form without a terminal.
    #[cfg(test)]
    pub const fn styled(width: usize) -> Palette {
        Palette {
            colour: true,
            width: Some(width),
        }
    }

    /// From `--color` and stdout. `auto` colours a terminal and nothing
    /// else, and honours `NO_COLOR`, `CLICOLOR_FORCE` and `TERM=dumb` as
    /// anstream reads them; `always` and `never` decide outright. Findings
    /// wrap only when stdout is a terminal, whatever the colour choice, so
    /// `--color always | less -R` still gets one finding per line.
    pub fn for_stdout(choice: clap::ColorChoice) -> Palette {
        let choice = match choice {
            clap::ColorChoice::Auto => anstream::ColorChoice::Auto,
            clap::ColorChoice::Always => anstream::ColorChoice::Always,
            clap::ColorChoice::Never => anstream::ColorChoice::Never,
        };
        choice.write_global();
        let stdout = std::io::stdout();
        let colour = anstream::AutoStream::choice(&stdout) != anstream::ColorChoice::Never;
        let width = stdout.is_terminal().then(terminal_width);
        Palette { colour, width }
    }

    fn paint(&self, style: Style, text: &str) -> String {
        if !self.colour || text.is_empty() {
            return text.to_owned();
        }
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    /// A section heading.
    pub fn heading(&self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }

    /// Text that supports a line without being its point: a path, a unit.
    pub fn dim(&self, text: &str) -> String {
        self.paint(Style::new().dimmed(), text)
    }

    /// The coloured mark for a standing.
    pub fn mark(&self, standing: Standing) -> String {
        let colour = match standing {
            Standing::Ok => AnsiColor::Green,
            Standing::Attention => AnsiColor::Yellow,
            Standing::Unknown => AnsiColor::Cyan,
            Standing::Note => AnsiColor::BrightBlack,
        };
        let style = Style::new().fg_color(Some(Color::Ansi(colour)));
        let style = match standing {
            Standing::Attention => style.bold(),
            _ => style,
        };
        self.paint(style, mark(standing))
    }

    /// One finding: `indent` spaces, the mark, a space and the text, ended
    /// by a newline. In a terminal the text wraps to the palette's width
    /// with each continuation line under the first word of the text.
    pub fn finding(&self, finding: &Finding, indent: usize) -> String {
        let lead = " ".repeat(indent);
        let hang = " ".repeat(indent + 2);
        let mut out = String::new();
        let mut first = true;
        for line in self.wrap(&finding.text, indent + 2) {
            if first {
                let _ = writeln!(out, "{lead}{} {line}", self.mark(finding.standing));
                first = false;
            } else {
                let _ = writeln!(out, "{hang}{line}");
            }
        }
        out
    }

    /// Findings one after another, each as [`Palette::finding`].
    pub fn findings(&self, findings: &[Finding], indent: usize) -> String {
        findings
            .iter()
            .map(|finding| self.finding(finding, indent))
            .collect()
    }

    /// A row of a key and value table: the key padded to `key_width`, then
    /// the value, ended by a newline. With a standing, its mark leads the
    /// row; without one, the row is indented by the mark's width so keys
    /// align down the table. The value wraps in a terminal, continuation
    /// lines under the value.
    pub fn row(
        &self,
        standing: Option<Standing>,
        key: &str,
        value: &str,
        key_width: usize,
    ) -> String {
        let lead = match standing {
            Some(standing) => format!("{} ", self.mark(standing)),
            None => "  ".to_owned(),
        };
        let key_column = key_width.max(key.chars().count() + 1);
        let hang = " ".repeat(2 + key_column);
        let mut out = String::new();
        let mut first = true;
        for line in self.wrap(value, 2 + key_column) {
            if first {
                let _ = writeln!(out, "{lead}{key:<key_column$}{line}");
                first = false;
            } else {
                let _ = writeln!(out, "{hang}{line}");
            }
        }
        out
    }

    /// `text` in lines of at most the palette's width less `used` columns,
    /// broken between words and never inside one, so a path or a URL longer
    /// than the width is kept whole and overflows. Lines the text already
    /// holds are kept. Without a width, the text's own lines.
    fn wrap(&self, text: &str, used: usize) -> Vec<String> {
        let Some(width) = self.width else {
            return text.lines().map(str::to_owned).collect();
        };
        let room = width.saturating_sub(used).max(20);
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                if line.is_empty() {
                    line.push_str(word);
                } else if line.chars().count() + 1 + word.chars().count() <= room {
                    line.push(' ');
                    line.push_str(word);
                } else {
                    lines.push(std::mem::take(&mut line));
                    line.push_str(word);
                }
            }
            lines.push(line);
        }
        lines
    }
}

/// How many findings stand each way, for the sentence a report ends with.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub ok: usize,
    pub attention: usize,
    pub unknown: usize,
    pub note: usize,
}

impl Tally {
    pub fn of<'a>(findings: impl IntoIterator<Item = &'a Finding>) -> Tally {
        let mut tally = Tally::default();
        for finding in findings {
            match finding.standing {
                Standing::Ok => tally.ok += 1,
                Standing::Attention => tally.attention += 1,
                Standing::Unknown => tally.unknown += 1,
                Standing::Note => tally.note += 1,
            }
        }
        tally
    }

    /// The findings that need the operator and the ones that could not be
    /// determined, in words: `nothing needs attention`, `2 findings need
    /// attention`, `1 finding needs attention, 1 could not be determined`.
    pub fn sentence(&self) -> String {
        let attention = match self.attention {
            0 => "nothing needs attention".to_owned(),
            1 => "1 finding needs attention".to_owned(),
            n => format!("{n} findings need attention"),
        };
        match self.unknown {
            0 => attention,
            n => format!("{attention}, {n} could not be determined"),
        }
    }

    /// The standing a whole report takes: the worst of its parts, in the
    /// order attention, unknown, ok.
    pub fn standing(&self) -> Standing {
        if self.attention > 0 {
            Standing::Attention
        } else if self.unknown > 0 {
            Standing::Unknown
        } else if self.ok > 0 {
            Standing::Ok
        } else {
            Standing::Note
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": self.ok,
            "attention": self.attention,
            "unknown": self.unknown,
            "note": self.note,
        })
    }
}

/// The terminal's width: `COLUMNS` where the shell exports it, else what
/// the terminal reports, else [`DEFAULT_WIDTH`]; kept within the bounds
/// above.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|columns| columns.trim().parse::<usize>().ok())
        .filter(|columns| *columns > 0)
        .or_else(reported_width)
        .unwrap_or(DEFAULT_WIDTH)
        .clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The column count the terminal on stdout reports, or `None` where it
/// reports nothing.
#[cfg(unix)]
fn reported_width() -> Option<usize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ fills the `winsize` the pointer names and touches
    // nothing else; `size` lives for the whole call.
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) };
    (result == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

#[cfg(not(unix))]
fn reported_width() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_finding_is_one_line_with_its_mark() {
        let text = Palette::PLAIN.finding(&Finding::attention("binary /x: not found"), 2);
        assert_eq!(text, "  ! binary /x: not found\n");
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn a_styled_finding_wraps_between_words_and_hangs_under_the_text() {
        let finding = Finding::ok(
            "policy: /home/op/.commonmeasure/policy.json loads; top-level mode Prefer, 2 \
             scope(s), 1 principal(s)",
        );
        let text = Palette::styled(60).finding(&finding, 2);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() > 1, "{text}");
        assert!(lines[0].contains("policy:"), "{text}");
        assert!(
            lines[1..].iter().all(|line| line.starts_with("    ")),
            "{text}"
        );
        // The words survive wrapping and colour.
        let plain: String = lines
            .iter()
            .map(|line| line.trim_start())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(plain.contains("loads; top-level mode Prefer, 2 scope(s), 1 principal(s)"));
        assert!(text.contains("\u{1b}[32m"), "the mark is green: {text:?}");
    }

    #[test]
    fn a_word_longer_than_the_width_is_kept_whole() {
        let path = "/".to_owned() + &"segment/".repeat(20);
        let finding = Finding::note(format!("recording: {path} is writable"));
        let text = Palette::styled(60).finding(&finding, 0);
        assert!(text.contains(&path), "{text}");
    }

    #[test]
    fn a_row_pads_its_key_and_leads_with_the_mark_or_its_width() {
        let marked = Palette::PLAIN.row(Some(Standing::Ok), "edge", "key-1", 18);
        assert_eq!(marked, "✓ edge              key-1\n");
        let unmarked = Palette::PLAIN.row(None, "deployment mode", "local", 18);
        assert_eq!(unmarked, "  deployment mode   local\n");
        let column =
            |line: &str, value: &str| line.find(value).map(|at| line[..at].chars().count());
        assert_eq!(
            column(&marked, "key-1"),
            column(&unmarked, "local"),
            "values align whether or not a row carries a mark"
        );
    }

    #[test]
    fn a_tally_reads_the_standings_and_says_what_needs_attention() {
        let findings = [
            Finding::ok("a"),
            Finding::attention("b"),
            Finding::unknown("c"),
            Finding::note("d"),
            Finding::attention("e"),
        ];
        let tally = Tally::of(&findings);
        assert_eq!(
            tally,
            Tally {
                ok: 1,
                attention: 2,
                unknown: 1,
                note: 1
            }
        );
        assert_eq!(
            tally.sentence(),
            "2 findings need attention, 1 could not be determined"
        );
        assert_eq!(tally.standing(), Standing::Attention);
        assert_eq!(
            Tally::of(&[Finding::ok("a")]).sentence(),
            "nothing needs attention"
        );
        assert_eq!(
            Tally::of(&[Finding::attention("a")]).sentence(),
            "1 finding needs attention"
        );
        assert_eq!(Tally::of(&[]).standing(), Standing::Note);
    }
}
