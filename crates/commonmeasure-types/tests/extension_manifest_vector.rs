//! The extension manifest and approved-configuration digests
//! (`docs/contracts/extension-manifest.md` §Digest) recomputed from the
//! shared vector, which the hub pins a copy of. The vector's manifest must
//! also be the contract's example, so the two documents cannot drift apart.

use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use serde_json::Value;

const VECTOR: &str = include_str!("../../../docs/contracts/extension-manifest-vector.json");
const CONTRACT: &str = include_str!("../../../docs/contracts/extension-manifest.md");

fn vector() -> Value {
    serde_json::from_str(VECTOR).expect("the vector is JSON")
}

/// The JSON code blocks of the contract, in order.
fn contract_examples() -> Vec<Value> {
    CONTRACT
        .split("```json\n")
        .skip(1)
        .map(|block| {
            let (json, _) = block.split_once("\n```").expect("closed code block");
            serde_json::from_str(json).expect("the contract's example is JSON")
        })
        .collect()
}

#[test]
fn the_vector_digests_are_sha256_over_rfc_8785_canonical_json() {
    let vector = vector();
    for part in ["manifest", "configuration"] {
        let document = &vector[part]["document"];
        assert_eq!(
            canonical_json(document),
            vector[part]["canonical"],
            "{part}"
        );
        assert_eq!(canonical_digest(document), vector[part]["digest"], "{part}");
    }
}

#[test]
fn the_vector_holds_the_contracts_examples() {
    let vector = vector();
    let examples = contract_examples();
    assert!(
        examples
            .iter()
            .any(|example| example["manifest"] == vector["manifest"]["document"]),
        "the contract's example manifest"
    );
    assert!(
        examples
            .iter()
            .any(|example| *example == vector["configuration"]["document"]),
        "the contract's example configuration"
    );
}
