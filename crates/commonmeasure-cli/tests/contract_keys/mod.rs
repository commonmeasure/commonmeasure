//! The key enumerations `docs/contracts/run-output.md` states, read from the
//! contract itself.
//!
//! One implementation, compiled into each test binary that checks an artefact
//! against the contract, so a key added to an artefact and not to the
//! document fails the suite instead of arriving undocumented — and so the run
//! the binary publishes and the runs committed in the repository are held to
//! one list.

// Each integration test binary compiles this module separately, so anything
// only one of them uses is dead code in the others.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The key names the contract enumerates after `heading`, sorted.
///
/// An enumeration is one sentence of backtick-quoted names ending in a full
/// stop; no key name contains one.
pub fn documented_keys(heading: &str) -> Vec<String> {
    let contract = std::fs::read_to_string(repo_root().join("docs/contracts/run-output.md"))
        .expect("the run output contract is readable");
    let start = contract
        .find(heading)
        .unwrap_or_else(|| panic!("the contract no longer enumerates `{heading}`"));
    let enumeration = contract[start + heading.len()..]
        .split_once('.')
        .expect("the enumeration ends in a full stop")
        .0;
    let mut keys: Vec<String> = enumeration
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    assert!(!keys.is_empty(), "`{heading}` enumerates no key");
    keys.sort();
    keys
}

/// A JSON object's keys, sorted.
pub fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}
