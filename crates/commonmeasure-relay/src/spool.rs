//! Durable outbound spool with acknowledgements.
//!
//! Every projected batch is queued here before any delivery attempt, so a
//! relay failure of any kind leaves durable, inspectable state on disk rather
//! than an in-memory loss. Delivery is at-least-once: a batch leaves the spool
//! only when the receiver's acceptance has been recorded, and a crash between
//! delivery and acknowledgement replays the batch with the same event ids,
//! which the receiver deduplicates.
//!

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// One spooled delivery: the wire document exactly as it will be posted, and
/// the evidence record it was projected from, so an operator inspecting the
/// spool can trace every queued byte to its source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolEntry {
    pub origin: String,
    pub document: Value,
}

pub struct Spool {
    queue_path: PathBuf,
    ack_path: PathBuf,
}

impl Spool {
    /// Open the spool under `<home>/relay/spool`, creating it if absent.
    pub fn open(home: &Path) -> Result<Spool> {
        let dir = home.join("relay").join("spool");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Spool {
            queue_path: dir.join("outbound.ndjson"),
            ack_path: dir.join("outbound.ack"),
        })
    }

    /// Enqueue one batch durably. Returns its spool index.
    pub fn enqueue(&self, entry: &SpoolEntry) -> Result<u64> {
        let line = serde_json::to_string(entry).context("serialise spool entry")?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.queue_path)
            .with_context(|| format!("open {}", self.queue_path.display()))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all().context("fsync spool")?;
        Ok(self.total()? - 1)
    }

    /// Batches enqueued but not yet acknowledged, with their indices.
    pub fn pending(&self) -> Result<Vec<(u64, SpoolEntry)>> {
        let acked = self.acked()?;
        if !self.queue_path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.queue_path).context("open spool")?;
        let mut out = Vec::new();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let index = index as u64;
            let line = line.context("read spool line")?;
            if index < acked || line.trim().is_empty() {
                continue;
            }
            let entry: SpoolEntry =
                serde_json::from_str(&line).with_context(|| format!("parse spool line {index}"))?;
            out.push((index, entry));
        }
        Ok(out)
    }

    /// Acknowledge every batch up to and including `index`. The ack file is
    /// replaced atomically (write-then-rename) so a crash mid-acknowledgement
    /// redelivers rather than losing.
    pub fn ack_through(&self, index: u64) -> Result<()> {
        let current = self.acked()?;
        let next = (index + 1).max(current);
        let tmp = self.ack_path.with_extension("ack.tmp");
        let mut file = File::create(&tmp).context("create ack tmp")?;
        file.write_all(next.to_string().as_bytes())?;
        file.sync_all().context("fsync ack")?;
        std::fs::rename(&tmp, &self.ack_path).context("rename ack into place")?;
        Ok(())
    }

    /// Number of acknowledged batches (also the index of the first
    /// unacknowledged one).
    pub fn acked(&self) -> Result<u64> {
        if !self.ack_path.exists() {
            return Ok(0);
        }
        let text = std::fs::read_to_string(&self.ack_path).context("read ack file")?;
        text.trim().parse().context("parse ack offset")
    }

    fn total(&self) -> Result<u64> {
        if !self.queue_path.exists() {
            return Ok(0);
        }
        let file = File::open(&self.queue_path).context("open spool")?;
        Ok(BufReader::new(file).lines().count() as u64)
    }
}
