//! The append-only evidence trail and how a run is published.
//!
//! Two properties matter and both are about failure. A record is durable
//! before it is acknowledged, so "recorded" means on disk rather than in a
//! buffer. And when a write fails, the window is materialised as an explicit
//! gap record by the next successful write, so a reader can always tell
//! "nothing happened" from "this log could not say what happened".
//!
//! There is no unclean-shutdown recovery and no outbound spool here: a batch
//! run has neither an unclean-shutdown window nor anything to spool. The
//! store carries the durability and gap discipline only.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_types::GapReason;
use serde_json::{Value, json};
use uuid::Uuid;

/// A write failure window awaiting a gap record.
#[derive(Debug, Clone)]
struct PendingGap {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    detail: String,
}

/// An append-only NDJSON evidence log.
pub struct EvidenceLog {
    path: PathBuf,
    sequence: u64,
    pending_gap: Option<PendingGap>,
}

impl EvidenceLog {
    /// Create a log at `path`. The file must not already exist: an evidence log
    /// describes exactly one run, and appending a second run's records to a
    /// first run's file would make both unreadable.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create_new(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            sequence: 0,
            pending_gap: None,
        })
    }

    /// Open an existing log for appending, creating it if absent.
    ///
    /// A harness session is not a run: it spans many processes, because each
    /// hook invocation is its own short-lived command, and each one appends to
    /// the same session's log. The sequence resumes from what is already there
    /// so numbering stays monotonic across those processes.
    ///
    /// Two hooks can fire close together, and each append is a single fsynced
    /// `write_all` to a file opened `O_APPEND`, which the kernel does not
    /// interleave for writes of this size. Sequence numbers can therefore
    /// collide under concurrency where positions cannot; a reader orders by
    /// position and treats `seq` as a hint.
    pub fn open_append(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let sequence = if path.exists() {
            BufReader::new(File::open(path)?).lines().count() as u64
        } else {
            0
        };
        Ok(Self {
            path: path.to_path_buf(),
            sequence,
            pending_gap: None,
        })
    }

    /// Append one record durably, materialising any owed gap first so the log
    /// records its own blindness in order.
    ///
    /// A failure is returned to the caller *and* remembered: the run may decide
    /// to continue under its policy mode, but the next successful append will
    /// carry the gap whether or not the caller looked at this error.
    pub fn append(&mut self, event: &str, payload: Value) -> std::io::Result<u64> {
        // The owed gap could not be written, so this event was never attempted:
        // widening the pending window is the only trace it will leave.
        if let Err(error) = self.flush_pending_gap() {
            self.note_write_failure(&format!("{event} was not attempted: {error}"));
            return Err(error);
        }
        let sequence = self.sequence + 1;
        let record = json!({
            "seq": sequence,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "event": event,
            "payload": payload,
        });
        match append_line(&self.path, &record) {
            Ok(()) => {
                self.sequence = sequence;
                Ok(sequence)
            }
            Err(error) => {
                self.note_write_failure(&format!("append of {event} failed: {error}"));
                Err(error)
            }
        }
    }

    /// Record that something the run knows happened could not be written.
    pub fn note_write_failure(&mut self, detail: &str) {
        let now = Utc::now();
        match &mut self.pending_gap {
            Some(gap) => gap.to = now,
            None => {
                self.pending_gap = Some(PendingGap {
                    from: now,
                    to: now,
                    detail: detail.to_owned(),
                })
            }
        }
    }

    pub fn has_pending_gap(&self) -> bool {
        self.pending_gap.is_some()
    }

    /// The file this log appends to, so a caller can read back exactly what
    /// landed rather than what it believes it wrote.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn flush_pending_gap(&mut self) -> std::io::Result<()> {
        let Some(gap) = self.pending_gap.take() else {
            return Ok(());
        };
        let sequence = self.sequence + 1;
        let record = json!({
            "seq": sequence,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "event": "evidence_gap",
            "payload": {
                "reason": GapReason::WriteFailed,
                "from": gap.from.to_rfc3339_opts(SecondsFormat::Millis, true),
                "to": gap.to.to_rfc3339_opts(SecondsFormat::Millis, true),
                "detail": gap.detail,
            },
        });
        // If the gap record itself cannot be written the gap stays pending.
        // Nothing is lost and nothing is silently dropped.
        if let Err(error) = append_line(&self.path, &record) {
            self.pending_gap = Some(gap);
            return Err(error);
        }
        self.sequence = sequence;
        Ok(())
    }

    pub fn read(path: &Path) -> std::io::Result<Vec<Value>> {
        let file = File::open(path)?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            records.push(serde_json::from_str(&line).map_err(std::io::Error::other)?);
        }
        Ok(records)
    }
}

/// Append one line and make it durable before returning.
///
/// When this append is the one that creates the file, the directory entry is
/// synced too: `sync_all` makes the bytes durable, but a file whose directory
/// entry is lost to a crash was never recorded in any sense a reader can use.
fn append_line(path: &Path, record: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let created = !path.exists();
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)?;
    file.sync_all()?;
    if created {
        sync_parent(path)?;
    }
    Ok(())
}

/// Make a new directory entry durable by syncing the directory that holds it.
fn sync_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) => File::open(parent)?.sync_all(),
        None => Ok(()),
    }
}

/// A run under construction, published only once it is complete.
///
/// Everything is written into a sibling staging directory and fsynced. The
/// previous run is moved aside, the staging directory takes its place, and only
/// then is the previous run removed. An interruption at any point leaves either
/// the previous complete run or a clearly named `.previous.<id>` directory
/// beside it; it never leaves a half-written run wearing the published name.
///
/// The path holds the last complete run. A second run into one output path
/// replaces the first whole, whether it started after the first finished or
/// alongside it, and the staging and set-aside names carry a per-run
/// identifier so two runs can never write into one directory and publish a
/// mixture of the two.
/// How many times a publish will set aside and take the path before giving
/// up.
///
/// Two runs colliding settle within two attempts: the one that loses the take
/// finds the path filled on its next attempt, sets it aside and takes it.
/// More than two can go on taking the path from each other, and a publish
/// that runs out of attempts fails with the error rather than spinning. One
/// writer per output path is the rule
/// (`docs/contracts/run-output.md` §Directory); this is what keeps the
/// ordinary collision of two from discarding a run that finished.
const PUBLISH_ATTEMPTS: u8 = 8;

pub struct RunDirectory {
    published: PathBuf,
    staging: PathBuf,
    /// Names this run's staging and set-aside siblings apart from any other
    /// run's.
    token: String,
}

impl RunDirectory {
    pub fn stage(published: &Path) -> std::io::Result<Self> {
        // The name is freshly generated, so nothing can be at it and there is
        // nothing to clear.
        let token = Uuid::new_v4().simple().to_string();
        let staging = sibling(published, &format!("staging.{token}"));
        fs::create_dir_all(&staging)?;
        Ok(Self {
            published: published.to_path_buf(),
            staging,
            token,
        })
    }

    /// A path inside the staging directory. Parent directories are created.
    pub fn path(&self, relative: &str) -> std::io::Result<PathBuf> {
        let path = self.staging.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    pub fn write_json(&self, relative: &str, value: &Value) -> std::io::Result<PathBuf> {
        let path = self.path(relative)?;
        let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        write_durable(&path, &bytes)?;
        Ok(path)
    }

    pub fn write_bytes(&self, relative: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.path(relative)?;
        write_durable(&path, bytes)?;
        Ok(path)
    }

    /// Move the completed run into place, replacing whatever run is there.
    ///
    /// Two runs finishing at once contend for one rename. Each sets aside
    /// what is at the path under its own name and then takes the path, and a
    /// run that finds the path taken again between those two steps tries
    /// again, so neither of two colliding runs is discarded because the other
    /// finished first; the later of them is the one the path holds. More than
    /// two can exhaust [`PUBLISH_ATTEMPTS`], and a run that does fails with
    /// the error and publishes nothing — the path still holds a whole run.
    pub fn publish(self) -> std::io::Result<PathBuf> {
        if let Some(parent) = self.published.parent() {
            fs::create_dir_all(parent)?;
        }
        let previous = sibling(&self.published, &format!("previous.{}", self.token));
        let mut attempts = PUBLISH_ATTEMPTS;
        loop {
            // `rename` reports the absence itself, so there is no window
            // between asking whether the path is taken and taking it.
            let had_previous = match fs::rename(&self.published, &previous) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error),
            };
            match fs::rename(&self.staging, &self.published) {
                Ok(()) => {
                    // The rename is what publishes; sync the directory that
                    // holds it so a crash cannot roll the publication back.
                    sync_parent(&self.published)?;
                    let _ = fs::remove_dir_all(&previous);
                    return Ok(self.published);
                }
                Err(error) => {
                    // Put back what was there rather than leaving nothing
                    // where a complete run used to be.
                    if had_previous {
                        let _ = fs::rename(&previous, &self.published);
                    }
                    // A path that filled between the two renames is another
                    // run publishing here, and trying again takes it. Any
                    // other error is about this filesystem and retrying it
                    // would only repeat it.
                    let contended = matches!(
                        error.kind(),
                        std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists
                    );
                    attempts -= 1;
                    if !contended || attempts == 0 {
                        return Err(error);
                    }
                }
            }
        }
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "run".to_owned());
    path.with_file_name(format!("{name}.{suffix}"))
}

fn write_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_parent(path)
}
