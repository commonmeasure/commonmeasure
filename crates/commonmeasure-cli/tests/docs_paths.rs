//! The documents' repo-relative path references must exist.
//!
//! Drift shows up first as a document naming a file that was deleted or
//! renamed: a dangling `work/…` brief, a moved contract. Prose can go stale
//! in ways only a reader catches; a path either resolves or it does not, so
//! this much is enforced mechanically.
//!
//! This repository is published whole. A document that names or links a file
//! the repository does not carry fails these tests, because the file is
//! absent.
//!
//! The pages under `docs/` also link each other with relative Markdown links,
//! which is how the website is built from that directory alone
//! (`DOCUMENTATION.md`). Each such link must resolve to a file inside
//! `docs/`.

use std::path::{Path, PathBuf};

/// Directories whose documents are scanned. `target/`, `.git/` and worktrees
/// under `.claude/` are never entered.
const SKIP_DIRS: &[&str] = &["target", ".git", ".claude", "node_modules"];

/// A reference is checked only when it starts with one of these top-level
/// entries: that is what makes it repo-relative rather than prose that
/// happens to contain a slash.
///
/// Declared rather than read from the directory, so that deleting a directory
/// outright cannot silently stop its references being checked. The test
/// asserts that every entry at the root is declared, so a new one cannot go
/// unchecked. A declared entry may be absent: `work/` is not in this
/// repository, and stays declared so that a reference into it is reported as
/// dangling rather than read as prose.
const ROOTS: &[&str] = &[
    "browser/",
    "crates/",
    "docs/",
    "work/",
    "demo/",
    "design-tokens/",
    "plugin/",
    "schema/",
    "console/",
    "conformance/",
];

/// The repository's top-level files, checked by exact name: a document naming
/// `ARCHITECTURE.md` after a rename is the drift most likely to go unnoticed,
/// because such a reference carries no directory to give it away.
const TOP_LEVEL_FILES: &[&str] = &[
    "AGENTS.md",
    "ARCHITECTURE.md",
    "CHANGELOG.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "DOCUMENTATION.md",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE.md",
    "NOTICE",
    "README.md",
    "RELEASING.md",
    "SECURITY.md",
    "install.sh",
    "justfile",
];

/// Build outputs at the repository root: produced by `plugin/package.sh`,
/// gitignored, and absent from a fresh checkout. Documents name them to
/// explain the convention, so references into them are not checked — the same
/// reasoning as `IGNORED_PREFIXES`, one level up.
const BUILD_OUTPUTS: &[&str] = &["dist"];

/// Paths that are documented but deliberately absent from a checkout:
/// gitignored build outputs and local-only run artefacts. The documents name
/// them to explain the convention, and this test must not force them into
/// existence.
const IGNORED_PREFIXES: &[&str] = &[
    "demo/arc/home",
    "plugin/bin/commonmeasure-linux-x64",
    "plugin/bin/commonmeasure-macos",
    "plugin/bin/commonmeasure-win",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The document kinds scanned, by how each quotes a path: markdown in
/// backticks, HTML in `<code>` elements, so a dead citation in an HTML
/// document is the same drift as in markdown.
#[derive(Clone, Copy)]
enum Document {
    Markdown,
    Html,
}

impl Document {
    fn of(name: &str) -> Option<Self> {
        if name.ends_with(".md") {
            Some(Self::Markdown)
        } else if name.ends_with(".html") {
            Some(Self::Html)
        } else {
            None
        }
    }
}

fn document_files(dir: &Path, found: &mut Vec<(PathBuf, Document)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                document_files(&path, found);
            }
        } else if let Some(kind) = Document::of(&name) {
            found.push((path, kind));
        }
    }
}

/// The quoted spans of a document: backtick-quoted in markdown, the contents
/// of `<code>…</code>` elements in HTML (an opening tag may carry attributes).
/// HTML is not split on backticks: a template literal in an inline script is
/// not a citation.
fn quoted_spans(text: &str, kind: Document) -> Vec<&str> {
    match kind {
        Document::Markdown => text.split('`').skip(1).step_by(2).collect(),
        Document::Html => {
            let mut spans = Vec::new();
            let mut rest = text;
            while let Some(start) = rest.find("<code") {
                let after_tag = &rest[start + "<code".len()..];
                let Some(open_end) = after_tag.find('>') else {
                    break;
                };
                let body = &after_tag[open_end + 1..];
                let Some(close) = body.find("</code>") else {
                    break;
                };
                spans.push(&body[..close]);
                rest = &body[close + "</code>".len()..];
            }
            spans
        }
    }
}

/// Quoted spans that look like repo-relative paths. Only spans made entirely
/// of path characters count: anything with spaces, globs, variables, markup
/// entities or a home prefix is prose or an example, not a checkable
/// reference.
fn path_references(text: &str, kind: Document) -> Vec<String> {
    let mut references = Vec::new();
    for span in quoted_spans(text, kind) {
        let candidate = span.trim_end_matches('/');
        let repo_relative = ROOTS.iter().any(|root| candidate.starts_with(root))
            || TOP_LEVEL_FILES.contains(&candidate);
        if !repo_relative {
            continue;
        }
        if !candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
        {
            continue;
        }
        if IGNORED_PREFIXES
            .iter()
            .any(|prefix| candidate.starts_with(prefix))
        {
            continue;
        }
        references.push(candidate.to_owned());
    }
    references
}

#[test]
fn every_repo_relative_path_a_document_names_exists() {
    let root = repo_root();
    let mut documents = Vec::new();
    document_files(&root, &mut documents);
    assert!(
        documents
            .iter()
            .any(|(path, _)| *path == root.join("README.md")),
        "the scan must reach the repository root documents"
    );

    // What may be referenced is the repository root itself, minus what is not
    // entered and what the build produces. Without this the declared list is a
    // sealed one: a new top-level directory or document would be referenced
    // freely and never checked.
    let mut present: Vec<String> = std::fs::read_dir(&root)
        .expect("the repository root is readable")
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.path().is_dir() {
                true => format!("{name}/"),
                false => name,
            }
        })
        // Dotfiles are tooling and local configuration, not the documented
        // surface, and `.env` exists only on a machine that has credentials.
        .filter(|name| !name.starts_with('.'))
        .filter(|name| {
            let entry = name.trim_end_matches('/');
            !SKIP_DIRS.contains(&entry) && !BUILD_OUTPUTS.contains(&entry)
        })
        .collect();
    present.sort();
    let undeclared: Vec<&String> = present
        .iter()
        .filter(|name| !ROOTS.contains(&name.as_str()) && !TOP_LEVEL_FILES.contains(&name.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "the repository root holds entries this test does not check references against; \
         classify each as a checked root, a build output or a skipped directory: {undeclared:?}"
    );

    let mut dangling = Vec::new();
    for (document, kind) in &documents {
        let text = std::fs::read_to_string(document).expect("readable document");
        for reference in path_references(&text, *kind) {
            if !root.join(&reference).exists() {
                dangling.push(format!(
                    "{} names {reference}, which does not exist",
                    document.strip_prefix(&root).unwrap_or(document).display()
                ));
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "dangling path references:\n{}",
        dangling.join("\n")
    );
}

/// The targets of relative Markdown links in a page: the parenthesised part
/// after `](`, without any fragment. Absolute URLs and in-page anchors are
/// not relative links and are skipped.
fn relative_link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        let target = after[..end].split('#').next().unwrap_or("");
        if !target.is_empty() && !target.contains("://") && !target.starts_with('/') {
            targets.push(target.to_owned());
        }
        rest = &after[end..];
    }
    targets
}

#[test]
fn every_relative_link_under_docs_resolves_inside_docs() {
    let root = repo_root();
    let docs = root.join("docs");
    let mut documents = Vec::new();
    document_files(&docs, &mut documents);
    let mut broken = Vec::new();
    for (document, kind) in &documents {
        if !matches!(kind, Document::Markdown) {
            continue;
        }
        let text = std::fs::read_to_string(document).expect("readable document");
        let directory = document.parent().expect("a document has a directory");
        for target in relative_link_targets(&text) {
            let resolved = directory.join(&target);
            let inside = resolved
                .canonicalize()
                .ok()
                .is_some_and(|path| path.starts_with(docs.canonicalize().expect("docs exists")));
            if !resolved.is_file() || !inside {
                broken.push(format!(
                    "{} links {target}, which does not resolve to a file inside docs/",
                    document.strip_prefix(&root).unwrap_or(document).display()
                ));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "relative links that do not resolve:\n{}",
        broken.join("\n")
    );
}

/// Relative Markdown links outside `docs/` (the README, the plugin and
/// console pages, the demonstration bundles) must resolve to a file or
/// directory in the repository, which is what fails when a page links a
/// file kept outside it.
#[test]
fn every_relative_link_outside_docs_resolves() {
    let root = repo_root();
    let mut documents = Vec::new();
    document_files(&root, &mut documents);
    let docs = root.join("docs");
    let mut broken = Vec::new();
    for (document, kind) in &documents {
        if !matches!(kind, Document::Markdown) || document.starts_with(&docs) {
            continue;
        }
        let text = std::fs::read_to_string(document).expect("readable document");
        let directory = document.parent().expect("a document has a directory");
        for target in relative_link_targets(&text) {
            if target.starts_with("mailto:") || directory.join(&target).exists() {
                continue;
            }
            broken.push(format!(
                "{} links {target}, which does not exist",
                document.strip_prefix(&root).unwrap_or(document).display()
            ));
        }
    }
    assert!(
        broken.is_empty(),
        "relative links that do not resolve:\n{}",
        broken.join("\n")
    );
}
