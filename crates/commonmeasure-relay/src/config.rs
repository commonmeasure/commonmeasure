//! Relay configuration: who receives the projection, if anyone.
//!
//! No configuration means no egress. That is not a fallback but the shipped
//! state: there is no default receiver, no ambient environment variable, and
//! no code path that sends without a receiver having been named explicitly in
//! `<home>/relay.json` or on the command line. A malformed file is an error,
//! never a silent absence, following the policy file's discipline: egress
//! nobody chose must not start, and egress somebody chose must not silently
//! stop.

use commonmeasure_harness::identity::write_private;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Unknown fields are load errors, like the policy file's: a misspelled
/// `"api_kye"` must fail the load, not deliver unauthenticated.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    /// Base URL of the Content Telemetry receiver, e.g. `http://localhost:8080`.
    pub receiver: String,
    /// API key presented as `X-API-Key`. Optional because the standard does
    /// not prescribe an auth scheme; a conforming receiver may require one.
    #[serde(default)]
    pub api_key: Option<String>,
}

impl RelayConfig {
    /// Load `<home>/relay.json`. Absent means no receiver is configured.
    pub fn load(home: &Path) -> Result<Option<Self>, String> {
        let source = home.join("relay.json");
        if !source.exists() {
            return Ok(None);
        }
        let encoded = std::fs::read(&source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        let config: RelayConfig = serde_json::from_slice(&encoded).map_err(|error| {
            format!("{} is not a valid relay config: {error}", source.display())
        })?;
        Ok(Some(config))
    }

    /// Write `<home>/relay.json` atomically, readable by the owner only:
    /// it carries the ingest key. Written by enrolment only; the relay
    /// itself never writes its own configuration.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        let path = home.join("relay.json");
        let tmp = path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise relay config: {error}"))?;
        write_private(&tmp, &encoded)?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(())
    }
}
