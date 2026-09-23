//! Fixtures with expected outcomes for the declaration reader: the
//! attachment draft's own example `robots.txt`, and a host whose
//! `robots.txt`, licence and `Content-Signal` line contradict each other.
//! Each fixture directory holds the documents and an `expected.json` naming,
//! per token and path, the group selected, whether the path is crawlable,
//! and the effective preference per category.

use std::path::PathBuf;

use commonmeasure_harness::declarations::{
    self, Category, Effective, PRODUCT_TOKEN, StatementSource,
};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/declarations")
}

fn effective_word(effective: Effective) -> &'static str {
    match effective {
        Effective::Allow => "allow",
        Effective::Disallow => "disallow",
        Effective::Unknown => "unknown",
    }
}

/// The source as the record spells it.
fn source_word(source: StatementSource) -> String {
    serde_json::to_value(source)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("a statement source serialises to a string")
}

#[test]
fn every_fixture_reads_as_its_expected_outcomes_say() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures()).expect("fixtures directory") {
        let dir = entry.expect("entry").path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let robots = std::fs::read_to_string(dir.join("robots.txt")).expect("robots.txt");
        let licence = std::fs::read_to_string(dir.join("license.xml")).ok();
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap())
                .expect("expected.json");
        let file = declarations::parse_robots(&robots);
        for case in expected["cases"].as_array().expect("cases") {
            let token = case["token"].as_str().unwrap_or(PRODUCT_TOKEN);
            let path = case["path"].as_str().expect("path");
            let reading = file.read(token, path);
            let label = format!("{name}: {token} at {path}");
            assert_eq!(
                reading.group.as_deref(),
                case["group"].as_str(),
                "{label}: group"
            );
            assert_eq!(
                reading.crawlable,
                case["crawlable"].as_bool(),
                "{label}: crawlable"
            );
            let mut statements = reading.statements.clone();
            if let Some(licence_url) = case["licence"].as_str() {
                assert!(
                    reading.licences.iter().any(|url| url == licence_url),
                    "{label}: the licence is named by robots.txt"
                );
                let document = declarations::parse_rsl(licence.as_deref().expect("license.xml"))
                    .expect("the licence parses");
                let page = format!("https://{name}.example{path}");
                let content = document
                    .content_for(&page)
                    .expect("a content entry matches");
                statements.extend(declarations::licence_terms(content, licence_url).statements);
            }
            let effective = declarations::combine(&statements);
            for category in Category::ALL {
                assert_eq!(
                    effective_word(effective[&category]),
                    case["effective"][category.label()]
                        .as_str()
                        .unwrap_or("unknown"),
                    "{label}: {}",
                    category.label()
                );
                if let Some(sources) = case["sources"][category.label()].as_array() {
                    let mut found: Vec<String> = statements
                        .iter()
                        .filter(|s| s.category == category)
                        .map(|s| source_word(s.source))
                        .collect();
                    found.sort();
                    found.dedup();
                    let mut wanted: Vec<&str> = sources.iter().filter_map(Value::as_str).collect();
                    wanted.sort();
                    assert_eq!(found, wanted, "{label}: sources for {}", category.label());
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 5, "the fixtures have gone missing");
}
