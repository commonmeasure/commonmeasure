//! Keep release-history advice out of the current product documentation and copy.
//! Exceptions name the exact passage and why it remains true or necessary.

use std::path::Path;
use std::process::Command;

const ALLOWLIST: &str = include_str!("docs_release_history_allowlist.tsv");
const PHRASES: &[&str] = &[
    "no longer",
    "formerly",
    "previously",
    "used to",
    "legacy",
    "backward compatibility",
    "backwards compatibility",
    "backward-compatible",
    "backwards-compatible",
    "upgrading from",
    "upgrade from",
    "upgrades from",
    "older version",
    "older release",
    "older build",
    "earlier version",
    "earlier release",
    "older binary",
    "older binaries",
    "older edges",
    "older edge",
    "older versions",
    "older releases",
    "older builds",
    "earlier binary",
    "earlier binaries",
    "earlier versions",
    "earlier releases",
    "earlier build",
    "previous versions",
    "previous releases",
    "since 0.",
    "since v0.",
    "before 0.",
    "before v0.",
    "version 0.",
    "registered by 0.",
];

fn surface(path: &str) -> bool {
    // Release records, third-party assets and evidence have their own history.
    if matches!(path, "CHANGELOG.md" | "RELEASING.md" | "LICENSE.md")
        || path.starts_with("conformance/")
        || path.starts_with("design-tokens/")
        || path.contains("/corpus/")
        || path.contains("/fixtures/")
        || path.contains("/golden/")
        || path.starts_with("console/fonts/")
        || path.ends_with(".min.js")
        || path.contains("docs_release_history")
    {
        return false;
    }
    let extension = path.rsplit('.').next().unwrap_or("");
    matches!(
        extension,
        "md" | "html" | "rs" | "js" | "mjs" | "ts" | "tsx" | "sh" | "txt" | "json"
    )
}

fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn hits(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for phrase in PHRASES {
        for (at, _) in text.match_indices(phrase) {
            // Do not match identifiers such as `legacy_scope` or words like `unused`.
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let before = text[..at].chars().next_back();
            let after = text[at + phrase.len()..].chars().next();
            if !before.is_some_and(word) && (phrase.ends_with('.') || !after.is_some_and(word)) {
                found.push(at);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

#[test]
fn current_copy_does_not_narrate_release_history() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .expect("git lists repository files");
    assert!(output.status.success(), "git ls-files failed");
    let mut files: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .split('\0')
        .filter(|path| surface(path) && root.join(path).is_file())
        .collect();
    files.sort_unstable();
    files.dedup();
    assert!(
        files.contains(&"README.md") && files.contains(&"crates/commonmeasure-cli/src/main.rs")
    );
    let exceptions: Vec<_> = ALLOWLIST
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(
                fields.len(),
                3,
                "allowlist needs path, passage, reason: {line}"
            );
            assert!(fields.iter().all(|field| !field.trim().is_empty()));
            (fields[0], normalise(fields[1]), fields[2])
        })
        .collect();
    let mut used = vec![false; exceptions.len()];
    let mut problems = Vec::new();
    for path in files {
        let text = normalise(&std::fs::read_to_string(root.join(path)).expect("readable text"));
        for at in hits(&text) {
            let mut allowed = false;
            for (index, (file, passage, _)) in exceptions.iter().enumerate() {
                if *file == path
                    && text
                        .match_indices(passage)
                        .any(|(start, _)| start <= at && at < start + passage.len())
                {
                    used[index] = true;
                    allowed = true;
                }
            }
            if !allowed {
                let excerpt: String = text[at..].chars().take(140).collect();
                problems.push(format!("{path}: {excerpt}"));
            }
        }
    }
    for (index, (path, passage, _)) in exceptions.iter().enumerate() {
        if !used[index] {
            problems.push(format!("stale exception: {path}: {passage}"));
        }
    }
    assert!(
        problems.is_empty(),
        "release-history copy (review or add a reasoned exception):\n{}",
        problems.join("\n")
    );
}

#[test]
fn history_matcher_covers_wrapped_copy_and_ignores_identifiers() {
    assert!(!hits(&normalise("Earlier\nbinaries may refuse this host")).is_empty());
    assert!(!hits(&normalise("Available since 0.3.5")).is_empty());
    assert!(!hits(&normalise("This is no\nlonger supported")).is_empty());
    assert!(hits("legacy_scope contextops-manifest/v5 2025-11-25 1.0").is_empty());
    assert!(!surface("CHANGELOG.md"));
    assert!(!surface("demo/corpus/source.md"));
    assert!(surface("plugin/commands/review.md"));
    assert!(surface("crates/commonmeasure-cli/tests/install_e2e.rs"));
}
