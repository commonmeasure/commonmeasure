//! The routing experiment's registered suites (`demo/jobs/commerce-routing/`)
//! hold still by test, not by claim. Every suite must parse through
//! the real loader, and every rubric item must be coverable from the mixed
//! commerce corpus by the coverage evaluator itself — so a suite edit the
//! loader rejects, or a rubric phrase drifting away from the corpus, fails
//! the offline gate here instead of failing loudly at the first fitting run
//! and silently as a claim until then.

use std::path::Path;

use commonmeasure_inference::ContextPart;
use commonmeasure_runtime::coverage::{CoverageInput, coverage};
use commonmeasure_runtime::load_suite;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One part per corpus document, exactly the text the internal adapter would
/// retain — the whole mixed corpus as an admitted window, so coverability is
/// judged by the evaluator that will judge the runs.
fn corpus_window(root: &Path) -> Vec<ContextPart> {
    fn collect(root: &Path, directory: &Path, parts: &mut Vec<ContextPart>) {
        for entry in std::fs::read_dir(directory).expect("read corpus directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                collect(root, &path, parts);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                let text = std::fs::read_to_string(&path).expect("read corpus document");
                parts.push(ContextPart {
                    source_ref: path
                        .strip_prefix(root)
                        .expect("document under corpus root")
                        .display()
                        .to_string(),
                    content_hash: format!("sha256:{:x}", Sha256::digest(text.as_bytes())),
                    text,
                });
            }
        }
    }
    let mut parts = Vec::new();
    collect(root, root, &mut parts);
    parts.sort_by(|a, b| a.source_ref.cmp(&b.source_ref));
    parts
}

#[test]
fn every_registered_routing_suite_loads_and_its_rubric_is_coverable_from_the_corpus() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let window = corpus_window(&repository.join("demo/commerce/corpus"));

    // The registration directory also holds its own artefacts (split.json,
    // rule.json); a suite is a file named for its supply condition.
    let mut suites: Vec<_> = std::fs::read_dir(repository.join("demo/jobs/commerce-routing"))
        .expect("read the registered suites")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        ["product-", "brand-", "editorial-", "mixed-"]
                            .iter()
                            .any(|condition| name.starts_with(condition))
                    })
        })
        .collect();
    suites.sort();
    assert_eq!(
        suites.len(),
        48,
        "the registration declares twelve questions under four supply conditions"
    );

    for path in suites {
        let suite = load_suite(&path)
            .unwrap_or_else(|error| panic!("{} must load: {error}", path.display()));
        let rubric = suite
            .coverage_rubric
            .as_ref()
            .unwrap_or_else(|| panic!("{} declares no coverage rubric", path.display()));
        let record = coverage(&CoverageInput {
            rubric: Some(rubric),
            window: Some(&window),
        });
        assert_eq!(
            record["unmeasured"],
            Value::Null,
            "{}: coverage must be measurable over the mixed corpus",
            path.display()
        );
        let uncovered: Vec<String> = record["items"]
            .as_array()
            .expect("a measured record carries its items")
            .iter()
            .filter(|item| item["covered"] != Value::Bool(true))
            .map(|item| item["item"].to_string())
            .collect();
        assert!(
            uncovered.is_empty(),
            "{}: rubric items no corpus document covers: {}",
            path.display(),
            uncovered.join(", ")
        );
    }
}
