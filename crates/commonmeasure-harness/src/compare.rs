//! Operator-started retrieval comparisons using the mediated acquisition path.
//!
//! No Hub or inference backend is required. Evidence stays outside the relay's
//! session directory; sharing needs a separately authorised projection.
use std::path::Path;

use commonmeasure_supply::credentials::CredentialsStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{SessionLog, mcp::McpServer, policy::SessionPolicy};

/// Version of the local comparison record, not an organisation upload format.
pub const SCHEMA: &str = "contextops-comparison/1";
/// Maximum results requested per selected provider.
pub const RESULT_LIMIT: u32 = 5;

/// An explicit operator selection. Order is execution order; duplicates and
/// unknown providers are refused before any supplier call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    pub query: String,
    pub providers: Vec<String>,
}

impl CompareRequest {
    /// Validate the whole selection before any evidence or external effect.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() || self.query.len() > 4096 {
            return Err("Enter a query of 1 to 4096 bytes.".into());
        }
        if self.providers.is_empty() {
            return Err("Select at least one provider; no query was sent.".into());
        }
        let mut seen = std::collections::HashSet::new();
        for provider in &self.providers {
            if !commonmeasure_supply::IMPLEMENTED_PROVIDERS.contains(&provider.as_str())
                || !seen.insert(provider)
            {
                return Err(
                    "Selection contains an unknown or repeated provider; no query was sent.".into(),
                );
            }
        }
        Ok(())
    }
}

/// Resolve the local process principal and source policy for `cwd`, then run
/// exactly the requested providers. Credentials must already have been applied
/// at process startup, before threads exist. This does not enrol or sync Hub,
/// accept a caller-supplied principal, or expand source permissions.
///
/// A write failure stops further providers. A failure after dispatch may still
/// have incurred a supplier charge; the incomplete record is retained.
pub fn run(
    home: &Path,
    cwd: Option<String>,
    credentials: CredentialsStatus,
    request: &CompareRequest,
) -> Result<Value, String> {
    request.validate()?;
    let policy = SessionPolicy::load(home, cwd.as_deref())?;
    let id = format!("compare-{}", uuid::Uuid::new_v4());
    let mut log = SessionLog::open_comparison(home, &id).map_err(|e| e.to_string())?;
    log.set_policy_identity(&policy.identity());
    log.record_comparison("comparison_started", json!({
        "schema": SCHEMA, "comparison_id": id, "query": request.query,
        "query_sha256": commonmeasure_types::canonical::sha256_digest(request.query.as_bytes()),
        "selected_providers": request.providers, "requested_limit": RESULT_LIMIT,
        "effective_limit": RESULT_LIMIT, "cwd": cwd, "home": home,
        "policy_identity": policy.identity(), "policy": policy.canonical(),
        "principal": policy.principal(), "authentication_basis": policy.authentication_basis().as_str(),
        "sharing": "local_only", "measurement": "retrieval_probe"
    })).map_err(|e| format!("comparison evidence unavailable before dispatch: {e}"))?;
    McpServer::new(log, policy, "edge-compare", cwd, credentials).compare(request)
}

/// Read a retained comparison, including an interrupted or failed attempt.
/// Identifiers are constrained to generated comparison names, never paths.
pub fn read(home: &Path, id: &str) -> Result<Value, String> {
    if !id.starts_with("compare-") || crate::safe_session(id).is_none() {
        return Err("invalid comparison identifier".into());
    }
    let path = home.join("comparisons").join(format!("{id}.ndjson"));
    let records = SessionLog::read(&path).map_err(|e| e.to_string())?;
    let mut result = records.iter().rev().find(|r| r["event"] == "comparison_finished")
        .map(|r| r["payload"].clone()).unwrap_or_else(|| {
            let start = records.iter().find(|r| r["event"] == "comparison_started").map(|r| &r["payload"]);
            json!({"schema": SCHEMA, "kind": "incomplete", "status": 200,
                "notice": "Comparison incomplete. A started supplier request may have incurred a charge; do not assume it is safe to retry.",
                "comparison_id": id, "query": start.and_then(|s| s["query"].as_str()),
                "selected_providers": start.map(|s| &s["selected_providers"]),
                "results": records.iter().filter(|r| r["event"] == "comparison_provider_finished").map(|r| r["payload"].clone()).collect::<Vec<_>>(),
                "recorded_in": path, "sharing": "local_only"})
        });
    result["evidence"] = json!(records);
    Ok(result)
}

/// Retained identifiers, newest first. Listing performs no acquisition.
pub fn list(home: &Path) -> Result<Vec<String>, String> {
    let directory = home.join("comparisons");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    paths.sort_by_key(|entry| std::cmp::Reverse(entry.metadata().and_then(|m| m.modified()).ok()));
    Ok(paths
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?;
            (path.extension()?.to_str()? == "ndjson"
                && id.starts_with("compare-")
                && crate::safe_session(id).is_some())
            .then(|| id.to_owned())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_comparison_remains_incomplete_and_never_enters_session_listing() {
        let home = tempfile::tempdir().unwrap();
        let id = "compare-interrupted";
        let mut log = SessionLog::open_comparison(home.path(), id).unwrap();
        log.record_comparison(
            "comparison_started",
            json!({"query":"test", "selected_providers":["exa", "internal"]}),
        )
        .unwrap();
        log.record_comparison("comparison_provider_started", json!({"provider":"exa"}))
            .unwrap();
        let result = read(home.path(), id).unwrap();
        assert_eq!(result["kind"], "incomplete");
        assert!(result["results"].as_array().unwrap().is_empty());
        assert_eq!(result["evidence"].as_array().unwrap().len(), 2);
        assert!(SessionLog::list(home.path()).unwrap().is_empty());
        assert_eq!(list(home.path()).unwrap(), vec![id]);
        assert!(read(home.path(), "compare-../policy").is_err());
    }
}
