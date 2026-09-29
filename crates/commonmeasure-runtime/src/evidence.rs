//! The append-only evidence trail and how a run is published.
//!
//! Two properties matter and both are about failure. A record is durable
//! before it is acknowledged, so "recorded" means on disk rather than in a
//! buffer. And when a write fails, the window is materialised as an explicit
//! gap record by the next successful write, so a reader can always tell
//! "nothing happened" from "this log could not say what happened".
//!
//! A record cut short by a process that died mid-append is the one damage
//! the log repairs: the next append terminates it and records the gap, and
//! readers skip it ([`scan`]). Anything else that does not parse stays a
//! hard error for every reader. There is no outbound spool here: a batch
//! run has nothing to spool.

mod scan;

pub use scan::{LogPosition, Scanned, TORN_LINE_BYTES, Tear, TornTail, Validated, scan};

use std::fs::{self, File};
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
    /// Two hooks can fire close together. Each append holds the log's
    /// exclusive append lock for its one fsynced `write_all`
    /// ([`Self::append`]), so appends never interleave. Sequence numbers can
    /// collide under concurrency where positions cannot, because a process
    /// counts the lines once, here; a reader orders by position and treats
    /// `seq` as a hint.
    ///
    /// A path that exists but is not a regular file (a directory, a FIFO) is
    /// refused before it is opened, and the first read error ends the count:
    /// opening a directory succeeds on some platforms, and a reader that
    /// skipped its errors would retry the same failed read without end.
    pub fn open_append(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let sequence = match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => count_lines(File::open(path)?)?,
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{} is not a regular file", path.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
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
    /// The append holds the log's exclusive advisory lock from reading its
    /// tail to the end of the fsync. A last line without its newline, found
    /// under the lock, was left by a process that has stopped writing it,
    /// so it is terminated and an `evidence_gap` record naming its length is
    /// written before this record ([`scan`] says how readers use it). A
    /// line another process is still writing is never touched: that process
    /// holds the lock until its write returns, and the kernel drops the lock
    /// of a process that exits.
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
        self.append_locked(event, payload).inspect_err(|error| {
            self.note_write_failure(&format!("append of {event} failed: {error}"));
        })
    }

    /// Append one record under the append lock, marking a torn tail first.
    fn append_locked(&mut self, event: &str, payload: Value) -> std::io::Result<u64> {
        let created = !self.path.exists();
        let mut file = scan::open_for_append(&self.path, true)?;
        scan::lock_within(&file, scan::Lock::Exclusive, &self.path)?;
        let mut sequence = self.sequence;
        let mut bytes = Vec::new();
        if let Some(torn) = scan::torn_tail(&mut file)? {
            sequence += 1;
            bytes.push(b'\n');
            push_line(&mut bytes, &torn_gap_record(sequence, torn, &file))?;
        }
        sequence += 1;
        push_line(
            &mut bytes,
            &json!({
                "seq": sequence,
                "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "event": event,
                "payload": payload,
            }),
        )?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // `sync_all` makes the bytes durable, but a file whose directory
        // entry is lost to a crash was never recorded in any sense a reader
        // can use, so the append that creates the file syncs that too.
        if created {
            sync_parent(&self.path)?;
        }
        self.sequence = sequence;
        Ok(sequence)
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
        let payload = json!({
            "reason": GapReason::WriteFailed,
            "from": gap.from.to_rfc3339_opts(SecondsFormat::Millis, true),
            "to": gap.to.to_rfc3339_opts(SecondsFormat::Millis, true),
            "detail": gap.detail,
        });
        // If the gap record itself cannot be written the gap stays pending.
        // Nothing is lost and nothing is silently dropped.
        if let Err(error) = self.append_locked("evidence_gap", payload) {
            self.pending_gap = Some(gap);
            return Err(error);
        }
        Ok(())
    }

    /// Every record of the log at `path`, in order.
    ///
    /// A torn line marked by the `evidence_gap` record after it is skipped;
    /// any other line that does not parse, and a final line without its
    /// newline that no process is writing, fail the read naming the line
    /// ([`scan`]). A final line another process is still writing is waited
    /// for. Nothing is written.
    pub fn read(path: &Path) -> std::io::Result<Vec<Value>> {
        Self::read_with(path, TornTail::Refuse).map(|(records, _)| records)
    }

    /// Every record of the log at `path` and the torn lines it skipped, the
    /// final line handled as `tail` says.
    pub fn read_with(path: &Path, tail: TornTail) -> std::io::Result<(Vec<Value>, Vec<Tear>)> {
        let mut records = Vec::new();
        let scanned = scan(path, LogPosition::default(), tail, |record, _| {
            records.push(record)
        })?;
        match scanned.fault {
            Some(fault) => Err(fault),
            None => Ok((records, scanned.tears)),
        }
    }

    /// Check that the log at `path` reads from `from` on as [`Self::read_with`]
    /// would read it, holding one line at a time: a check repeated as the log
    /// grows passes the returned [`Scanned::end`] back as `from`.
    pub fn validate(path: &Path, from: LogPosition, tail: TornTail) -> std::io::Result<Scanned> {
        scan::<Validated>(path, from, tail, |_, _| {})
    }
}

/// Terminate the final line of the log at `path` and write the
/// `evidence_gap` record that marks it, if it ends without its newline and
/// no process is writing it, as the next append would. Returns the tear
/// marked, if one was. A log that does not exist has nothing to repair.
///
/// The relay repairs before it reads a session for delivery, because a
/// session whose last writer died mid-append may never be appended to again.
pub fn repair_torn_tail(path: &Path) -> std::io::Result<Option<Tear>> {
    let mut file = match scan::open_for_append(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    scan::lock_within(&file, scan::Lock::Exclusive, path)?;
    let Some(torn) = scan::torn_tail(&mut file)? else {
        return Ok(None);
    };
    // The torn line is the last counted.
    let lines = count_lines(File::open(path)?)?;
    let mut bytes = vec![b'\n'];
    push_line(&mut bytes, &torn_gap_record(lines + 1, torn, &file))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(Some(Tear {
        line: lines,
        bytes: torn,
        marked: true,
    }))
}

/// The `evidence_gap` record that marks the line before it, `bytes` long,
/// as torn. The window runs from the file's last modification, which was
/// the torn write, since nothing has been appended after it, to now.
fn torn_gap_record(sequence: u64, bytes: u64, file: &File) -> Value {
    let now = Utc::now();
    let from = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or(now);
    json!({
        "seq": sequence,
        "timestamp": now.to_rfc3339_opts(SecondsFormat::Millis, true),
        "event": "evidence_gap",
        "payload": {
            "reason": GapReason::WriteFailed,
            "from": from.to_rfc3339_opts(SecondsFormat::Millis, true),
            "to": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            TORN_LINE_BYTES: bytes,
            "detail": format!(
                "the line before this record ended without its newline after {bytes} bytes and \
                 no process was still writing it, so a write was cut short; the line is \
                 terminated here, and readers skip it unless it parses"
            ),
        },
    })
}

/// `record` as one NDJSON line, appended to `bytes`.
fn push_line(bytes: &mut Vec<u8>, record: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *bytes, record).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(())
}

/// Count lines as `BufRead::lines` would, a final line without its newline
/// included, but on bytes, so a record that is not UTF-8 still counts, and
/// stopping at the first read error.
fn count_lines(source: impl std::io::Read) -> std::io::Result<u64> {
    let mut reader = BufReader::new(source);
    let mut line = Vec::new();
    let mut count = 0;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(count);
        }
        count += 1;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that yields one line and then fails every read, as a
    /// directory opened for reading does on macOS.
    struct FailsAfterOneLine {
        served: bool,
        reads: u32,
    }

    impl std::io::Read for FailsAfterOneLine {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            assert!(self.reads < 100, "the count kept reading after an error");
            if !self.served {
                self.served = true;
                buffer[..2].copy_from_slice(b"{\n");
                return Ok(2);
            }
            Err(std::io::Error::other("injected read error"))
        }
    }

    #[test]
    fn a_read_error_ends_the_count_with_that_error() {
        let error = count_lines(FailsAfterOneLine {
            served: false,
            reads: 0,
        })
        .expect_err("the injected error");
        assert_eq!(error.to_string(), "injected read error");
    }

    #[test]
    fn lines_are_counted_as_bufread_lines_counts_them() {
        for text in [&b""[..], b"a\n", b"a\nb", b"a\n\xff\n", b"\n\n"] {
            let expected = BufReader::new(text).lines().count() as u64;
            assert_eq!(count_lines(text).unwrap(), expected, "{text:?}");
        }
    }

    #[test]
    fn a_log_path_that_is_not_a_regular_file_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join("credentials-changes.ndjson");
        fs::create_dir(&directory).unwrap();
        let error = EvidenceLog::open_append(&directory)
            .err()
            .expect("a directory is refused");
        assert!(error.to_string().contains("not a regular file"), "{error}");
    }
}
