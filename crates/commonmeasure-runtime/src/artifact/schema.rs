//! The deliberately small v1 record profile. Unknown fields are unsupported.
use super::{MAX_ASSOCIATIONS, MAX_EVIDENCE_BYTES, VERSION};
use chrono::DateTime;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(super) fn fields(value: &Value, required: &[&str], optional: &[&str]) -> Result<(), String> {
    let object = value.as_object().ok_or("expected a JSON object")?;
    for key in required {
        if !object.contains_key(*key) {
            return Err(format!("missing required field: {key}"));
        }
    }
    for key in object.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("unsupported field: {key}"));
        }
    }
    Ok(())
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    let s = value[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))?;
    if s.is_empty() || s.len() > 4096 || s.chars().any(char::is_control) {
        return Err(format!(
            "{key} must be nonempty, at most 4096 bytes, without control characters"
        ));
    }
    Ok(s)
}

pub(super) fn equal(value: &Value, key: &str, expected: &str) -> Result<(), String> {
    if text(value, key)? != expected {
        return Err(format!("unsupported {key}; expected {expected}"));
    }
    Ok(())
}

pub(super) fn uuid(value: &str) -> Result<(), String> {
    if !Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value) {
        return Err("record identifier must be a canonical lowercase UUID".into());
    }
    Ok(())
}

pub(super) fn artifact_id(value: &str) -> Result<(), String> {
    uuid(
        value
            .strip_prefix("urn:uuid:")
            .ok_or("artifact_id must be a UUID URN")?,
    )
}

pub(super) fn digest(value: &str) -> Result<(), String> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or("digest must use sha256")?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("digest must have 64 lowercase hexadecimal digits".into());
    }
    Ok(())
}

pub(super) fn timestamp(value: &Value, key: &str) -> Result<(), String> {
    let value = text(value, key)?;
    if !value.ends_with('Z') || DateTime::parse_from_rfc3339(value).is_err() {
        return Err(format!("{key} must be a UTC RFC3339 time ending in Z"));
    }
    Ok(())
}

pub(super) fn base(value: &Value, kind: &str) -> Result<(), String> {
    equal(value, "record_version", VERSION)?;
    equal(value, "kind", kind)?;
    artifact_id(text(value, "artifact_id")?)?;
    timestamp(value, "created_at")
}

pub(super) fn identity(value: &Value) -> Result<(), String> {
    fields(
        value,
        &["record_version", "kind", "artifact_id", "created_at"],
        &[],
    )?;
    base(value, "identity")
}

pub(super) fn request(value: &Value) -> Result<(), String> {
    fields(value, &["session", "role", "assertion"], &[])?;
    session(&value["session"])?;
    role(value)?;
    fields(&value["assertion"], &["basis", "actor"], &[])?;
    assertion(&value["assertion"])
}

fn session(value: &Value) -> Result<(), String> {
    fields(value, &["namespace", "id", "edge"], &["host"])?;
    text(value, "namespace")?;
    text(value, "id")?;
    if value.get("host").is_some() {
        text(value, "host")?;
    }
    fields(&value["edge"], &["issuer", "installation_id"], &[])?;
    text(&value["edge"], "issuer")?;
    text(&value["edge"], "installation_id")?;
    Ok(())
}

fn role(value: &Value) -> Result<(), String> {
    if ![
        "research",
        "drafting",
        "review",
        "transformation",
        "unspecified",
    ]
    .contains(&text(value, "role")?)
    {
        return Err("unsupported association role".into());
    }
    Ok(())
}

fn assertion(value: &Value) -> Result<(), String> {
    if !["agent_declaration", "operator_declaration"].contains(&text(value, "basis")?) {
        return Err("only agent_declaration and operator_declaration are supported".into());
    }
    text(value, "actor")?;
    Ok(())
}

pub(super) fn association(value: &Value) -> Result<(), String> {
    fields(
        value,
        &[
            "record_version",
            "kind",
            "record_id",
            "created_at",
            "artifact_id",
            "session",
            "role",
            "assertion",
            "applies_to",
        ],
        &[],
    )?;
    base(value, "association")?;
    uuid(text(value, "record_id")?)?;
    session(&value["session"])?;
    role(value)?;
    fields(&value["assertion"], &["basis", "actor", "relation"], &[])?;
    assertion(&value["assertion"])?;
    equal(
        &value["assertion"],
        "relation",
        "session-associated-with-artifact",
    )?;
    fields(&value["applies_to"], &["kind"], &[])?;
    equal(&value["applies_to"], "kind", "working_artifact")
}

pub(super) fn array<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a [Value], String> {
    let values = value[key]
        .as_array()
        .ok_or_else(|| format!("{key} must be an array"))?;
    if values.len() > max {
        return Err(format!("{key} exceeds its {max}-item limit"));
    }
    Ok(values)
}

pub(super) fn uint(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("{key} must be an unsigned integer"))
}

pub(super) fn snapshot(value: &Value) -> Result<(), String> {
    fields(
        value,
        &[
            "record_version",
            "kind",
            "snapshot_id",
            "created_at",
            "artifact_id",
            "identity_digest",
            "parents",
            "content",
            "associations",
            "evidence",
        ],
        &[],
    )?;
    base(value, "snapshot")?;
    uuid(text(value, "snapshot_id")?)?;
    digest(text(value, "identity_digest")?)?;
    if !array(value, "parents", 0)?.is_empty() {
        return Err("parent snapshots are unsupported in this profile".into());
    }
    content(&value["content"])?;
    let mut previous = "";
    for a in array(value, "associations", MAX_ASSOCIATIONS)? {
        fields(a, &["record_id", "digest"], &[])?;
        let id = text(a, "record_id")?;
        uuid(id)?;
        digest(text(a, "digest")?)?;
        if id <= previous {
            return Err("association references must be unique and sorted by record_id".into());
        }
        previous = id;
    }
    let mut previous = ("", "");
    for e in array(value, "evidence", MAX_ASSOCIATIONS)? {
        fields(
            e,
            &["association_id", "digest", "scope", "format", "coverage"],
            &[],
        )?;
        uuid(text(e, "association_id")?)?;
        digest(text(e, "digest")?)?;
        let key = (text(e, "digest")?, text(e, "association_id")?);
        if key <= previous {
            return Err(
                "evidence references must be unique and sorted by digest, association_id".into(),
            );
        }
        previous = key;
        equal(e, "scope", "evidence-bytes/1")?;
        equal(e, "format", "application/x-ndjson")?;
        let c = &e["coverage"];
        fields(
            c,
            &[
                "kind",
                "log_id",
                "start",
                "end",
                "record_count",
                "complete_lines",
                "captured_at",
            ],
            &[],
        )?;
        equal(c, "kind", "full_prefix")?;
        text(c, "log_id")?;
        timestamp(c, "captured_at")?;
        if uint(c, "start")? != 0
            || uint(c, "end")? == 0
            || uint(c, "end")? > MAX_EVIDENCE_BYTES as u64
        {
            return Err("unsupported or oversized evidence byte range".into());
        }
        if uint(c, "record_count")? == 0
            || uint(c, "record_count")? > 100_000
            || c["complete_lines"] != true
        {
            return Err("evidence must contain 1..100000 complete lines".into());
        }
    }
    Ok(())
}

pub(super) fn content(value: &Value) -> Result<(), String> {
    equal(value, "algorithm", "sha256")?;
    digest(text(value, "digest")?)?;
    match text(value, "scope")? {
        "file-bytes/1" => {
            fields(value, &["scope", "algorithm", "digest", "byte_length"], &[])?;
            if uint(value, "byte_length")? > super::MAX_FILE_BYTES as u64 {
                return Err("file binding exceeds size limit".into());
            }
        }
        "repo-inventory/1" => {
            fields(
                value,
                &["scope", "algorithm", "digest", "source", "inventory"],
                &[],
            )?;
            fields(&value["source"], &["object_format", "tree"], &[])?;
            let width = match text(&value["source"], "object_format")? {
                "sha1" => 40,
                "sha256" => 64,
                _ => return Err("unsupported Git object format".into()),
            };
            let tree = text(&value["source"], "tree")?;
            if tree.len() != width
                || !tree
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("invalid Git tree locator".into());
            }
            let inventory = &value["inventory"];
            fields(
                inventory,
                &["scope", "excluded_prefixes", "excluded_paths", "entries"],
                &[],
            )?;
            equal(inventory, "scope", "repo-inventory/1")?;
            if inventory["excluded_prefixes"] != json!([".commonmeasure/"])
                || inventory["excluded_paths"] != json!(["COMMONMEASURE.md"])
            {
                return Err("unsupported repository exclusions".into());
            }
            let mut previous = "";
            let mut folded = BTreeSet::new();
            let mut total = 0u64;
            for entry in array(inventory, "entries", 10_000)? {
                fields(entry, &["path", "mode", "byte_length", "digest"], &[])?;
                let path = text(entry, "path")?;
                super::git::validate_path(path)?;
                if path.starts_with(".commonmeasure/") || path == "COMMONMEASURE.md" {
                    return Err("excluded inventory path".into());
                }
                if path <= previous || !folded.insert(super::git::folded_path(path)) {
                    return Err("inventory paths must be sorted, unique and case-distinct".into());
                }
                previous = path;
                if !["100644", "100755"].contains(&text(entry, "mode")?) {
                    return Err("unsupported Git mode".into());
                }
                let length = uint(entry, "byte_length")?;
                if length > 64 * 1024 * 1024 {
                    return Err("Git blob exceeds 64 MiB".into());
                }
                total += length;
                if total > 256 * 1024 * 1024 {
                    return Err("Git inventory exceeds 256 MiB".into());
                }
                digest(text(entry, "digest")?)?;
            }
        }
        _ => return Err("unsupported content scope".into()),
    }
    Ok(())
}
