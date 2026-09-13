//! A cited open finding must resolve to a row in `docs/qa/OPEN.md`.
//!
//! `OPEN.md` is the one home for open findings and a closed row is deleted
//! rather than kept (`AGENTS.md`, "Document ownership"), so a document that
//! cites `QA-62` after the row has gone is citing nothing. A reader cannot
//! tell that from the text; this test can.
//!
//! The scan reaches the documents and the committed artefacts alike. A
//! finding id written into a recon manifest's redaction note is copied
//! verbatim into every run that serves that recording, so it outlives the row
//! in sealed evidence, where deleting it costs a republish. `responses/` is
//! left out: those bytes are a provider's own, and a `QA-`-shaped string
//! there is that provider's text rather than a citation this repository
//! makes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directories whose markdown is scanned. `docs/` holds the contracts and
/// knowledge base, `work/` the live briefs; those are the two places
/// `AGENTS.md` lets a finding be named in prose.
const SCANNED: &[&str] = &["docs", "work"];

/// The recon manifests, whose prose is copied into every run that serves a
/// recording they describe.
const MANIFESTS: &[&str] = &[
    "demo/recon/replay-manifest.json",
    "demo/recon/fixture-manifest.json",
];

/// The artefacts of a committed run that carry prose from elsewhere: the
/// summary, the sealed manifest, the replay binding and the evidence log.
const RUN_ARTEFACTS: &[&str] = &[
    "summary.json",
    "manifest.json",
    "replay.json",
    "evidence.ndjson",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn markdown_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            markdown_files(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "md") {
            found.push(path);
        }
    }
}

/// Every `QA-<digits>` in `text`. A longer digit run is taken whole, so
/// `QA-6` does not match inside `QA-62`.
fn cited_ids(text: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("QA-") {
        let after = &rest[start + "QA-".len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            ids.insert(format!("QA-{digits}"));
        }
        rest = &after[digits.len()..];
    }
    ids
}

/// The ids `OPEN.md` holds a row for: the first cell of each table row.
fn open_rows(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("| "))
        .filter_map(|line| line.split_once(" |"))
        .map(|(id, _)| id.trim().to_owned())
        .filter(|id| id.starts_with("QA-"))
        .collect()
}

/// The published run directories committed to this repository.
fn published_runs(dir: &Path, found: &mut Vec<PathBuf>) {
    if dir.join("summary.json").is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.path().is_dir() {
            published_runs(&entry.path(), found);
        }
    }
}

#[test]
fn every_cited_open_finding_resolves_to_a_row() {
    let root = repo_root();
    let open = root.join("docs/qa/OPEN.md");
    let rows = open_rows(&std::fs::read_to_string(&open).expect("docs/qa/OPEN.md is readable"));

    let mut documents = Vec::new();
    for directory in SCANNED {
        markdown_files(&root.join(directory), &mut documents);
    }
    assert!(
        documents.iter().any(|path| path == &open),
        "the scan must reach docs/qa/OPEN.md itself"
    );

    for manifest in MANIFESTS {
        let path = root.join(manifest);
        assert!(path.is_file(), "{manifest} is missing");
        documents.push(path);
    }
    let mut runs = Vec::new();
    published_runs(&root.join("demo/output"), &mut runs);
    assert!(
        runs.len() > 50,
        "the scan must reach the committed runs; found {}",
        runs.len()
    );
    for run in runs {
        for artefact in RUN_ARTEFACTS {
            let path = run.join(artefact);
            if path.is_file() {
                documents.push(path);
            }
        }
    }

    let mut dangling = Vec::new();
    for document in &documents {
        let text = std::fs::read_to_string(document).expect("readable document");
        for id in cited_ids(&text) {
            if !rows.contains(&id) {
                dangling.push(format!(
                    "{} cites {id}, which has no row in docs/qa/OPEN.md",
                    document.strip_prefix(&root).unwrap_or(document).display()
                ));
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "dangling open-finding citations:\n{}",
        dangling.join("\n")
    );
}

#[test]
fn the_scan_reads_the_ids_it_claims_to() {
    assert_eq!(
        cited_ids("QA-62 and QA-104, but not QA- or QAX-1"),
        BTreeSet::from(["QA-62".to_owned(), "QA-104".to_owned()])
    );
    assert_eq!(
        open_rows("| ID | Sev |\n| QA-62 | P2 | text |\n| QA-74 | P2 | text |"),
        BTreeSet::from(["QA-62".to_owned(), "QA-74".to_owned()])
    );
}
