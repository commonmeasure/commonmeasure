//! Durable relay state: what has actually been delivered, and to whom.
//!
//! Two artefacts under `<home>/relay/`, both inspectable with a text editor:
//!
//! - `delivered.idx` — one event id per line, appended after the receiver
//!   accepts the batch carrying it. The set of ids, not a counter, so a
//!   redelivered batch can never double-count and the delivered figure is
//!   reproducible from the file alone.
//! - `receipts.json` — the last delivery outcome, written atomically. This is
//!   what the console's egress block reports, so it must never say more than
//!   the index can prove.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::RelayConfig;
use crate::spool::Spool;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Receipts {
    /// The receiver the last delivery went to.
    #[serde(default)]
    pub receiver: Option<String>,
    #[serde(default)]
    pub last_delivered_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub struct RelayState {
    delivered_path: PathBuf,
    receipts_path: PathBuf,
}

impl RelayState {
    pub fn open(home: &Path) -> Result<Self> {
        let dir = home.join("relay");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self {
            delivered_path: dir.join("delivered.idx"),
            receipts_path: dir.join("receipts.json"),
        })
    }

    /// Every event id a receiver has acknowledged, ever.
    pub fn delivered(&self) -> Result<HashSet<Uuid>> {
        if !self.delivered_path.exists() {
            return Ok(HashSet::new());
        }
        let text = std::fs::read_to_string(&self.delivered_path).context("read delivered.idx")?;
        Ok(text
            .lines()
            .filter_map(|line| Uuid::parse_str(line.trim()).ok())
            .collect())
    }

    /// Record acknowledged event ids durably, before the spool acknowledgement
    /// that depends on them: a crash between the two redelivers, and the set
    /// absorbs the duplicates.
    pub fn record_delivered(&self, ids: &[Uuid]) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.delivered_path)
            .context("open delivered.idx")?;
        for id in ids {
            writeln!(file, "{id}")?;
        }
        file.sync_all().context("fsync delivered.idx")?;
        Ok(())
    }

    pub fn receipts(&self) -> Receipts {
        std::fs::read(&self.receipts_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Replace the receipts atomically (write-then-rename).
    pub fn write_receipts(&self, receipts: &Receipts) -> Result<()> {
        let tmp = self.receipts_path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(receipts).context("serialise receipts")?,
        )
        .context("write receipts tmp")?;
        std::fs::rename(&tmp, &self.receipts_path).context("rename receipts into place")?;
        Ok(())
    }

    pub fn record_success(&self, receiver: &str) -> Result<()> {
        self.write_receipts(&Receipts {
            receiver: Some(receiver.to_owned()),
            last_delivered_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
            last_error: None,
        })
    }

    pub fn record_failure(&self, receiver: &str, error: &str) -> Result<()> {
        let mut receipts = self.receipts();
        receipts.receiver = Some(receiver.to_owned());
        receipts.last_error = Some(error.to_owned());
        self.write_receipts(&receipts)
    }
}

/// The egress block for the console's `/api/status`: the configured receiver
/// and delivered counts, truthfully, from the durable state alone. This is a
/// report, not a health check: it must answer even when the configuration is
/// broken, and what it says then is that the configuration is broken.
pub fn egress_report(home: &Path) -> Value {
    let (configured, config_error) = match RelayConfig::load(home) {
        Ok(config) => (config, None),
        Err(error) => (None, Some(error)),
    };
    let state = RelayState::open(home).ok();
    let delivered = state
        .as_ref()
        .and_then(|state| state.delivered().ok())
        .map(|ids| ids.len() as u64)
        .unwrap_or(0);
    let receipts = state.map(|state| state.receipts()).unwrap_or_default();
    let pending: u64 = Spool::open(home)
        .and_then(|spool| spool.pending())
        .map(|entries| {
            entries
                .iter()
                .map(|(_, entry)| {
                    entry.document["events"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or(0) as u64
                })
                .sum()
        })
        .unwrap_or(0);

    let receiver = configured.as_ref().map(|config| config.receiver.clone());
    let detail = if let Some(error) = config_error {
        format!("{error}; nothing is sent until it parses.")
    } else {
        let pristine =
            receiver.is_none() && delivered == 0 && pending == 0 && receipts.last_error.is_none();
        let mut detail = match (&receiver, pristine) {
            (None, true) => {
                "No telemetry receiver is configured; nothing leaves this machine.".to_owned()
            }
            // No standing configuration, but the durable account shows relay
            // activity: a receiver was named explicitly on an invocation.
            (None, false) => {
                let mut detail =
                    "No telemetry receiver is configured; nothing is sent without one.".to_owned();
                if delivered > 0 {
                    detail.push_str(&format!(
                        " {delivered} events were delivered to {} when a receiver was named \
                         explicitly.",
                        receipts
                            .receiver
                            .as_deref()
                            .unwrap_or("an earlier receiver")
                    ));
                }
                detail
            }
            (Some(_), _) => match &receipts.last_delivered_at {
                Some(at) => format!("Last delivery at {at}."),
                None => "Nothing has been delivered yet.".to_owned(),
            },
        };
        if pending > 0 {
            detail.push_str(&format!(" {pending} events await delivery in the spool."));
        }
        if let Some(error) = &receipts.last_error {
            detail.push_str(&format!(" Last attempt failed: {error}"));
        }
        detail
    };

    // The enrolled key, so the panel names the identity this edge presents
    // beside where its evidence goes. Null for an edge that is not enrolled.
    let enrolment = commonmeasure_harness::EnrolmentRecord::load(home)
        .ok()
        .flatten();

    json!({
        "receiver": receiver,
        "delivered": delivered,
        "pending": pending,
        "detail": detail,
        "key_id": enrolment.as_ref().map(|record| record.key_id.clone()),
        "key_standing": enrolment.as_ref().map(|record| record.standing()),
    })
}
