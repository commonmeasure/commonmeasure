//! The manifest reader held to the standard's own fixtures
//! (`schema/manifest-tests/`, vendored beside the schema): every valid
//! manifest is accepted with its facts, and every invalid one is rejected,
//! including the ones that pass JSON Schema and fail only the consumer rules
//! of section 8.7 (duplicate key ids, foreign domains) or the application-
//! layer rules of sections 8.5 and 8.6.

use std::path::PathBuf;

use commonmeasure_harness::manifest;
use serde_json::Value;

fn fixtures(kind: &str) -> Vec<(String, Value, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/manifest-tests")
        .join(kind);
    let mut found: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            let bytes = std::fs::read(&path).expect("fixture");
            let document: Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                document,
                bytes,
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn every_valid_manifest_fixture_is_accepted() {
    let valid = fixtures("valid");
    assert!(valid.len() >= 8, "the fixtures have gone missing");
    for (name, document, bytes) in valid {
        let url = document["id"].as_str().expect("a valid manifest has an id");
        let facts = manifest::read(url, &bytes)
            .unwrap_or_else(|reason| panic!("{name} was rejected: {reason}"));
        assert_eq!(facts.id, url);
        assert!(!facts.roles.is_empty());
    }
}

#[test]
fn every_invalid_manifest_fixture_is_rejected_with_a_reason() {
    let invalid = fixtures("invalid");
    assert!(invalid.len() >= 20, "the fixtures have gone missing");
    for (name, document, bytes) in invalid {
        let url = document["id"]
            .as_str()
            .unwrap_or("https://example.com/.well-known/content-telemetry.json");
        let reason = match manifest::read(url, &bytes) {
            Ok(facts) => panic!("{name} was accepted: {facts:?}"),
            Err(reason) => reason,
        };
        assert!(
            !reason.trim().is_empty(),
            "{name}: the rejection names a reason"
        );
    }
}
