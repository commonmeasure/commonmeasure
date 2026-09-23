//! Real local processes exercise portable artifact records and explicit scope.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use commonmeasure_runtime::EvidenceLog;
use serde_json::{Value, json};

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn root(temp: &tempfile::TempDir) -> PathBuf {
    temp.path()
        .canonicalize()
        .expect("canonical temporary path")
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command.arg("artifact");
    command
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON command result")
}

fn assert_record_schema(value: &Value) {
    let schema: Value = serde_json::from_str(include_str!("../../../schema/artifact.v1.json"))
        .expect("artifact schema JSON");
    let schema = jsonschema::JSONSchema::compile(&schema).expect("artifact schema");
    if let Err(errors) = schema.validate(value) {
        panic!(
            "record disagrees with published schema: {}",
            errors
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}

fn initialise(store: &Path) -> Value {
    success(
        command()
            .arg("init")
            .arg("--store")
            .arg(store)
            .output()
            .expect("init"),
    )
}

fn association_command(store: &Path, session: &str) -> Command {
    let mut command = command();
    command.arg("associate").arg("--store").arg(store).args([
        "--session",
        session,
        "--namespace",
        "urn:example:sessions",
        "--edge",
        "synthetic-edge",
        "--issuer",
        "urn:example:operator",
        "--actor",
        "urn:example:agent",
        "--basis",
        "agent_declaration",
        "--role",
        "drafting",
    ]);
    command
}

fn associate(store: &Path, session: &str) -> Value {
    success(
        association_command(store, session)
            .output()
            .expect("associate"),
    )
}

fn file_snapshot(store: &Path, file: &Path) -> Value {
    success(
        command()
            .args(["snapshot", "file"])
            .arg(file)
            .arg("--store")
            .arg(store)
            .output()
            .expect("snapshot file"),
    )
}

fn export(store: &Path, snapshot: &Value, output: &Path) -> Value {
    success(
        command()
            .arg("export")
            .arg("--store")
            .arg(store)
            .arg("--snapshot")
            .arg(snapshot["snapshot_id"].as_str().expect("snapshot id"))
            .arg("--output")
            .arg(output)
            .output()
            .expect("export"),
    )
}

fn verify_file(bundle: &Path, file: &Path) -> Output {
    command()
        .arg("verify")
        .arg(bundle)
        .arg("--file")
        .arg(file)
        .output()
        .expect("verify file")
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(repo: &Path, path: &str) {
    git(repo, &["add", "--", path]);
    git(
        repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Synthetic artifact fixture",
        ],
    );
}

fn git_snapshot(store: &Path, repo: &Path) -> Value {
    success(
        command()
            .args(["snapshot", "git"])
            .arg(repo)
            .arg("--store")
            .arg(store)
            .output()
            .expect("snapshot git"),
    )
}

fn verify_git(bundle: &Path, repo: &Path) -> Output {
    command()
        .arg("verify")
        .arg(bundle)
        .arg("--repo")
        .arg(repo)
        .output()
        .expect("verify git")
}

#[test]
fn portable_file_bundle_survives_session_and_store_deletion_and_detects_edits() {
    let temp = scratch();
    let root = root(&temp);
    let store = root.join("records");
    let identity = initialise(&store);
    assert_record_schema(&identity);
    assert_eq!(identity["artifact_id"], initialise(&store)["artifact_id"]);
    let first = associate(&store, "session-one");
    assert_record_schema(&first);
    let second = associate(&store, "session-two");
    assert_ne!(first["record_id"], second["record_id"]);
    let log_path = root.join("session.ndjson");
    let mut log = EvidenceLog::create(&log_path).expect("real log");
    log.append(
        "crossing_mediated",
        json!({"session_id":"session-one", "grounded":false,
        "url":"https://source.example/synthetic", "content_hash":"sha256:synthetic"}),
    )
    .expect("append");
    drop(log);
    let source_log = fs::read(&log_path).expect("source log bytes");
    let file = root.join("draft.txt");
    fs::write(&file, "Synthetic cafe\u{301} answer.\n").expect("answer");
    let snapshot = success(
        command()
            .args(["snapshot", "file"])
            .arg(&file)
            .arg("--store")
            .arg(&store)
            .arg("--evidence")
            .arg(format!(
                "{}={}",
                first["record_id"].as_str().expect("record id"),
                log_path.display()
            ))
            .output()
            .expect("snapshot"),
    );
    let bundle = root.join("draft.txt.commonmeasure.json");
    export(&store, &snapshot, &bundle);
    let exported: Value =
        serde_json::from_slice(&fs::read(&bundle).expect("bundle")).expect("bundle JSON");
    assert_record_schema(&exported["snapshot"]);
    assert_record_schema(&exported);
    assert_eq!(
        exported["associations"]
            .as_array()
            .expect("associations")
            .len(),
        2
    );
    assert_eq!(
        exported["evidence"][0]["bytes"]
            .as_str()
            .expect("evidence")
            .as_bytes(),
        source_log
    );
    fs::remove_dir_all(&store).expect("remove original store");
    fs::remove_file(&log_path).expect("remove original session log");
    let copied_file = root.join("renamed.txt");
    fs::rename(&file, &copied_file).expect("move file");
    let report = success(verify_file(&bundle, &copied_file));
    assert_eq!(report["valid"], true);
    assert_eq!(report["signature"], "absent");
    assert_eq!(report["trust"], "not_evaluated");
    let pinned = success(
        command()
            .arg("verify")
            .arg(&bundle)
            .arg("--file")
            .arg(&copied_file)
            .arg("--expected-snapshot")
            .arg(snapshot["snapshot_digest"].as_str().expect("digest"))
            .output()
            .expect("pinned verification"),
    );
    assert_eq!(pinned["valid"], true);
    fs::write(&copied_file, "Changed synthetic answer.\n").expect("alter content");
    assert!(!verify_file(&bundle, &copied_file).status.success());
    fs::write(&copied_file, "Synthetic cafe\u{301} answer.\n").expect("restore content");
    let mut altered = exported.clone();
    altered["associations"][0]["role"] = json!("review");
    fs::write(&bundle, serde_json::to_vec(&altered).expect("JSON")).expect("alter association");
    assert!(!verify_file(&bundle, &copied_file).status.success());
    altered = exported.clone();
    altered["evidence"][0]["bytes"] = json!("{}\n");
    fs::write(&bundle, serde_json::to_vec(&altered).expect("JSON")).expect("alter evidence");
    assert!(!verify_file(&bundle, &copied_file).status.success());
    altered = exported;
    altered["evidence"] = json!([]);
    fs::write(&bundle, serde_json::to_vec(&altered).expect("JSON")).expect("remove evidence");
    assert!(!verify_file(&bundle, &copied_file).status.success());
}

#[test]
fn git_binding_excludes_provenance_but_detects_tracked_changes() {
    let temp = scratch();
    let repo = root(&temp);
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Synthetic fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.test"]);
    fs::write(repo.join("code.txt"), "original\n").expect("code");
    commit(&repo, "code.txt");
    let store = repo.join(".commonmeasure/artifacts/project");
    initialise(&store);
    associate(&store, "coding-session");
    let first = git_snapshot(&store, &repo);
    assert_record_schema(&first["snapshot"]);
    let nested = repo.join("nested");
    fs::create_dir(&nested).expect("nested directory");
    let refused = command()
        .args(["snapshot", "git"])
        .arg(&nested)
        .arg("--store")
        .arg(&store)
        .output()
        .expect("nested repository snapshot");
    assert!(!refused.status.success());
    let bundle = repo.join(".commonmeasure/export.json");
    export(&store, &first, &bundle);
    let index = command()
        .arg("index")
        .arg("--store")
        .arg(&store)
        .output()
        .expect("index");
    assert!(index.status.success());
    let index_text = String::from_utf8(index.stdout).expect("Markdown");
    assert!(index_text.contains("coding-session"));
    fs::write(repo.join("COMMONMEASURE.md"), index_text).expect("index file");
    commit(&repo, ".");
    let second = git_snapshot(&store, &repo);
    assert_eq!(
        first["snapshot"]["content"]["digest"],
        second["snapshot"]["content"]["digest"]
    );
    assert_ne!(
        first["snapshot"]["content"]["source"]["tree"],
        second["snapshot"]["content"]["source"]["tree"]
    );
    assert_eq!(success(verify_git(&bundle, &repo))["valid"], true);
    fs::write(repo.join("code.txt"), "uncommitted change\n").expect("working tree edit");
    // The contract binds the selected tree, explicitly excluding uncommitted work.
    assert_eq!(success(verify_git(&bundle, &repo))["valid"], true);
    commit(&repo, "code.txt");
    assert!(!verify_git(&bundle, &repo).status.success());
    let changed = git_snapshot(&store, &repo);
    let changed_bundle = repo.join(".commonmeasure/changed.json");
    export(&store, &changed, &changed_bundle);
    fs::write(repo.join("added.txt"), "addition\n").expect("new code");
    commit(&repo, "added.txt");
    assert!(!verify_git(&changed_bundle, &repo).status.success());
    let added = git_snapshot(&store, &repo);
    let added_bundle = repo.join(".commonmeasure/added.json");
    export(&store, &added, &added_bundle);
    fs::remove_file(repo.join("added.txt")).expect("delete code");
    commit(&repo, "added.txt");
    assert!(!verify_git(&added_bundle, &repo).status.success());
}

#[test]
fn concurrent_declarations_retain_each_record() {
    let temp = scratch();
    let root = root(&temp);
    let store = root.join("records");
    initialise(&store);
    let mut children = Vec::new();
    for number in 0..6 {
        children.push(
            association_command(&store, &format!("parallel-{number}"))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("concurrent writer"),
        );
    }
    let mut ids = std::collections::BTreeSet::new();
    for child in children {
        let record = success(child.wait_with_output().expect("writer result"));
        ids.insert(record["record_id"].as_str().expect("record id").to_owned());
    }
    assert_eq!(ids.len(), 6);
    let file = root.join("answer.txt");
    fs::write(&file, "synthetic\n").expect("file");
    let snapshot = file_snapshot(&store, &file);
    assert_eq!(
        snapshot["snapshot"]["associations"]
            .as_array()
            .expect("records")
            .len(),
        6
    );
}

#[test]
fn ambiguous_input_and_unsupported_observation_claims_are_refused() {
    let temp = scratch();
    let root = root(&temp);
    let file = root.join("answer.txt");
    fs::write(&file, "synthetic\n").expect("answer");
    let bundle = root.join("ambiguous.json");
    fs::write(
        &bundle,
        r#"{"record_version":"commonmeasure-artifact/1","record_version":"other/1"}"#,
    )
    .expect("ambiguous JSON");
    assert!(!verify_file(&bundle, &file).status.success());
    let store = root.join("records");
    initialise(&store);
    let output = command()
        .arg("associate")
        .arg("--store")
        .arg(&store)
        .args([
            "--session",
            "synthetic",
            "--namespace",
            "urn:example:sessions",
            "--edge",
            "local",
            "--issuer",
            "urn:example:operator",
            "--actor",
            "urn:example:agent",
            "--basis",
            "host_observation",
        ])
        .output()
        .expect("unsupported observation");
    assert!(!output.status.success());
    let association = associate(&store, "expected-session");
    let wrong_log = root.join("wrong.ndjson");
    fs::write(&wrong_log, "{\"session_id\":\"another-session\"}\n").expect("wrong evidence");
    let output = command()
        .args(["snapshot", "file"])
        .arg(&file)
        .arg("--store")
        .arg(&store)
        .arg("--evidence")
        .arg(format!(
            "{}={}",
            association["record_id"].as_str().expect("id"),
            wrong_log.display()
        ))
        .output()
        .expect("wrong session capture");
    assert!(!output.status.success());
    let snapshot = file_snapshot(&store, &file);
    let output = root.join("existing.json");
    fs::write(&output, b"do not replace").expect("existing output");
    let refused = command()
        .arg("export")
        .arg("--store")
        .arg(&store)
        .arg("--snapshot")
        .arg(snapshot["snapshot_id"].as_str().expect("id"))
        .arg("--output")
        .arg(&output)
        .output()
        .expect("refused overwrite");
    assert!(!refused.status.success());
    assert_eq!(
        fs::read(&output).expect("unchanged output"),
        b"do not replace"
    );
}

#[cfg(unix)]
#[test]
fn export_refuses_symlinked_parent_and_leaves_no_output() {
    let temp = scratch();
    let root = root(&temp);
    let store = root.join("records");
    initialise(&store);
    let file = root.join("answer.txt");
    fs::write(&file, "synthetic\n").expect("answer");
    let snapshot = file_snapshot(&store, &file);
    let actual = root.join("actual");
    fs::create_dir(&actual).expect("output directory");
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&actual, &alias).expect("directory symlink");
    let refused = command()
        .arg("export")
        .arg("--store")
        .arg(&store)
        .arg("--snapshot")
        .arg(snapshot["snapshot_id"].as_str().expect("id"))
        .arg("--output")
        .arg(alias.join("bundle.json"))
        .output()
        .expect("export through symlink");
    assert!(!refused.status.success());
    assert_eq!(
        fs::read_dir(&actual).expect("unchanged directory").count(),
        0
    );
}

#[test]
fn separately_retained_digest_rejects_a_replaced_self_consistent_bundle() {
    let temp = scratch();
    let root = root(&temp);
    let store = root.join("records");
    initialise(&store);
    associate(&store, "session");
    let file = root.join("answer.txt");
    fs::write(&file, "synthetic\n").expect("answer");
    let snapshot = file_snapshot(&store, &file);
    let bundle_path = root.join("answer.commonmeasure.json");
    export(&store, &snapshot, &bundle_path);
    let mut bundle: Value =
        serde_json::from_slice(&fs::read(&bundle_path).expect("bundle")).expect("JSON bundle");
    bundle["snapshot"]["created_at"] = json!("2026-01-01T00:00:00Z");
    bundle["snapshot_digest"] = json!(commonmeasure_types::canonical::canonical_digest(
        &bundle["snapshot"]
    ));
    fs::write(&bundle_path, serde_json::to_vec(&bundle).expect("JSON")).expect("replacement");
    // An unsigned self-consistent record can be replaced; the verifier must not
    // imply authenticity. A separately held digest lets a caller detect this.
    assert_eq!(success(verify_file(&bundle_path, &file))["valid"], true);
    let output = command()
        .arg("verify")
        .arg(&bundle_path)
        .arg("--file")
        .arg(&file)
        .arg("--expected-snapshot")
        .arg(
            snapshot["snapshot_digest"]
                .as_str()
                .expect("original digest"),
        )
        .output()
        .expect("pinned verification");
    assert!(!output.status.success());
}

#[test]
fn concurrent_initialisation_uses_one_complete_identity() {
    let temp = scratch();
    let store = root(&temp).join("records");
    let mut children = Vec::new();
    for _ in 0..6 {
        children.push(
            command()
                .arg("init")
                .arg("--store")
                .arg(&store)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("concurrent init"),
        );
    }
    let mut ids = std::collections::BTreeSet::new();
    for child in children {
        let result = success(child.wait_with_output().expect("init result"));
        ids.insert(
            result["artifact_id"]
                .as_str()
                .expect("artifact id")
                .to_owned(),
        );
    }
    assert_eq!(ids.len(), 1);
}
