//! The working guide and its evidence companion, rendered from their Markdown
//! sources (`docs/guide/*.md`) with the guide's stylesheet inlined.
//!
//! The Markdown is the one home of each document. The console renders the
//! product variant once, at first request; `public_pages` renders the
//! education variant, which strips product panels and marked sections and refuses to emit a
//! page in which the product name survives, for publication where the
//! product is not announced. Each page is self-contained: inline stylesheet,
//! with local appearance controls in the console and no remote assets.

use std::fmt::Write as _;
use std::sync::OnceLock;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

const STYLES: &str = include_str!("../../../docs/guide/guide.css");

/// A source document: the file stem the console and the public build name it
/// by, its Markdown, the date its research was last checked, and whether every
/// cited URL was link-checked. The last two are stated on the masthead, so a
/// guide claims a link check only when one was run.
pub struct Source {
    pub name: &'static str,
    pub markdown: &'static str,
    pub current_to: &'static str,
    pub links_checked: bool,
}

/// Every document rendered, in publication order.
pub const SOURCES: [Source; 3] = [
    Source {
        name: "context-window-optimisation",
        markdown: include_str!("../../../docs/guide/context-window-optimisation.md"),
        current_to: "4 August 2026",
        links_checked: true,
    },
    Source {
        name: "state-of-the-evidence",
        markdown: include_str!("../../../docs/guide/state-of-the-evidence.md"),
        current_to: "4 August 2026",
        links_checked: true,
    },
    Source {
        name: "untrusted-context",
        markdown: include_str!("../../../docs/guide/untrusted-context.md"),
        current_to: "15 September 2026",
        links_checked: false,
    },
];

/// The opening line of a product panel in the source: a blockquote whose
/// first paragraph is exactly this, in bold.
const PANEL_HEADING: &str = "What this means for Common Measure";

/// Reading speed the masthead's estimate assumes, in words per minute.
const WORDS_PER_MINUTE: f64 = 260.0;

/// Which audience a page is rendered for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Served by the console: product panels kept, companion links to
    /// console routes.
    Console,
    /// For publication: product panels stripped, companion links to the
    /// sibling HTML file, and no product name anywhere in the output.
    Public,
}

/// One rendered page.
pub struct Page {
    pub name: &'static str,
    pub html: String,
}

/// The console's rendering of the named source, rendered once per process.
pub fn console_page(name: &str) -> Option<&'static str> {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES
        .get_or_init(|| {
            SOURCES
                .iter()
                .map(|source| Page {
                    name: source.name,
                    html: render(source, Variant::Console),
                })
                .collect()
        })
        .iter()
        .find(|page| page.name == name)
        .map(|page| page.html.as_str())
}

/// The education pages for publication. Fails, naming the page, if the
/// product name survives in any output: a mention outside a panel is an error,
/// never a quiet leak.
pub fn public_pages() -> Result<Vec<Page>, String> {
    SOURCES
        .iter()
        .map(|source| {
            let html = render(source, Variant::Public);
            let lower = html.to_lowercase();
            if lower.contains("common measure") || lower.contains("commonmeasure") {
                return Err(format!(
                    "public build of {}.md: the product name survives in the output; \
                     move the mention into a panel",
                    source.name
                ));
            }
            Ok(Page {
                name: source.name,
                html,
            })
        })
        .collect()
}

/// Render one source for one audience.
pub fn render(source: &Source, variant: Variant) -> String {
    let markdown = without_yaml_front_matter(source.markdown);
    let (title, standfirst, body) = split_front_matter(markdown);
    let body = drop_contents_section(&body);
    let body = match variant {
        Variant::Console => body,
        Variant::Public => strip_panels(&body),
    };
    let body = link_companions(&body, variant);
    let chapters = chapters(&body);
    let body_html = promote_panels(&body_to_html(&body));

    let eyebrow = match variant {
        Variant::Console => "Common Measure &middot; working guide",
        Variant::Public => "Working guide",
    };
    let mut toc = String::new();
    for (anchor, label) in &chapters {
        let _ = writeln!(
            toc,
            "      <li><a href=\"#{}\">{}</a></li>",
            esc(anchor),
            esc(label)
        );
    }
    let description: String = standfirst.chars().take(180).collect();

    let styles = without_css_comments(STYLES);
    let styles = match variant {
        Variant::Console => format!(
            "@font-face {{ font-family: 'Geist Variable'; src: url('/fonts/Geist-Variable.woff2') format('woff2'); font-weight: 100 900; font-display: swap; }}\n@font-face {{ font-family: 'Geist Mono Variable'; src: url('/fonts/GeistMono-Variable.woff2') format('woff2'); font-weight: 100 900; font-display: swap; }}\n{styles}"
        ),
        Variant::Public => styles,
    };
    let (theme_script, theme_control) = match variant {
        Variant::Console => (
            "<script src=\"/theme.js\"></script>\n",
            "<button class=\"theme-toggle\" type=\"button\" data-theme-toggle hidden>Dark mode</button>\n",
        ),
        Variant::Public => ("", ""),
    };
    format!(
        "<!doctype html>\n<html lang=\"en-GB\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<meta name=\"description\" content=\"{description}\">\n\
         {theme_script}<style>\n{styles}\n</style>\n</head>\n<body>\n<div class=\"shell\">\n\n\
         <header class=\"masthead\">\n  {theme_control}<span class=\"eyebrow\">{eyebrow}</span>\n  <h1>{title}</h1>\n  \
         <p class=\"standfirst\">{standfirst}</p>\n  <div class=\"facts\">\n{facts}\n  </div>\n</header>\n\n\
         <nav class=\"toc\" aria-label=\"Contents\">\n  <details open>\n    <summary>Contents</summary>\n    \
         <ol>\n{toc}    </ol>\n  </details>\n</nav>\n\n<main>\n{body_html}\n</main>\n\n</div>\n</body>\n</html>\n",
        title = esc(&title),
        description = esc(&description),
        standfirst = esc(&standfirst),
        facts = masthead_facts(markdown, source),
    )
}

/// The document without the YAML block a static-site generator reads at its
/// head (`---` lines around `key: value` pairs). The page's own title and
/// standfirst come from the Markdown that follows, so the block is metadata
/// for the site and must not render as text here.
fn without_yaml_front_matter(markdown: &str) -> &str {
    let Some(rest) = markdown.strip_prefix("---\n") else {
        return markdown;
    };
    match rest.find("\n---\n") {
        Some(end) => rest[end + "\n---\n".len()..].trim_start_matches('\n'),
        None => markdown,
    }
}

/// The H1 title, the italic standfirst, and the rest. The scan for the
/// standfirst stops at the first `## ` heading so an emphasised sentence in a
/// later chapter is never mistaken for it.
fn split_front_matter(markdown: &str) -> (String, String, String) {
    let lines: Vec<&str> = markdown.lines().collect();
    let title = lines
        .first()
        .and_then(|line| line.strip_prefix("# "))
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let end = lines
        .iter()
        .position(|line| line.starts_with("## "))
        .unwrap_or(lines.len());
    let head = &lines[1.min(lines.len())..end];
    let body = &lines[end..];

    let mut standfirst: Vec<&str> = Vec::new();
    let mut consumed = 0;
    for (index, line) in head.iter().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() {
            if !standfirst.is_empty() {
                break;
            }
            continue;
        }
        standfirst.push(stripped);
        consumed = index + 1;
        if stripped.ends_with('*') {
            break;
        }
    }
    let standfirst_text = standfirst
        .join(" ")
        .trim()
        .trim_matches('*')
        .trim()
        .to_owned();
    let intro = if standfirst.is_empty() {
        head
    } else {
        &head[consumed..]
    };
    let mut rest: Vec<&str> = intro.to_vec();
    rest.extend_from_slice(body);
    (title, standfirst_text, rest.join("\n"))
}

/// The sidebar replaces the inline table of contents.
fn drop_contents_section(body: &str) -> String {
    let Some(start) = body.find("\n## Contents\n") else {
        return body.to_owned();
    };
    let after = start + 1;
    let end = body[after..]
        .find("\n## ")
        .map_or(body.len(), |offset| after + offset);
    format!("{}{}", &body[..start], &body[end..])
}

/// Remove product panels and explicitly marked product-only sections.
fn strip_panels(body: &str) -> String {
    let mut body = body.to_owned();
    while let Some(start) = body.find("<!-- product-only:start -->") {
        let Some(end) = body[start..].find("<!-- product-only:end -->") else {
            // Leave malformed regions intact so the public-name gate refuses them.
            break;
        };
        body.replace_range(start..start + end + "<!-- product-only:end -->".len(), "");
    }
    let opening = format!("> **{PANEL_HEADING}**");
    let mut kept = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == opening {
            while lines.peek().is_some_and(|next| next.starts_with('>')) {
                lines.next();
            }
            continue;
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// The sources link each other by relative file, as a static site reads
/// them; the page links the other page where this build serves it.
fn link_companions(body: &str, variant: Variant) -> String {
    let (guide, evidence, untrusted) = match variant {
        Variant::Console => (
            "/guide",
            "/guide/state-of-the-evidence",
            "/guide/untrusted-context",
        ),
        Variant::Public => (
            "context-window-optimisation.html",
            "state-of-the-evidence.html",
            "untrusted-context.html",
        ),
    };
    body.replace("](state-of-the-evidence.md)", &format!("]({evidence})"))
        .replace("](context-window-optimisation.md)", &format!("]({guide})"))
        .replace("](untrusted-context.md)", &format!("]({untrusted})"))
}

/// `(anchor, label)` for every `## ` heading, in order.
fn chapters(body: &str) -> Vec<(String, String)> {
    let mut slugs = Slugs::default();
    body.lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|label| {
            let label = label.trim().replace('`', "");
            (slugs.next(&label), label)
        })
        .collect()
}

/// Heading anchors as the Python-Markdown `toc` extension made them, so the
/// sources' own in-text links (`#glossary`) keep resolving: lower-cased,
/// punctuation dropped, whitespace and underscores to hyphens, duplicates
/// numbered.
#[derive(Default)]
struct Slugs {
    seen: Vec<String>,
}

impl Slugs {
    fn next(&mut self, label: &str) -> String {
        let cleaned: String = label
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '-' | '_'))
            .collect();
        let base = cleaned
            .split(|c: char| c.is_whitespace() || c == '_')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let mut slug = base.clone();
        let mut n = 1;
        while self.seen.contains(&slug) {
            slug = format!("{base}_{n}");
            n += 1;
        }
        self.seen.push(slug.clone());
        slug
    }
}

/// Markdown to HTML: tables, footnotes and smart punctuation on; every
/// heading given its anchor; tables wrapped so the page never scrolls
/// sideways; footnotes as one numbered list at the end.
fn body_to_html(body: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_SMART_PUNCTUATION;
    let events: Vec<Event> = Parser::new_ext(body, options).collect();

    let mut slugs = Slugs::default();
    let mut out: Vec<Event> = Vec::with_capacity(events.len() + 64);
    let mut footnotes_open = false;
    let mut index = 0;
    while index < events.len() {
        match &events[index] {
            Event::Start(Tag::Heading {
                level,
                classes,
                attrs,
                ..
            }) => {
                let text = heading_text(&events[index + 1..]);
                out.push(Event::Start(Tag::Heading {
                    level: *level,
                    id: Some(slugs.next(&text).into()),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            Event::Start(Tag::Table(_)) => {
                out.push(Event::Html("<div class=\"table-wrap\">".into()));
                out.push(events[index].clone());
            }
            Event::End(TagEnd::Table) => {
                out.push(events[index].clone());
                out.push(Event::Html("</div>".into()));
            }
            Event::FootnoteReference(label) => {
                let label = esc(label);
                out.push(Event::Html(
                    format!(
                        "<sup id=\"fnref:{label}\"><a class=\"footnote-ref\" href=\"#fn:{label}\">{label}</a></sup>"
                    )
                    .into(),
                ));
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                if !footnotes_open {
                    out.push(Event::Html(
                        "<section class=\"footnotes\">\n<hr>\n<ol>\n".into(),
                    ));
                    footnotes_open = true;
                }
                out.push(Event::Html(format!("<li id=\"fn:{}\">", esc(label)).into()));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                out.push(Event::Html("</li>\n".into()));
                let more = events[index + 1..]
                    .iter()
                    .any(|event| matches!(event, Event::Start(Tag::FootnoteDefinition(_))));
                if !more {
                    out.push(Event::Html("</ol>\n</section>\n".into()));
                    footnotes_open = false;
                }
            }
            event => out.push(event.clone()),
        }
        index += 1;
    }
    let mut rendered = String::with_capacity(body.len() * 2);
    html::push_html(&mut rendered, out.into_iter());
    rendered
}

/// The text of a heading whose start event has just been read.
fn heading_text(events: &[Event]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::End(TagEnd::Heading(_)) => break,
            _ => {}
        }
    }
    text
}

/// Product panels become labelled asides, so a reader can skip every one of
/// them without losing the argument.
fn promote_panels(html: &str) -> String {
    let opening = format!("<blockquote>\n<p><strong>{PANEL_HEADING}</strong></p>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&opening) {
        out.push_str(&rest[..start]);
        let after = &rest[start + opening.len()..];
        let Some(close) = after.find("</blockquote>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let _ = write!(
            out,
            "<aside class=\"implication\"><h4>{PANEL_HEADING}</h4>{}</aside>",
            &after[..close]
        );
        rest = &after[close + "</blockquote>".len()..];
    }
    out.push_str(rest);
    out
}

/// Derived from the source, so the counts cannot go stale.
fn masthead_facts(markdown: &str, source: &Source) -> String {
    let is_definition = |line: &&str| line.starts_with("[^") && line.contains("]:");
    let words = markdown
        .lines()
        .filter(|line| !is_definition(line))
        .flat_map(str::split_whitespace)
        .count();
    let references = markdown.lines().filter(is_definition).count();
    let minutes = ((words as f64 / WORDS_PER_MINUTE).round() as usize).max(1);
    let mut facts = vec![
        ("Research current to", source.current_to.to_owned()),
        ("Reading time", format!("about {minutes} minutes")),
    ];
    if references > 0 {
        let references = if source.links_checked {
            format!("{references}, all link-checked")
        } else {
            references.to_string()
        };
        facts.push(("References", references));
    }
    facts
        .iter()
        .map(|(k, v)| format!("    <span class=\"fact\"><b>{k}</b>{v}</span>"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The stylesheet's comments are for its maintainers, not its readers, and
/// one of them names the product; a published page carries the rules only.
fn without_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out.lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
