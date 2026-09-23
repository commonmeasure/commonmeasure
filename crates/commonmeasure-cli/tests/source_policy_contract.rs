//! The source policy's published contract held to the loader and the
//! admission check it describes (`docs/contracts/source-policy.md`).
//!
//! Three artefacts sit beside the contract and each is checked here against
//! the code rather than trusted: the JSON Schema is the one the binary
//! derives; every validation vector is accepted or refused by the real binary
//! as the vector says, with the loader's own sentence and the loader's form;
//! and every ruling vector is the ruling the runtime reaches. A second
//! implementation that runs the same vectors is held to the same answers.

use std::path::{Path, PathBuf};
use std::process::Command;

use commonmeasure_harness::grounding::host_of;
use commonmeasure_harness::policy::{PolicyDocument, SessionPolicy};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_types::canonical::canonical_digest;
use commonmeasure_types::{ContextEnvelope, LicenceState};
use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn vectors() -> Value {
    let path = repo_root().join("docs/contracts/source-policy-vectors.json");
    let text = std::fs::read_to_string(&path).expect("the vectors are committed");
    serde_json::from_str(&text).expect("the vectors are JSON")
}

fn schema() -> Value {
    let path = repo_root().join("docs/contracts/source-policy.schema.json");
    let text = std::fs::read_to_string(&path).expect("the schema is committed");
    serde_json::from_str(&text).expect("the schema is JSON")
}

fn text<'a>(vector: &'a Value, key: &str) -> &'a str {
    vector[key]
        .as_str()
        .unwrap_or_else(|| panic!("vector {} has no string {key}", vector["name"]))
}

#[test]
fn the_committed_schema_is_the_one_the_binary_derives() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["policy", "schema"])
        .output()
        .expect("run the binary");
    assert!(output.status.success(), "policy schema failed: {output:?}");
    let committed =
        std::fs::read_to_string(repo_root().join("docs/contracts/source-policy.schema.json"))
            .expect("the schema is committed");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf-8"),
        committed,
        "docs/contracts/source-policy.schema.json is not what `commonmeasure policy schema` \
         prints; regenerate it with that command"
    );
}

#[test]
fn every_validation_vector_is_ruled_by_the_binary_as_the_contract_states() {
    let vectors = vectors();
    let source_name = text(&vectors, "source_name");
    let mut checks_covered = std::collections::BTreeSet::new();
    for vector in vectors["validation"]
        .as_array()
        .expect("validation vectors")
    {
        let name = text(vector, "name");
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join(source_name),
            serde_json::to_vec(&vector["policy"]).expect("serialise"),
        )
        .expect("write the candidate");
        // Run from the candidate's directory and name it relatively, so the
        // refusal names the file as the vector does.
        let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["policy", "check", source_name])
            .current_dir(directory.path())
            .output()
            .expect("run the binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if vector["accepted"] == json!(true) {
            assert!(
                output.status.success(),
                "{name}: the loader refused a document the contract accepts: {stderr}"
            );
            let loader_form = &vector["loader_form"];
            let parsed = PolicyDocument::check(
                &serde_json::to_vec(&vector["policy"]).expect("serialise"),
                Path::new(source_name),
            )
            .unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
            assert_eq!(
                &serde_json::to_value(&parsed).expect("serialise"),
                loader_form,
                "{name}: the loader's form differs from the vector's"
            );
            let digest_line = format!("digest        {}", canonical_digest(loader_form));
            assert!(
                stdout.lines().any(|line| line == digest_line),
                "{name}: policy check did not print {digest_line:?}:\n{stdout}"
            );
        } else {
            assert!(
                !output.status.success(),
                "{name}: the loader accepted a document the contract refuses:\n{stdout}"
            );
            assert!(
                stdout.is_empty(),
                "{name}: a refusal printed an acceptance report:\n{stdout}"
            );
            let check = text(vector, "check");
            checks_covered.insert(check.to_owned());
            if check == "structure" {
                // The parser's own message, which names a line and column
                // of these bytes; only the refusal and its prefix are
                // contractual.
                assert!(
                    stderr.contains(&format!("{source_name} is not a valid policy: ")),
                    "{name}: a structural refusal did not come from the parser: {stderr}"
                );
            } else {
                let refusal = text(vector, "refusal");
                assert_eq!(
                    stderr.trim_end(),
                    format!("commonmeasure: {refusal}"),
                    "{name}: the refusal is not the vector's"
                );
            }
        }
    }
    // The checks the contract's table names, each with a refused document and
    // no vector naming a check outside the table.
    let named: std::collections::BTreeSet<String> = [
        "structure",
        "internal_prefix_not_absolute",
        "internal_prefix_unterminated",
        "scope_match_empty",
        "scope_match_duplicate",
        "principal_name_empty",
        "principal_name_duplicate",
        "principal_os_user_duplicate",
        "principal_key_count",
        "principal_key_empty",
        "principal_scope_missing",
        "allowance_timezone_unknown",
        "allowance_period_duplicate",
        "access_rule_licence_empty",
        "scope_engagement_empty",
        "scope_egress_without_engagement",
        "scope_principal_undeclared",
        "terms_host_empty",
        "terms_reference_empty",
        "terms_host_duplicate",
        "terms_identifier_incomplete",
        "terms_assessment_invalid",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        checks_covered, named,
        "the vectors and the named checks differ"
    );
}

#[test]
fn the_schema_decides_structure_and_nothing_after_it() {
    let schema = schema();
    let validator = jsonschema::JSONSchema::compile(&schema).expect("the schema compiles");
    for vector in vectors()["validation"]
        .as_array()
        .expect("validation vectors")
    {
        let name = text(vector, "name");
        let valid = validator.is_valid(&vector["policy"]);
        assert_eq!(
            json!(valid),
            vector["schema_valid"],
            "{name}: the schema's verdict is not the vector's"
        );
    }
}

#[test]
fn every_ruling_vector_is_the_ruling_the_runtime_reaches() {
    let vectors = vectors();
    let policies = &vectors["ruling_policies"];
    for vector in vectors["rulings"].as_array().expect("ruling vectors") {
        let name = text(vector, "name");
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            serde_json::to_vec(&policies[text(vector, "policy")]).expect("serialise"),
        )
        .expect("write the policy");
        let policy = SessionPolicy::load(home.path(), Some(text(vector, "cwd")))
            .unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
        assert_eq!(
            json!(policy.scope()),
            vector["scope"],
            "{name}: a different scope governs"
        );
        assert_eq!(
            serde_json::to_value(policy.mode()).expect("serialise"),
            vector["mode"],
            "{name}: a different mode governs"
        );

        let url = text(vector, "url");
        let ruling = match vector["licence"].as_str() {
            None => policy.admit_host(url),
            // The envelope a mediated fetch rules on once the source's
            // licence is known: the shape `admit_host` builds, with the
            // declaration.
            Some(reference) => policy.admit(&ContextEnvelope {
                source_url: url.to_owned(),
                host: host_of(url),
                title: None,
                text: Some(String::new()),
                content_hash: None,
                licence: LicenceState::Declared {
                    reference: reference.to_owned(),
                },
                declared_date: None,
                native_metadata: json!({}),
                retrieval_rank: 1,
            }),
        };
        let (kind, reason) = match &ruling {
            Ruling::Allowed => ("allowed", None),
            Ruling::Refused { reason, .. } => ("refused", Some(reason.as_str())),
            Ruling::AllowedWithBreach { reason, .. } => {
                ("allowed_with_breach", Some(reason.as_str()))
            }
        };
        assert_eq!(json!(kind), vector["ruling"], "{name}: {ruling:?}");
        assert_eq!(json!(reason), vector["reason"], "{name}: {ruling:?}");
    }
}
