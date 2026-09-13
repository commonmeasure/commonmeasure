//! Every brief under `work/` has the shape `work/README.md` states and serves
//! a package that exists.
//!
//! A brief that restates a package's status, or names a package that has
//! been closed or renumbered, is the drift `AGENTS.md` §Document ownership
//! forbids: the roadmap owns status, and a brief is a dispatch unit only.

use std::path::{Path, PathBuf};

/// The second-level headings a brief carries, in order.
const HEADINGS: &[&str] = &[
    "## Package",
    "## Goal",
    "## Files",
    "## Done when",
    "## Out of scope",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The `WP-nn` ids that have a `### WP-nn:` heading in the roadmap.
fn roadmap_packages(roadmap: &str) -> Vec<String> {
    roadmap
        .lines()
        .filter_map(|line| line.strip_prefix("### WP-"))
        .filter_map(|rest| rest.split(':').next())
        .map(|number| format!("WP-{number}"))
        .collect()
}

/// Every `WP-nn` token in a text.
fn package_references(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("WP-") {
        let after = &rest[start + 3..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            found.push(format!("WP-{digits}"));
        }
        rest = &after[digits.len()..];
    }
    found
}

#[test]
fn every_brief_has_the_stated_shape_and_serves_an_open_package() {
    let root = repo_root();
    let work = root.join("work");
    assert!(
        work.join("README.md").is_file(),
        "work/README.md states the shape of a brief and must exist"
    );
    let roadmap = std::fs::read_to_string(root.join("ROADMAP.md")).expect("ROADMAP.md is readable");
    let packages = roadmap_packages(&roadmap);
    assert!(
        !packages.is_empty(),
        "the roadmap declares its packages as `### WP-nn:` headings"
    );

    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&work)
        .expect("work/ is readable")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "README.md" || !name.ends_with(".md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("brief is readable");
        let mut cursor = 0;
        for heading in HEADINGS {
            match text[cursor..].find(&format!("\n{heading}\n")) {
                Some(at) => cursor += at + heading.len(),
                None => failures.push(format!("work/{name} lacks `{heading}` in order")),
            }
        }
        let package_section = text
            .split("\n## Package\n")
            .nth(1)
            .and_then(|after| after.split("\n## ").next())
            .unwrap_or("");
        let referenced = package_references(package_section);
        if referenced.is_empty() {
            failures.push(format!("work/{name} names no `WP-nn` under `## Package`"));
        }
        for id in referenced {
            if !packages.contains(&id) {
                failures.push(format!(
                    "work/{name} serves {id}, which ROADMAP.md does not declare"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "briefs out of shape:\n{}",
        failures.join("\n")
    );
}
