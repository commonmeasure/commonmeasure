//! Every document says which domain owns it and who it is for, and no file
//! names a private document.
//!
//! Each Markdown document opens with YAML front matter carrying `domain` and
//! `audience`; a page the website publishes under `/docs/` for an operator or
//! an integrator also carries `section` (`DOCUMENTATION.md`, rule 1). The
//! navigation and the per-domain reviews read
//! these fields, so a missing or misspelt value would drop a page silently.
//!
//! This repository is published whole at each release, while the product
//! record and the operations runbooks stay private. A file that names one of
//! those documents points readers at something they cannot open.
//!
//! Both checks read the files git tracks or would track: committed, staged,
//! and untracked but not ignored. Ignored build output and local run
//! artefacts are not documents of the repository.

use std::path::{Path, PathBuf};
use std::process::Command;

const DOMAINS: &[&str] = &[
    "edge",
    "hub",
    "network",
    "marketplace",
    "extensions",
    "shared",
];
const AUDIENCES: &[&str] = &[
    "operator",
    "integrator",
    "contributor",
    "internal",
    "reader",
];
const SECTIONS: &[&str] = &["get-started", "use", "integrate", "reference"];

/// Markdown files that carry no front matter, with the reason. A prefix ending
/// in `/` exempts the directory beneath it; a name without `/` exempts that
/// file name wherever it appears.
const EXEMPT: &[(&str, &str)] = &[
    (
        "CHANGELOG.md",
        "release notes; the release workflow reads their headings",
    ),
    ("CLAUDE.md", "read by Claude Code as instructions"),
    ("LICENSE.md", "licence text, never edited"),
    (
        "SKILL.md",
        "Agent Skill format, whose front matter keys are fixed",
    ),
    (
        "plugin/commands/",
        "Claude Code command files, served verbatim as prompts",
    ),
    ("plugin/skills/", "Claude Code skill files"),
    (
        "plugin/m365-copilot/instructions.md",
        "packaged verbatim as the declarative agent's instructions",
    ),
    (
        "design-tokens/README.md",
        "vendored from the website release; must stay byte-identical",
    ),
    (
        "demo/corpus/",
        "fixture documents a job reads as source content",
    ),
    (
        "demo/commerce/corpus/",
        "fixture documents a job reads as source content",
    ),
    (
        "demo/injection/corpus/",
        "fixture documents a job reads as source content",
    ),
    (
        "demo/specialist/corpus/",
        "fixture documents a job reads as source content",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// Repository-relative paths of the files git tracks or would track. Fails
/// rather than scanning nothing when git cannot list them.
fn repository_files(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .expect("git runs; these checks read the file list from git");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut files: Vec<String> = String::from_utf8(output.stdout)
        .expect("git lists UTF-8 paths")
        .split('\0')
        .filter(|path| !path.is_empty())
        // A tracked file deleted in the working tree is listed but has nothing to read.
        .filter(|path| root.join(path).is_file())
        .map(str::to_owned)
        .collect();
    files.sort();
    files.dedup();
    files
}

fn exemption(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    EXEMPT.iter().find_map(|(pattern, reason)| {
        let matches = if pattern.ends_with('/') {
            path.starts_with(pattern)
        } else if pattern.contains('/') {
            path == *pattern
        } else {
            name == *pattern
        };
        matches.then_some(*reason)
    })
}

/// The top-level `key: value` pairs of a document's opening YAML block, or
/// `None` when it has none. Values may be quoted; nested YAML is not used in
/// this repository's front matter and is not parsed.
fn front_matter(text: &str) -> Option<Vec<(String, String)>> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut fields = Vec::new();
    for line in lines {
        let line = line.trim_end();
        if line == "---" {
            return Some(fields);
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            fields.push((key.trim().to_owned(), value.to_owned()));
        }
    }
    None
}

/// Whether the website publishes the page under `/docs/`: `docs/` except the
/// guides, which are its `/guides/` section.
fn published_under_docs(path: &str) -> bool {
    path.starts_with("docs/") && !path.starts_with("docs/guide/")
}

/// What is wrong with one document's front matter, if anything.
fn front_matter_problems(path: &str, text: &str) -> Vec<String> {
    let Some(fields) = front_matter(text) else {
        return vec![format!("{path} has no front matter")];
    };
    let value = |key: &str| {
        let mut values = fields
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str());
        let first = values.next();
        (first, values.next().is_some())
    };
    let mut problems = Vec::new();
    let mut require = |key: &str, allowed: &[&str]| -> Option<String> {
        match value(key) {
            (None, _) => problems.push(format!("{path} has no `{key}`")),
            (Some(_), true) => problems.push(format!("{path} has more than one `{key}`")),
            (Some(found), false) if !allowed.contains(&found) => problems.push(format!(
                "{path} has `{key}: {found}`; expected one of {}",
                allowed.join(", ")
            )),
            (Some(found), false) => return Some(found.to_owned()),
        }
        None
    };
    require("domain", DOMAINS);
    let audience = require("audience", AUDIENCES);
    let wants_section = published_under_docs(path)
        && matches!(audience.as_deref(), Some("operator" | "integrator"));
    if wants_section {
        let section = require("section", SECTIONS);
        let reference_only = path.starts_with("docs/contracts/") || path == "docs/GLOSSARY.md";
        if reference_only && section.as_deref().is_some_and(|s| s != "reference") {
            problems.push(format!(
                "{path} is a contract or the glossary; its `section` is `reference`"
            ));
        }
    } else if value("section").0.is_some() {
        problems.push(format!(
            "{path} has a `section`, which only an operator or integrator page under docs/ (not docs/guide/) carries"
        ));
    }
    problems
}

#[test]
fn every_document_declares_its_domain_and_audience() {
    let root = repo_root();
    let documents: Vec<String> = repository_files(&root)
        .into_iter()
        .filter(|path| path.ends_with(".md") && exemption(path).is_none())
        .collect();
    assert!(
        documents.iter().any(|path| path == "README.md")
            && documents
                .iter()
                .any(|path| path.starts_with("docs/contracts/")),
        "the scan must reach the root README and the contracts"
    );
    let mut problems = Vec::new();
    for path in &documents {
        let text = std::fs::read_to_string(root.join(path)).expect("readable document");
        problems.extend(front_matter_problems(path, &text));
    }
    assert!(
        problems.is_empty(),
        "front matter (DOCUMENTATION.md, rule 1):\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_front_matter_rules_reject_what_they_should() {
    let good =
        "---\ntitle: X\ndomain: edge\naudience: integrator\nsection: reference\n---\n\n# X\n";
    assert!(front_matter_problems("docs/contracts/x.md", good).is_empty());
    assert!(!front_matter_problems("docs/contracts/x.md", "# X\n").is_empty());
    let unknown = "---\ndomain: edges\naudience: integrator\nsection: reference\n---\n";
    assert!(!front_matter_problems("docs/contracts/x.md", unknown).is_empty());
    let missing_section = "---\ndomain: edge\naudience: operator\n---\n";
    assert!(!front_matter_problems("docs/X.md", missing_section).is_empty());
    assert!(front_matter_problems("browser/README.md", missing_section).is_empty());
    let guide_section = "---\ndomain: shared\naudience: reader\nsection: use\n---\n";
    assert!(!front_matter_problems("docs/guide/x.md", guide_section).is_empty());
    let contract_use = "---\ndomain: hub\naudience: integrator\nsection: use\n---\n";
    assert!(!front_matter_problems("docs/contracts/x.md", contract_use).is_empty());
    let unclosed = "---\ndomain: edge\naudience: operator\n";
    assert!(!front_matter_problems("README.md", unclosed).is_empty());
}

/// Names that only the private repositories' documents carry: the product
/// record's file names and the directories of the company and operations
/// repositories. Assembled here so that this file does not itself contain
/// them.
fn private_document_names() -> Vec<String> {
    let record = [
        "PRODUCT",
        "DECISIONS",
        "ROADMAP",
        "ROADMAP-COMPLETED",
        "ROADMAP-DEFERRED",
        "READ-A-RUN",
        "READ-A-HOLDOUT",
        "READ-A-COMPARISON",
    ]
    .map(|stem| format!("{stem}.{}", "md"));
    let product = [
        "roadmap",
        "qa",
        "work",
        "knowledge-base",
        "evidence",
        "design",
        "docs",
    ]
    .map(|dir| ["product", dir, ""].join("/"));
    let repositories = ["assessments", "intel", "pitch", "runbooks"].map(|dir| format!("{dir}/"));
    record
        .into_iter()
        .chain(product)
        .chain(repositories)
        .collect()
}

/// Occurrences of `name` in `text` that are not the tail of a longer word, so
/// `SUBPRODUCT.md` or `…_runbooks/` do not count. A path prefix still counts:
/// `../company/` before a name is exactly what the check is for.
fn names(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        !text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    })
}

#[test]
fn no_file_names_a_private_document() {
    let root = repo_root();
    let forbidden = private_document_names();
    let mut found = Vec::new();
    let mut scanned = 0;
    for path in repository_files(&root) {
        // The changelog records history, including documents since moved out.
        if path == "CHANGELOG.md" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue; // not text
        };
        scanned += 1;
        for name in &forbidden {
            if names(&path, name) || names(&text, name) {
                found.push(format!("{path} names {name}"));
            }
        }
    }
    assert!(
        scanned > 100,
        "the scan must reach the repository's text files"
    );
    assert!(
        found.is_empty(),
        "files naming a document of the private company or operations repositories:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_private_name_match_ignores_longer_words() {
    let roadmap = format!("{}.md", "ROADMAP");
    assert!(names(&format!("see `{roadmap}` §Edge"), &roadmap));
    assert!(names(&format!("../company/{roadmap}"), &roadmap));
    assert!(!names(&format!("SUB{roadmap}"), &roadmap));
    assert!(!names(&format!("MY-{roadmap}"), &roadmap));
}
