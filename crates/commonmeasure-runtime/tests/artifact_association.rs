//! Synthetic offline evidence checks through the public artifact API.
use commonmeasure_runtime::artifact::{self, EvidenceInput, VerifyTarget};
use commonmeasure_types::canonical::{canonical_digest, canonical_json, sha256_digest};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

fn root() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap();
    (temp, path)
}

fn request(session: &str) -> Value {
    json!({"session":{"namespace":"urn:example:local","id":session,"edge":{"issuer":"urn:example:operator","installation_id":"edge-a"}},"role":"drafting","assertion":{"basis":"agent_declaration","actor":"urn:example:agent"}})
}

fn setup(root: &Path, evidence: bool) -> (PathBuf, PathBuf, Value) {
    let store = root.join("store");
    let file = root.join("document.docx");
    fs::write(&file, b"synthetic file bytes\0\xff").unwrap();
    artifact::init_store(&store, None).unwrap();
    let association = artifact::associate(&store, &request("session-a")).unwrap();
    let inputs = if evidence {
        let log = root.join("source.ndjson");
        fs::write(&log, concat!("{\"seq\":1,\"event\":\"crossing_observed\",\"payload\":{\"session_id\":\"session-a\",\"url\":\"https://example.invalid/\"}}\n", "{\"session_id\":\"session-a\",\"data\":\"é\\r\\n\"}\n")).unwrap();
        vec![EvidenceInput {
            association_id: association["record_id"].as_str().unwrap().into(),
            path: log,
        }]
    } else {
        vec![]
    };
    let snapshot = artifact::snapshot_file(&store, &file, &inputs).unwrap();
    let bundle =
        artifact::export_bundle(&store, snapshot["snapshot_id"].as_str().unwrap()).unwrap();
    (store, file, bundle)
}

fn check(bundle: &Value, file: &Path) -> Value {
    artifact::verify_bundle(bundle, &VerifyTarget::File(file.into()), None).unwrap()
}

#[test]
fn portable_bytes_survive_original_store_and_log_deletion_and_artifact_relocation() {
    let (_temp, root) = root();
    let (store, file, bundle) = setup(&root, true);
    let encoded = serde_json::to_vec(&bundle).unwrap();
    let bundle_path = root.join("bundle.json");
    fs::write(&bundle_path, encoded).unwrap();
    fs::remove_dir_all(store).unwrap();
    fs::remove_file(root.join("source.ndjson")).unwrap();
    let moved = root.join("renamed-file.bin");
    fs::rename(file, &moved).unwrap();
    let decoded = artifact::read_json(&bundle_path).unwrap();
    let report = check(&decoded, &moved);
    assert_eq!(report["valid"], true);
    assert_eq!(report["signature"], "absent");
    assert_eq!(report["trust"], "not_evaluated");
    assert_eq!(report["checks"]["evidence"]["status"], "matched");
}

#[test]
fn changed_artifact_role_identity_and_evidence_are_independent_failures() {
    let (_temp, root) = root();
    let (_, file, bundle) = setup(&root, true);
    let mut changed = bundle.clone();
    changed["associations"][0]["role"] = json!("review");
    assert_eq!(
        check(&changed, &file)["checks"]["associations"]["valid"],
        false
    );
    let mut changed = bundle.clone();
    changed["identity"]["created_at"] = json!("2020-01-01T00:00:00Z");
    assert_eq!(check(&changed, &file)["checks"]["identity"]["valid"], false);
    let mut changed = bundle.clone();
    changed["evidence"][0]["bytes"] = json!("{\"session_id\":\"session-a\",\"changed\":true}\n");
    assert_eq!(check(&changed, &file)["checks"]["evidence"]["valid"], false);
    fs::write(&file, b"changed saved bytes").unwrap();
    assert_eq!(check(&bundle, &file)["checks"]["content"]["valid"], false);
}

#[test]
fn refreshed_digests_cannot_override_an_expected_snapshot_pin() {
    let (_temp, root) = root();
    let (_, file, mut bundle) = setup(&root, true);
    let expected = bundle["snapshot_digest"].as_str().unwrap().to_owned();
    bundle["associations"][0]["role"] = json!("review");
    bundle["snapshot"]["associations"][0]["digest"] =
        json!(canonical_digest(&bundle["associations"][0]));
    bundle["snapshot_digest"] = json!(canonical_digest(&bundle["snapshot"]));
    assert_eq!(check(&bundle, &file)["valid"], true);
    let pinned =
        artifact::verify_bundle(&bundle, &VerifyTarget::File(file), Some(&expected)).unwrap();
    assert_eq!(pinned["valid"], false);
    assert_eq!(pinned["checks"]["expected_snapshot"]["valid"], false);
}

#[test]
fn missing_objects_ranges_and_session_claims_do_not_verify() {
    let (_temp, root) = root();
    let (_, file, bundle) = setup(&root, true);
    let mut changed = bundle.clone();
    changed["evidence"] = json!([]);
    assert_eq!(check(&changed, &file)["valid"], false);
    let mut changed = bundle.clone();
    changed["associations"] = json!([]);
    assert_eq!(check(&changed, &file)["valid"], false);
    let mut changed = bundle.clone();
    changed["snapshot"]["evidence"][0]["coverage"]["end"] = json!(1);
    changed["snapshot_digest"] = json!(canonical_digest(&changed["snapshot"]));
    assert_eq!(check(&changed, &file)["checks"]["evidence"]["valid"], false);
    let mut changed = bundle.clone();
    let bytes = "{\"session_id\":\"different-session\"}\n";
    let digest = sha256_digest(bytes.as_bytes());
    changed["evidence"][0] = json!({"digest": digest, "bytes": bytes});
    changed["snapshot"]["evidence"][0]["digest"] = json!(digest);
    changed["snapshot"]["evidence"][0]["coverage"]["end"] = json!(bytes.len());
    changed["snapshot"]["evidence"][0]["coverage"]["record_count"] = json!(1);
    changed["snapshot_digest"] = json!(canonical_digest(&changed["snapshot"]));
    assert_eq!(check(&changed, &file)["checks"]["evidence"]["valid"], false);
}

#[test]
fn declarations_without_evidence_do_not_claim_recorded_evidence() {
    let (_temp, root) = root();
    let (_, file, bundle) = setup(&root, false);
    let report = check(&bundle, &file);
    assert_eq!(report["valid"], true);
    assert_eq!(report["checks"]["evidence"]["status"], "not_recorded");
}

#[test]
fn bundle_publication_is_canonical_readable_exclusive_and_validated() {
    let (_temp, root) = root();
    let (_, file, bundle) = setup(&root, true);
    let output = root.join("export.json");
    artifact::write_bundle(&output, &bundle).unwrap();
    let original = fs::read(&output).unwrap();
    assert_eq!(original, canonical_json(&bundle).as_bytes());
    assert_eq!(
        check(&artifact::read_json(&output).unwrap(), &file)["valid"],
        true
    );
    assert!(
        artifact::write_bundle(&output, &bundle)
            .unwrap_err()
            .contains("already exists")
    );
    assert_eq!(original, fs::read(&output).unwrap());
    let mut invalid = bundle;
    invalid["associations"][0]["role"] = json!("review");
    let rejected = root.join("invalid.json");
    assert!(artifact::write_bundle(&rejected, &invalid).is_err());
    assert!(!rejected.exists());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pending-")
    }));
}

#[test]
fn oversized_encoded_bundle_never_creates_output() {
    let (_temp, root) = root();
    let (_, _, mut bundle) = setup(&root, true);
    // Quotes double in the encoded JSON representation. The encoded boundary
    // applies before publication even for programmatically constructed Values.
    bundle["evidence"][0]["bytes"] = json!("\"".repeat(artifact::MAX_JSON_BYTES / 2));
    let output = root.join("too-large.json");
    assert!(
        artifact::write_bundle(&output, &bundle)
            .unwrap_err()
            .contains("32 MiB")
    );
    assert!(!output.exists());
}

#[test]
fn strict_json_rejects_nested_duplicate_keys_unsafe_numbers_and_extra_values() {
    let (_temp, root) = root();
    let path = root.join("untrusted.json");
    for data in [
        r#"{"a":{"session_id":"one","session_id":"two"}}"#,
        r#"{"n":9007199254740992}"#,
        r#"{"n":-9007199254740992}"#,
        r#"{"n":9.007199254740992e15}"#,
        r#"{"n":18446744073709551616}"#,
        r#"{"n":1e999}"#,
        "{} {}",
    ] {
        fs::write(&path, data).unwrap();
        assert!(artifact::read_json(&path).is_err(), "accepted {data}");
    }
    fs::write(&path, r#"{"n":9007199254740991,"unicode":"é"}"#).unwrap();
    assert!(artifact::read_json(&path).is_ok());
}

#[test]
fn unsupported_versions_fields_and_observations_fail_closed() {
    let (_temp, root) = root();
    let (store, file, bundle) = setup(&root, false);
    let mut declaration = request("s");
    declaration["assertion"]["basis"] = json!("host_observation");
    assert!(artifact::associate(&store, &declaration).is_err());
    let mut changed = bundle.clone();
    changed["record_version"] = json!("commonmeasure-artifact/2");
    assert!(artifact::verify_bundle(&changed, &VerifyTarget::File(file.clone()), None).is_err());
    let mut changed = bundle;
    changed["snapshot"]["critical"] = json!(["unknown-extension"]);
    assert!(artifact::verify_bundle(&changed, &VerifyTarget::File(file), None).is_err());
}

#[test]
fn invalid_evidence_never_publishes_a_snapshot() {
    let (_temp, root) = root();
    let store = root.join("store");
    artifact::init_store(&store, None).unwrap();
    let association = artifact::associate(&store, &request("s")).unwrap();
    let file = root.join("file");
    fs::write(&file, "artifact").unwrap();
    let log = root.join("log.ndjson");
    let input = EvidenceInput {
        association_id: association["record_id"].as_str().unwrap().into(),
        path: log.clone(),
    };
    for bytes in [
        b"{\"session_id\":\"s\"}".as_slice(),
        b"{\"session_id\":\"other\"}\n",
        b"{\"data\":1}\n",
        b"{\"session_id\":\"s\",\"payload\":{\"session_id\":\"other\"}}\n",
        b"{\"session_id\":\"s\",\"n\":9007199254740992}\n",
        b"{\"session_id\":\"s\",\"session_id\":\"s\"}\n",
        b"\xff\n",
        b"\n",
    ] {
        fs::write(&log, bytes).unwrap();
        assert!(artifact::snapshot_file(&store, &file, std::slice::from_ref(&input)).is_err());
        assert_eq!(fs::read_dir(store.join("snapshots")).unwrap().count(), 0);
    }
}

#[test]
fn evidence_must_name_a_selected_association_and_fit_size_bounds() {
    let (_temp, root) = root();
    let (store, file, _) = setup(&root, false);
    let log = root.join("oversized.ndjson");
    let output = fs::File::create(&log).unwrap();
    output
        .set_len(artifact::MAX_EVIDENCE_BYTES as u64 + 1)
        .unwrap();
    let association = artifact::associate(&store, &request("s")).unwrap();
    let mut input = EvidenceInput {
        association_id: association["record_id"].as_str().unwrap().into(),
        path: log,
    };
    assert!(
        artifact::snapshot_file(&store, &file, std::slice::from_ref(&input))
            .unwrap_err()
            .contains("limit")
    );
    input.association_id = uuid::Uuid::new_v4().to_string();
    assert!(
        artifact::snapshot_file(&store, &file, &[input])
            .unwrap_err()
            .contains("outside")
    );
}

#[test]
fn concurrent_initialisation_and_append_preserve_one_identity_and_every_declaration() {
    let (_temp, root) = root();
    let store = root.join("store");
    let barrier = Arc::new(Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|i| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let identity = artifact::init_store(&store, None).unwrap();
                let association =
                    artifact::associate(&store, &request(&format!("session-{i}"))).unwrap();
                assert_eq!(identity["artifact_id"], association["artifact_id"]);
                identity["artifact_id"].clone()
            })
        })
        .collect();
    let identities: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(identities.iter().all(|id| id == &identities[0]));
    assert_eq!(
        fs::read_dir(store.join("associations")).unwrap().count(),
        12
    );
    let file = root.join("file");
    fs::write(&file, "artifact").unwrap();
    let snapshot = artifact::snapshot_file(&store, &file, &[]).unwrap();
    assert_eq!(
        snapshot["snapshot"]["associations"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
}

#[test]
fn identity_conflicts_and_snapshot_self_capture_are_refused() {
    let (_temp, root) = root();
    let (store, _, bundle) = setup(&root, false);
    let identity_before = fs::read(store.join("identity.json")).unwrap();
    assert!(
        artifact::init_store(
            &store,
            Some("urn:uuid:00000000-0000-0000-0000-000000000001")
        )
        .is_err()
    );
    assert_eq!(
        identity_before,
        fs::read(store.join("identity.json")).unwrap()
    );
    assert!(artifact::snapshot_file(&store, &store.join("identity.json"), &[]).is_err());
    assert!(artifact::export_bundle(&store, "../identity").is_err());
    assert!(artifact::init_store(&root.join("store/../other"), None).is_err());
    let snapshot_path = store.join("snapshots").join(format!(
        "{}.json",
        bundle["snapshot"]["snapshot_id"].as_str().unwrap()
    ));
    assert!(artifact::snapshot_file(&store, &snapshot_path, &[]).is_err());
}

#[cfg(unix)]
#[test]
fn symlinks_at_store_input_and_record_boundaries_are_refused() {
    use std::os::unix::fs::symlink;
    let (_temp, root) = root();
    let (store, file, bundle) = setup(&root, false);
    let link = root.join("store-link");
    symlink(&store, &link).unwrap();
    assert!(artifact::init_store(&link, None).is_err());
    let link = root.join("file-link");
    symlink(&file, &link).unwrap();
    assert!(artifact::snapshot_file(&store, &link, &[]).is_err());
    let id = bundle["associations"][0]["record_id"].as_str().unwrap();
    let path = store.join("associations").join(format!("{id}.json"));
    let outside = root.join("outside-record.json");
    fs::rename(&path, &outside).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(artifact::snapshot_file(&store, &file, &[]).is_err());
}

#[test]
fn tampered_retained_evidence_is_not_exported_and_prior_snapshots_remain() {
    let (_temp, root) = root();
    let (store, file, bundle) = setup(&root, true);
    let old_id = bundle["snapshot"]["snapshot_id"].as_str().unwrap();
    let new = artifact::snapshot_file(&store, &file, &[]).unwrap();
    assert_ne!(new["snapshot_id"], old_id);
    assert!(
        store
            .join("snapshots")
            .join(format!("{old_id}.json"))
            .exists()
    );
    let digest = bundle["evidence"][0]["digest"].as_str().unwrap();
    fs::write(
        store
            .join("evidence")
            .join(format!("{}.ndjson", &digest[7..])),
        "{\"session_id\":\"session-a\"}\n",
    )
    .unwrap();
    assert!(artifact::export_bundle(&store, old_id).is_err());
    let snapshots_before = fs::read_dir(store.join("snapshots")).unwrap().count();
    let evidence = EvidenceInput {
        association_id: bundle["associations"][0]["record_id"]
            .as_str()
            .unwrap()
            .into(),
        path: root.join("source.ndjson"),
    };
    assert!(artifact::snapshot_file(&store, &file, &[evidence]).is_err());
    assert_eq!(
        fs::read_dir(store.join("snapshots")).unwrap().count(),
        snapshots_before
    );
}
