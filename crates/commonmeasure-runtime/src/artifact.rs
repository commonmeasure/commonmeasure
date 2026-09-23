//! Local, unsigned artifact declarations and portable evidence snapshots.
//!
//! Digests establish consistency with supplied bytes. They do not authenticate
//! declarants or establish acquisition, insertion, authorship or completeness.
mod files;
mod git;
mod json;
mod schema;

use chrono::{SecondsFormat, Utc};
use commonmeasure_types::canonical::{canonical_digest, canonical_json, sha256_digest};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Record and bundle format supported by this local profile.
pub const VERSION: &str = "commonmeasure-artifact/1";
/// Maximum encoded JSON input or portable bundle size.
pub const MAX_JSON_BYTES: usize = 32 * 1024 * 1024;
/// Maximum exact file binding size.
pub const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;
/// Maximum bytes in one explicitly selected NDJSON source.
pub const MAX_EVIDENCE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum aggregate evidence bytes in one capture or bundle.
pub const MAX_TOTAL_EVIDENCE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum declarations or evidence references in one snapshot.
pub const MAX_ASSOCIATIONS: usize = 1024;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// An explicitly selected NDJSON file associated with one declaration.
#[derive(Clone, Debug)]
pub struct EvidenceInput {
    pub association_id: String,
    pub path: PathBuf,
}

/// The artifact bytes a portable bundle should be checked against.
#[derive(Clone, Debug)]
pub enum VerifyTarget {
    File(PathBuf),
    Git { repo: PathBuf, revision: String },
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Read bounded JSON, rejecting duplicate members and ambiguous integer values.
pub fn read_json(path: &Path) -> Result<Value, String> {
    let value = json::parse(&files::read(path, MAX_JSON_BYTES)?)?;
    json::validate(&value)?;
    Ok(value)
}

fn read_record(path: &Path) -> Result<Value, String> {
    let value = json::parse(&files::read(path, MAX_RECORD_BYTES)?)?;
    json::validate(&value)?;
    Ok(value)
}

fn record_bytes(value: &Value) -> Result<Vec<u8>, String> {
    json::validate(value)?;
    let bytes = canonical_json(value).into_bytes();
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("artifact record exceeds 1 MiB".into());
    }
    Ok(bytes)
}

fn publish_record(path: &Path, value: &Value) -> Result<(), String> {
    if !files::publish(path, &record_bytes(value)?)? {
        return Err(format!(
            "immutable record already exists: {}",
            path.display()
        ));
    }
    Ok(())
}

fn load_identity(store: &Path) -> Result<(PathBuf, Value), String> {
    let store = files::checked_path(store, false)?;
    let identity = read_record(&store.join("identity.json"))?;
    schema::identity(&identity)?;
    Ok((store, identity))
}

/// Initialise an immutable artifact identity. Concurrent writers reuse the
/// fully published winner; an explicitly requested different ID is a conflict.
pub fn init_store(store: &Path, artifact_id: Option<&str>) -> Result<Value, String> {
    if let Some(id) = artifact_id {
        schema::artifact_id(id)?;
    }
    let store = files::checked_path(store, true)?;
    for subdir in ["associations", "snapshots", "evidence"] {
        files::checked_path(&store.join(subdir), true)?;
    }
    let identity = json!({
        "record_version": VERSION,
        "kind": "identity",
        "artifact_id": artifact_id.map(str::to_owned).unwrap_or_else(|| format!("urn:uuid:{}", Uuid::new_v4())),
        "created_at": now(),
    });
    let path = store.join("identity.json");
    files::publish(&path, &record_bytes(&identity)?)?;
    let retained = read_record(&path)?;
    schema::identity(&retained)?;
    if artifact_id.is_some_and(|id| retained["artifact_id"] != id) {
        return Err("store already has a different artifact_id".into());
    }
    Ok(retained)
}

/// Append an explicit agent or operator declaration to an initialised store.
/// Host observations and inferred session mappings are unsupported.
pub fn associate(store: &Path, request: &Value) -> Result<Value, String> {
    json::validate(request)?;
    schema::request(request)?;
    let (store, identity) = load_identity(store)?;
    let mut assertion = request["assertion"].clone();
    assertion["relation"] = json!("session-associated-with-artifact");
    let record = json!({
        "record_version": VERSION,
        "kind": "association",
        "record_id": Uuid::new_v4().to_string(),
        "created_at": now(),
        "artifact_id": identity["artifact_id"],
        "session": request["session"],
        "role": request["role"],
        "assertion": assertion,
        "applies_to": {"kind": "working_artifact"},
    });
    schema::association(&record)?;
    let id = schema::text(&record, "record_id")?;
    publish_record(
        &store.join("associations").join(format!("{id}.json")),
        &record,
    )?;
    Ok(record)
}

fn records(store: &Path, kind: &str) -> Result<Vec<Value>, String> {
    let path = files::checked_path(&store.join(kind), false)?;
    let mut paths = Vec::new();
    for (count, entry) in fs::read_dir(path).map_err(|e| e.to_string())?.enumerate() {
        if count >= MAX_ASSOCIATIONS * 2 {
            return Err(
                "record directory exceeds its entry limit, including pending writes".into(),
            );
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let filename = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 store filename")?;
        if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            return Err("symlink inside artifact store".into());
        }
        // Unpublished temporaries can remain after a terminated writer.
        if filename.starts_with(".pending-") {
            continue;
        }
        let id = filename
            .strip_suffix(".json")
            .ok_or("unexpected file inside record directory")?;
        schema::uuid(id)?;
        paths.push((id.to_owned(), entry.path()));
        if paths.len() > MAX_ASSOCIATIONS {
            return Err(format!("{kind} exceeds {MAX_ASSOCIATIONS} records"));
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    paths
        .into_iter()
        .map(|(id, path)| {
            let value = read_record(&path)?;
            let key = if kind == "associations" {
                schema::association(&value)?;
                "record_id"
            } else {
                schema::snapshot(&value)?;
                "snapshot_id"
            };
            if value[key] != id {
                return Err("record filename disagrees with record identifier".into());
            }
            Ok(value)
        })
        .collect()
}

fn file_content(file: &Path) -> Result<Value, String> {
    let bytes = files::read(file, MAX_FILE_BYTES)?;
    Ok(
        json!({"scope": "file-bytes/1", "algorithm": "sha256", "digest": sha256_digest(&bytes), "byte_length": bytes.len()}),
    )
}

/// Capture saved file bytes, all current declarations and explicitly selected
/// evidence. A file within its own artifact store cannot be captured.
pub fn snapshot_file(
    store: &Path,
    file: &Path,
    evidence: &[EvidenceInput],
) -> Result<Value, String> {
    let (store, _) = load_identity(store)?;
    let file = files::checked_path(file, false)?;
    if file.starts_with(&store) {
        return Err("cannot capture the artifact store as artifact content".into());
    }
    snapshot(&store, file_content(&file)?, evidence)
}

/// Capture one fixed Git tree. The store must be external to the repository or
/// beneath its excluded `.commonmeasure/` directory.
pub fn snapshot_repo(
    store: &Path,
    repo: &Path,
    revision: &str,
    evidence: &[EvidenceInput],
) -> Result<Value, String> {
    let (store, _) = load_identity(store)?;
    let repo = files::checked_path(repo, false)?;
    if store.starts_with(&repo) && !store.starts_with(repo.join(".commonmeasure")) {
        return Err(
            "repository artifact store must be outside the repo or below .commonmeasure/".into(),
        );
    }
    snapshot(&store, git::capture(&repo, revision)?, evidence)
}

fn ndjson(bytes: &[u8], session: &str) -> Result<usize, String> {
    if bytes.is_empty() || bytes.len() > MAX_EVIDENCE_BYTES || !bytes.ends_with(b"\n") {
        return Err("evidence must contain a nonempty bounded prefix ending in a newline".into());
    }
    std::str::from_utf8(bytes).map_err(|_| "evidence must be UTF-8")?;
    let mut count = 0;
    for line in bytes[..bytes.len() - 1].split(|b| *b == b'\n') {
        count += 1;
        if line.is_empty() || line.len() > MAX_LINE_BYTES || count > 100_000 {
            return Err("evidence exceeds line/count bounds or contains an empty line".into());
        }
        let record = json::parse(line)?;
        json::validate(&record)?;
        if !record.is_object() {
            return Err("evidence record must be an object".into());
        }
        let direct = record.get("session_id");
        let payload = record.get("payload").and_then(|v| v.get("session_id"));
        if direct.is_none() && payload.is_none() {
            return Err("every evidence record must identify its session_id".into());
        }
        if direct
            .into_iter()
            .chain(payload)
            .any(|id| id.as_str() != Some(session))
        {
            return Err("evidence session_id does not match its association".into());
        }
    }
    Ok(count)
}

fn evidence_name(digest: &str) -> Result<String, String> {
    schema::digest(digest)?;
    Ok(format!("{}.ndjson", &digest[7..]))
}

fn snapshot(store: &Path, content: Value, inputs: &[EvidenceInput]) -> Result<Value, String> {
    let (store, identity) = load_identity(store)?;
    schema::content(&content)?;
    let associations = records(&store, "associations")?;
    if associations
        .iter()
        .any(|a| a["artifact_id"] != identity["artifact_id"])
    {
        return Err("association names a different artifact".into());
    }
    if inputs.len() > MAX_ASSOCIATIONS {
        return Err("too many evidence selections".into());
    }
    let mut evidence = Vec::new();
    let mut retained = BTreeMap::new();
    let mut selected = BTreeSet::new();
    let mut total = 0usize;
    for input in inputs {
        schema::uuid(&input.association_id)?;
        if !selected.insert(&input.association_id) {
            return Err("select at most one evidence prefix per association".into());
        }
        let association = associations
            .iter()
            .find(|a| a["record_id"] == input.association_id)
            .ok_or("evidence refers to an association outside the snapshot")?;
        let session = schema::text(&association["session"], "id")?;
        let bytes = files::read(&input.path, MAX_EVIDENCE_BYTES)?;
        total += bytes.len();
        if total > MAX_TOTAL_EVIDENCE_BYTES {
            return Err("aggregate evidence exceeds 16 MiB".into());
        }
        let count = ndjson(&bytes, session)?;
        let digest = sha256_digest(&bytes);
        evidence.push(json!({
            "association_id": input.association_id,
            "digest": digest,
            "scope": "evidence-bytes/1",
            "format": "application/x-ndjson",
            "coverage": {"kind": "full_prefix", "log_id": session, "start": 0, "end": bytes.len(), "record_count": count, "complete_lines": true, "captured_at": now()},
        }));
        retained.insert(digest, bytes);
    }
    evidence.sort_by(|a, b| {
        (a["digest"].as_str(), a["association_id"].as_str())
            .cmp(&(b["digest"].as_str(), b["association_id"].as_str()))
    });
    let snapshot = json!({
        "record_version": VERSION, "kind": "snapshot", "snapshot_id": Uuid::new_v4().to_string(),
        "created_at": now(), "artifact_id": identity["artifact_id"], "identity_digest": canonical_digest(&identity),
        "parents": [], "content": content,
        "associations": associations.iter().map(|a| json!({"record_id": a["record_id"], "digest": canonical_digest(a)})).collect::<Vec<_>>(),
        "evidence": evidence,
    });
    schema::snapshot(&snapshot)?;
    // Validate the whole record before writing anything, then retain every
    // evidence object before making the referencing snapshot visible.
    record_bytes(&snapshot)?;
    for (digest, bytes) in retained {
        let path = store.join("evidence").join(evidence_name(&digest)?);
        if !files::publish(&path, &bytes)? && files::read(&path, MAX_EVIDENCE_BYTES)? != bytes {
            return Err("existing evidence object conflicts with its digest".into());
        }
    }
    let id = schema::text(&snapshot, "snapshot_id")?;
    publish_record(
        &store.join("snapshots").join(format!("{id}.json")),
        &snapshot,
    )?;
    Ok(
        json!({"snapshot_id": id, "snapshot_digest": canonical_digest(&snapshot), "snapshot": snapshot}),
    )
}

/// Export a self-contained unsigned bundle. No original Edge or store path is
/// required for later verification; original source logs are never discovered.
pub fn export_bundle(store: &Path, snapshot_id: &str) -> Result<Value, String> {
    schema::uuid(snapshot_id)?;
    let (store, identity) = load_identity(store)?;
    let snapshot = read_record(&store.join("snapshots").join(format!("{snapshot_id}.json")))?;
    schema::snapshot(&snapshot)?;
    if snapshot["snapshot_id"] != snapshot_id {
        return Err("snapshot filename disagrees with snapshot_id".into());
    }
    let mut associations = Vec::new();
    for reference in schema::array(&snapshot, "associations", MAX_ASSOCIATIONS)? {
        let id = schema::text(reference, "record_id")?;
        let association = read_record(&store.join("associations").join(format!("{id}.json")))?;
        schema::association(&association)?;
        associations.push(association);
    }
    let mut evidence = BTreeMap::new();
    let mut total = 0;
    for reference in schema::array(&snapshot, "evidence", MAX_ASSOCIATIONS)? {
        let digest = schema::text(reference, "digest")?;
        if evidence.contains_key(digest) {
            continue;
        }
        let bytes = files::read(
            &store.join("evidence").join(evidence_name(digest)?),
            MAX_EVIDENCE_BYTES,
        )?;
        total += bytes.len();
        if total > MAX_TOTAL_EVIDENCE_BYTES {
            return Err("aggregate evidence exceeds 16 MiB".into());
        }
        let bytes = String::from_utf8(bytes).map_err(|_| "evidence must be UTF-8")?;
        evidence.insert(digest.to_owned(), json!({"digest": digest, "bytes": bytes}));
    }
    let bundle = json!({
        "record_version": VERSION, "kind": "bundle", "identity": identity,
        "associations": associations, "snapshot_digest": canonical_digest(&snapshot), "snapshot": snapshot,
        "evidence": evidence.into_values().collect::<Vec<_>>(),
    });
    let integrity = bundle_integrity(&bundle, None)?;
    if !integrity.valid {
        return Err(format!(
            "retained bundle integrity failed: {}",
            integrity.checks
        ));
    }
    if canonical_json(&bundle).len() > MAX_JSON_BYTES {
        return Err("bundle exceeds 32 MiB JSON limit".into());
    }
    Ok(bundle)
}

/// Publish a validated portable bundle as bounded canonical JSON. The final
/// name appears only after all bytes are synced and must not already exist.
pub fn write_bundle(path: &Path, bundle: &Value) -> Result<(), String> {
    let integrity = bundle_integrity(bundle, None)?;
    if !integrity.valid {
        return Err(format!("bundle integrity failed: {}", integrity.checks));
    }
    let filename = path.file_name().ok_or("bundle output must name a file")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = files::checked_path(parent, false)?;
    let path = parent.join(filename);
    let bytes = canonical_json(bundle).into_bytes();
    if bytes.len() > MAX_JSON_BYTES {
        return Err("bundle exceeds 32 MiB JSON limit".into());
    }
    if !files::publish(&path, &bytes)? {
        return Err(format!("bundle output already exists: {}", path.display()));
    }
    Ok(())
}

struct Integrity {
    valid: bool,
    checks: Value,
    digest: String,
}

fn bundle_integrity(bundle: &Value, expected: Option<&str>) -> Result<Integrity, String> {
    json::validate(bundle)?;
    if canonical_json(bundle).len() > MAX_JSON_BYTES {
        return Err("bundle exceeds 32 MiB JSON limit".into());
    }
    schema::fields(
        bundle,
        &[
            "record_version",
            "kind",
            "identity",
            "associations",
            "snapshot",
            "snapshot_digest",
            "evidence",
        ],
        &[],
    )?;
    schema::equal(bundle, "record_version", VERSION)?;
    schema::equal(bundle, "kind", "bundle")?;
    schema::identity(&bundle["identity"])?;
    let snapshot = &bundle["snapshot"];
    schema::snapshot(snapshot)?;
    schema::digest(schema::text(bundle, "snapshot_digest")?)?;
    if let Some(digest) = expected {
        schema::digest(digest)?;
    }
    let digest = canonical_digest(snapshot);
    let snapshot_ok = bundle["snapshot_digest"] == digest;
    let pin_ok = expected.is_none_or(|expected| expected == digest);
    let identity_ok = canonical_digest(&bundle["identity"]) == snapshot["identity_digest"]
        && bundle["identity"]["artifact_id"] == snapshot["artifact_id"];
    let mut associations = BTreeMap::new();
    let mut previous = "";
    for a in schema::array(bundle, "associations", MAX_ASSOCIATIONS)? {
        schema::association(a)?;
        let id = schema::text(a, "record_id")?;
        if id <= previous {
            return Err("bundle associations must be unique and sorted by record_id".into());
        }
        previous = id;
        associations.insert(id, a);
    }
    let references = schema::array(snapshot, "associations", MAX_ASSOCIATIONS)?;
    let mut association_ok = references.len() == associations.len();
    for reference in references {
        let id = schema::text(reference, "record_id")?;
        association_ok &= associations.get(id).is_some_and(|a| {
            a["artifact_id"] == snapshot["artifact_id"]
                && canonical_digest(a) == reference["digest"]
        });
    }
    let mut bytes_by_digest = BTreeMap::new();
    let mut total = 0;
    let mut previous = "";
    for e in schema::array(bundle, "evidence", MAX_ASSOCIATIONS)? {
        schema::fields(e, &["digest", "bytes"], &[])?;
        let digest = schema::text(e, "digest")?;
        schema::digest(digest)?;
        if digest <= previous {
            return Err("bundle evidence must be unique and sorted by digest".into());
        }
        previous = digest;
        let bytes = e["bytes"]
            .as_str()
            .ok_or("evidence bytes must be a UTF-8 string")?
            .as_bytes();
        total += bytes.len();
        if bytes.len() > MAX_EVIDENCE_BYTES || total > MAX_TOTAL_EVIDENCE_BYTES {
            return Err("bundle evidence exceeds byte limits".into());
        }
        bytes_by_digest.insert(digest, bytes);
    }
    let evidence_refs = schema::array(snapshot, "evidence", MAX_ASSOCIATIONS)?;
    let mut evidence_ok = true;
    let mut used = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for reference in evidence_refs {
        let digest = schema::text(reference, "digest")?;
        let association_id = schema::text(reference, "association_id")?;
        if !selected.insert(association_id) {
            return Err("multiple evidence prefixes for one association are unsupported".into());
        }
        used.insert(digest);
        let Some(association) = associations.get(association_id) else {
            evidence_ok = false;
            continue;
        };
        let Some(bytes) = bytes_by_digest.get(digest) else {
            evidence_ok = false;
            continue;
        };
        let coverage = &reference["coverage"];
        let session = schema::text(&association["session"], "id")?;
        evidence_ok &= sha256_digest(bytes) == digest
            && coverage["log_id"] == session
            && coverage["end"].as_u64() == Some(bytes.len() as u64)
            && ndjson(bytes, session)
                .is_ok_and(|count| coverage["record_count"].as_u64() == Some(count as u64));
    }
    evidence_ok &= used.len() == bytes_by_digest.len()
        && used
            .iter()
            .all(|digest| bytes_by_digest.contains_key(digest));
    let inventory_ok = snapshot["content"]["scope"] != "repo-inventory/1"
        || canonical_digest(&snapshot["content"]["inventory"]) == snapshot["content"]["digest"];
    let valid =
        snapshot_ok && pin_ok && identity_ok && association_ok && evidence_ok && inventory_ok;
    Ok(Integrity {
        valid,
        digest,
        checks: json!({
            "snapshot_digest": {"valid": snapshot_ok},
            "expected_snapshot": {"status": if expected.is_some() { "checked" } else { "not_supplied" }, "valid": pin_ok},
            "identity": {"valid": identity_ok}, "associations": {"valid": association_ok, "count": references.len()},
            "evidence": {"valid": evidence_ok, "status": if evidence_refs.is_empty() && bytes_by_digest.is_empty() { "not_recorded" } else if evidence_ok { "matched" } else { "mismatch" }},
            "inventory_digest": {"valid": inventory_ok},
        }),
    })
}

/// Verify bytes and declarations offline. Mismatches return `valid: false`;
/// malformed or unsupported profiles return errors. No signer is authenticated.
pub fn verify_bundle(
    bundle: &Value,
    target: &VerifyTarget,
    expected_snapshot: Option<&str>,
) -> Result<Value, String> {
    let mut integrity = bundle_integrity(bundle, expected_snapshot)?;
    let content = &bundle["snapshot"]["content"];
    let actual = match target {
        VerifyTarget::File(path) => file_content(path),
        VerifyTarget::Git { repo, revision } => {
            files::checked_path(repo, false).and_then(|repo| git::capture(&repo, revision))
        }
    };
    let content_check = match actual {
        Ok(actual) => {
            let valid = actual["scope"] == content["scope"]
                && actual["digest"] == content["digest"]
                && if content["scope"] == "file-bytes/1" {
                    actual["byte_length"] == content["byte_length"]
                } else {
                    actual["inventory"] == content["inventory"]
                };
            json!({"valid": valid, "status": if valid { "matched" } else { "mismatch" }, "actual": actual})
        }
        Err(error) => json!({"valid": false, "status": "unavailable", "error": error}),
    };
    integrity.valid &= content_check["valid"] == true;
    integrity.checks["content"] = content_check;
    Ok(json!({
        "record_version": VERSION, "kind": "verification", "verified_at": now(),
        "artifact_id": bundle["snapshot"]["artifact_id"], "snapshot_id": bundle["snapshot"]["snapshot_id"],
        "snapshot_digest": integrity.digest, "valid": integrity.valid, "checks": integrity.checks,
        "signature": "absent", "trust": "not_evaluated",
        "claim": "Byte and declaration consistency only; declarations are unauthenticated and do not establish source use or authorship.",
    }))
}

fn markdown_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('\\', "&#92;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "&#96;")
        .replace('|', "&#124;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace('*', "&#42;")
        .replace('_', "&#95;")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Generate a human-readable index of immutable records. It reports declarations
/// and paths; it never claims that a listed snapshot has just been verified.
pub fn markdown_index(store: &Path) -> Result<String, String> {
    let display_store = if store.is_absolute() {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        store.strip_prefix(cwd).unwrap_or(store).to_path_buf()
    } else {
        store.to_path_buf()
    };
    let (store, identity) = load_identity(store)?;
    let associations = records(&store, "associations")?;
    let snapshots = records(&store, "snapshots")?;
    if associations
        .iter()
        .chain(snapshots.iter())
        .any(|r| r["artifact_id"] != identity["artifact_id"])
    {
        return Err("record names a different artifact".into());
    }
    let mut output = format!(
        "# Common Measure artifact associations\n\nArtifact: `{}`\n\nStore: `{}`\n\nThese are unsigned declarations. They do not establish source use, insertion or authorship.\n\n",
        schema::text(&identity, "artifact_id")?,
        markdown_text(&display_store.display().to_string())
    );
    output.push_str("## Session declarations\n\n| Record | Session namespace / ID | Role | Basis / actor |\n|---|---|---|---|\n");
    for a in &associations {
        output.push_str(&format!(
            "| `{}` | {} / {} | {} | {} / {} |\n",
            schema::text(a, "record_id")?,
            markdown_text(schema::text(&a["session"], "namespace")?),
            markdown_text(schema::text(&a["session"], "id")?),
            schema::text(a, "role")?,
            schema::text(&a["assertion"], "basis")?,
            markdown_text(schema::text(&a["assertion"], "actor")?)
        ));
    }
    output.push_str("\nDeclaration files: `associations/<record UUID>.json` within the store.\n\n## Captured snapshots\n\n| Snapshot | Scope | Snapshot digest |\n|---|---|---|\n");
    for snapshot in snapshots {
        output.push_str(&format!(
            "| `{}` | {} | `{}` |\n",
            schema::text(&snapshot, "snapshot_id")?,
            schema::text(&snapshot["content"], "scope")?,
            canonical_digest(&snapshot)
        ));
    }
    output.push_str("\nSnapshot files: `snapshots/<snapshot UUID>.json` within the store. Listing a snapshot does not verify the current artifact.\n");
    Ok(output)
}
