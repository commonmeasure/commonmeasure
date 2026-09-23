//! Durable outbound spool with acknowledgements.
//!
//! Every projected batch is queued here before any delivery attempt, so a
//! relay failure of any kind leaves durable, inspectable state on disk rather
//! than an in-memory loss. Delivery is at-least-once: a batch leaves the spool
//! only when the receiver's acceptance has been recorded, and a crash between
//! delivery and acknowledgement replays the batch with the same event ids,
//! which the receiver deduplicates.
//!
//! Files under `<home>/relay/spool`:
//!
//! - `outbound.ndjson`: one retained batch per line, each with its index.
//! - `outbound.delivery.json`: every retained batch's delivery state as of
//!   the last compaction.
//! - `outbound.delivery.journal`: state changes since then, one per line,
//!   each made durable before the operation returns. The writer folds the
//!   journal into the snapshot when it closes, so a state change costs one
//!   append whatever the number of retained batches.
//! - `outbound.close-error`: present only while the last close failed to
//!   fold the journal; it holds the reason for status to show.
//! - `outbound.pruned`: present once any batch has been pruned. Beside a
//!   missing journal it shows that the pruned count is lost.
//!
//! Compaction also prunes delivered batches under the retention rule
//! (`RETAIN_DELIVERED_DAYS`, `RETAIN_DELIVERED_BATCHES`). Queued and dead
//! batches are never pruned, and an index is never given to a second batch
//! while the journal exists.
//! `docs/contracts/session-evidence.md` §Delivery state is the contract.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// One spooled delivery: the projected wire document and its source evidence.
/// A consent recheck may narrow the sent events; retained bytes preserve the
/// original projection so an operator can trace it to its source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolEntry {
    pub origin: String,
    /// Unknown for batches written before delivery cadence was introduced.
    #[serde(default)]
    pub queued_at: Option<DateTime<Utc>>,
    pub document: Value,
    /// Local consent provenance, never sent to the receiver. Legacy batches
    /// without this field retain their original behaviour in legacy homes.
    #[serde(default)]
    pub directory_selection: bool,
}

/// Ten attempts allow transient outages to recover while bounding automatic work.
pub const MAX_ATTEMPTS: u32 = 10;
/// One minute keeps the first retry beyond the HTTP client's 30-second budget.
pub const RETRY_BASE_SECONDS: i64 = 60;
/// Sustained outages receive at most one attempt per hour for each batch.
pub const RETRY_CEILING_SECONDS: i64 = 3600;

/// Durable delivery standing. Queued and dead batches are undelivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Queued,
    Delivered,
    Dead,
}

/// Private delivery metadata; never included in Content Telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryState {
    pub status: DeliveryStatus,
    /// Attempts in the current schedule; explicit requeue starts a new schedule.
    pub attempts: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Latest consent recheck; separate from the last delivery failure.
    #[serde(default)]
    pub hold_reason: Option<String>,
    /// Whether the accepted attempt contained fewer events than the retained
    /// batch. Unknown for deliveries recorded before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_subset: Option<bool>,
    /// How many `instance` members the accepted attempt withheld because the
    /// receiver was not their issuer. When above zero, the retained payload
    /// is the only spooled copy that still carries them, so the count rule
    /// does not release the batch. Absent on deliveries recorded before
    /// delivery withheld the member, when none could have been withheld.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_references_withheld: Option<u64>,
}

impl DeliveryState {
    fn queued() -> Self {
        Self {
            status: DeliveryStatus::Queued,
            attempts: 0,
            next_attempt_at: None,
            last_attempt_at: None,
            last_error: None,
            hold_reason: None,
            delivered_subset: None,
            instance_references_withheld: None,
        }
    }
}

/// A delivered batch is released this long after its accepted attempt. It is
/// not a minimum: the count rule below releases a younger batch on an edge
/// that delivers more than `RETAIN_DELIVERED_BATCHES` in the period. The
/// source record, not the spool, is the lasting evidence; the retained
/// payload serves inspection of recent deliveries.
pub const RETAIN_DELIVERED_DAYS: i64 = 30;
/// Delivered batches beyond this many are released whatever their age, lowest
/// spool index first, which also bounds deliveries whose time is unknown.
/// Index order is enqueue order, not delivery order: an old batch requeued
/// and delivered today is released before a newer batch delivered earlier.
/// A batch whose `instance` members were withheld at delivery is exempt and
/// waits for the period (`DeliveryState::instance_references_withheld`).
pub const RETAIN_DELIVERED_BATCHES: usize = 1000;
/// Pruning rewrites the queue file, so it waits until the released batches
/// are at least one in this many of those retained.
const PRUNE_FRACTION: usize = 8;

/// A writable spool holds an exclusive process lock until dropped. It loads
/// the delivery metadata once, appends each state change to a journal, and
/// folds the journal into the snapshot when it closes. Read-only views never
/// create files and read the snapshot and journal as one generation.
pub struct Spool {
    queue_path: PathBuf,
    ack_path: PathBuf,
    delivery_path: PathBuf,
    journal_path: PathBuf,
    close_error_path: PathBuf,
    pruned_marker_path: PathBuf,
    relay_state: crate::state::RelayState,
    lock: Option<File>,
    writer: Mutex<Option<Writer>>,
    /// Fault injection: runs once between a load's reads of the snapshot and
    /// of the journal, where a compaction by another process can complete.
    #[cfg(test)]
    between_reads: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// One consistent read of the spool: every state, the batches still owed to
/// the receiver, and the delivered batches already pruned.
pub struct SpoolSnapshot {
    pub states: BTreeMap<u64, DeliveryState>,
    /// Queued and dead batches with their payloads, in index order.
    pub undelivered: Vec<(u64, SpoolEntry)>,
    /// Delivered batches removed under the retention rule since the journal
    /// was introduced. They count as delivered and have no retained state.
    /// The journal alone holds the count: `None` when the journal is missing
    /// and the spool shows that batches were pruned (`outbound.pruned`, or
    /// a gap in the queue's indices), so the count is unknown and stays
    /// unknown.
    pub pruned_delivered: Option<u64>,
    /// Why the last writer's close did not fold the journal, if it did not.
    /// Delivery state is intact; the next writer's close retries. At most
    /// `CLOSE_ERROR_LIMIT` bytes of the file, without control characters.
    pub close_error: Option<String>,
    /// Why `outbound.close-error` did not read. Whether the last close
    /// failed is then unknown, so `close_error` is `None` and no failed fold
    /// is claimed.
    pub close_error_unreadable: Option<String>,
}

/// A retained batch's position in `outbound.ndjson`.
#[derive(Debug, Clone, Copy)]
struct Line {
    index: u64,
    start: u64,
    len: usize,
}

/// Delivery metadata as the snapshot and journal together state it. Batches
/// absent from `explicit` have never been claimed, held or imported.
struct View {
    lines: Vec<Line>,
    explicit: BTreeMap<u64, DeliveryState>,
    /// Absent until the first journalled change in this spool directory.
    generation: Option<u64>,
    next_index: u64,
    /// `None` when unknown; see `SpoolSnapshot::pruned_delivered`.
    pruned_delivered: Option<u64>,
    journal_records: usize,
}

struct Loaded {
    view: View,
    queue: Vec<u8>,
    /// Length of the queue file up to its last complete batch.
    queue_len: u64,
    queue_lacks_newline: bool,
    journal_len: u64,
    /// A prune the journal announced and the snapshot has not yet absorbed.
    /// The view already excludes these batches.
    interrupted_prune: Vec<u64>,
}

struct Writer {
    view: View,
    journal: Option<File>,
    /// Length of the journal up to its last durable record. A failed append
    /// is cut back to it.
    journal_len: u64,
    /// Set by a failed journal append. The bytes after `journal_len` may not
    /// have been removed, so this writer appends nothing further and its
    /// close folds nothing; the next writer repairs the tail.
    failed: bool,
    queue_len: u64,
    #[cfg(test)]
    crash_at: Option<CompactionStep>,
    /// Fault injection: the next journal append writes this many bytes and
    /// then fails, as a full disk can.
    #[cfg(test)]
    fail_append_after: Option<usize>,
    #[cfg(test)]
    abandoned: bool,
}

/// Points between the durable steps of a compaction, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum CompactionStep {
    PruneAnnounced,
    QueueRewritten,
    SnapshotReplaced,
}

/// `outbound.delivery.journal`, one record per line. A state record carries
/// the batch's whole state, so replaying a journal over a snapshot that has
/// already absorbed it changes nothing.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalRecord {
    /// First line. `generation` changes whenever the journal is replaced, so
    /// a reader can tell that a compaction completed while it was reading.
    Header {
        version: u32,
        generation: u64,
        next_index: u64,
        /// Null when unknown: the count was lost with an earlier journal.
        pruned_delivered: Option<u64>,
    },
    State {
        index: u64,
        state: DeliveryState,
    },
    /// Written before a compaction removes batches. From this record on, the
    /// named batches are pruned whatever the queue and snapshot still hold.
    Prune {
        indices: Vec<u64>,
        next_index: u64,
    },
}

struct Journal {
    generation: Option<u64>,
    next_index: u64,
    pruned_delivered: Option<u64>,
    states: Vec<(u64, DeliveryState)>,
    prune: Vec<u64>,
    len: u64,
}

const JOURNAL_VERSION: u32 = 1;
/// A read-only view retries when a compaction completes underneath it.
const LOAD_ATTEMPTS: usize = 8;

impl Spool {
    /// Open the spool under `<home>/relay/spool`, creating it if absent. An
    /// interrupted enqueue, journal append or compaction is repaired here.
    pub fn open(home: &Path) -> Result<Spool> {
        let dir = home.join("relay").join("spool");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("delivery.lock"))?;
        lock.try_lock()
            .context("another relay or requeue owns the spool")?;
        let mut spool = Self::read_only(home);
        spool.lock = Some(lock);
        let loaded = spool.load()?;
        spool.repair_tails(&loaded)?;
        let mut writer = Writer {
            view: loaded.view,
            journal: None,
            journal_len: loaded.journal_len,
            failed: false,
            queue_len: loaded.queue_len + u64::from(loaded.queue_lacks_newline),
            #[cfg(test)]
            crash_at: None,
            #[cfg(test)]
            fail_append_after: None,
            #[cfg(test)]
            abandoned: false,
        };
        if !loaded.interrupted_prune.is_empty() {
            spool.fold(&mut writer, &loaded.queue, true)?;
        }
        spool.writer = Mutex::new(Some(writer));
        Ok(spool)
    }

    /// Locate the spool without creating any directories or files.
    pub fn read_only(home: &Path) -> Self {
        let dir = home.join("relay").join("spool");
        Self {
            queue_path: dir.join("outbound.ndjson"),
            ack_path: dir.join("outbound.ack"),
            delivery_path: dir.join("outbound.delivery.json"),
            journal_path: dir.join("outbound.delivery.journal"),
            close_error_path: dir.join("outbound.close-error"),
            pruned_marker_path: dir.join("outbound.pruned"),
            relay_state: crate::state::RelayState::read_only(home),
            lock: None,
            writer: Mutex::new(None),
            #[cfg(test)]
            between_reads: Mutex::new(None),
        }
    }

    /// Enqueue one batch durably. Returns its spool index. A batch with no
    /// delivery metadata is queued and unclaimed, so none is written here.
    pub fn enqueue(&self, entry: &SpoolEntry) -> Result<u64> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        let index = writer.view.next_index;
        let line = stored_line(index, entry)?;
        let created = !self.queue_path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.queue_path)
            .with_context(|| format!("open {}", self.queue_path.display()))?;
        file.write_all(line.as_bytes())?;
        file.sync_all().context("fsync spool")?;
        if created {
            self.sync_directory()?;
        }
        writer.view.lines.push(Line {
            index,
            start: writer.queue_len,
            len: line.len() - 1,
        });
        writer.queue_len += line.len() as u64;
        writer.view.next_index = index + 1;
        Ok(index)
    }

    /// Batches enqueued but not yet acknowledged, with their indices.
    pub fn pending(&self) -> Result<Vec<(u64, SpoolEntry)>> {
        Ok(self.snapshot()?.undelivered)
    }

    /// Retained batches, including acknowledged ones, preserve the first
    /// identity sent for a session before separate identity pins existed.
    pub fn entries(&self) -> Result<Vec<(u64, SpoolEntry)>> {
        self.read(|view, queue| {
            view.lines
                .iter()
                .map(|line| Ok((line.index, parse_entry(line, queue)?)))
                .collect()
        })
    }

    /// Per-batch standing, importing the legacy acknowledgement prefix without
    /// rewriting payloads. Missing metadata means an unclaimed batch.
    pub fn delivery_states(&self) -> Result<BTreeMap<u64, DeliveryState>> {
        self.read(|view, queue| self.full_states(view, queue))
    }

    /// States, undelivered payloads and the pruned count from one read. A
    /// read-only caller that combined separate calls could see a compaction
    /// complete between them.
    pub fn snapshot(&self) -> Result<SpoolSnapshot> {
        self.read(|view, queue| {
            let states = self.full_states(view, queue)?;
            let undelivered = view
                .lines
                .iter()
                .filter(|line| states[&line.index].status != DeliveryStatus::Delivered)
                .map(|line| Ok((line.index, parse_entry(line, queue)?)))
                .collect::<Result<_>>()?;
            let (close_error, close_error_unreadable) = match self.close_error() {
                Ok(reason) => (reason, None),
                Err(unreadable) => (None, Some(unreadable)),
            };
            Ok(SpoolSnapshot {
                states,
                undelivered,
                pruned_delivered: view.pruned_delivered,
                close_error,
                close_error_unreadable,
            })
        })
    }

    /// Claim before HTTP: persist both the attempt and its retry deadline.
    /// A final claim whose outcome was lost becomes dead at that deadline.
    pub fn claim(&self, index: u64, now: DateTime<Utc>) -> Result<bool> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        let mut state = self.state_of(&writer.view, index)?;
        if state.status != DeliveryStatus::Queued
            || state.next_attempt_at.is_some_and(|at| at > now)
        {
            return Ok(false);
        }
        if state.attempts >= MAX_ATTEMPTS {
            state.status = DeliveryStatus::Dead;
            state.next_attempt_at = None;
            self.record(writer, vec![(index, state)])?;
            return Ok(false);
        }
        state.hold_reason = None;
        state.attempts += 1;
        state.last_attempt_at = Some(now);
        state.next_attempt_at = Some(
            now + Duration::seconds(
                (RETRY_BASE_SECONDS * (1_i64 << (state.attempts - 1))).min(RETRY_CEILING_SECONDS),
            ),
        );
        state.last_error =
            Some("Attempt claimed; receiver acceptance has not been recorded.".into());
        self.record(writer, vec![(index, state)])?;
        Ok(true)
    }

    /// Keep a rejected or interrupted delivery, including its final error.
    pub fn record_failure(&self, index: u64, error: &str) -> Result<()> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        let mut state = self.state_of(&writer.view, index)?;
        anyhow::ensure!(
            state.status == DeliveryStatus::Queued,
            "batch is not queued"
        );
        state.last_error = Some(error.to_owned());
        if state.attempts >= MAX_ATTEMPTS {
            state.status = DeliveryStatus::Dead;
            state.next_attempt_at = None;
        }
        self.record(writer, vec![(index, state)])
    }

    /// A policy hold consumes no HTTP attempt and remains undelivered.
    pub fn hold(&self, index: u64, reason: &str) -> Result<()> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        let mut state = self.state_of(&writer.view, index)?;
        anyhow::ensure!(
            state.status == DeliveryStatus::Queued,
            "batch is not queued"
        );
        if state.hold_reason.as_deref() == Some(reason) {
            return Ok(());
        }
        state.hold_reason = Some(reason.to_owned());
        self.record(writer, vec![(index, state)])
    }

    /// Record acceptance for this batch alone, after its accepted event ids
    /// have been persisted. `accepted_ids` names this attempt's accepted subset.
    /// `instance_references_withheld` is how many `instance` members the
    /// attempt left out for a receiver that did not issue them; retention
    /// keeps such a batch for the whole period. No earlier queued or dead
    /// batch is acknowledged.
    pub fn record_acceptance(
        &self,
        index: u64,
        accepted_ids: &[uuid::Uuid],
        instance_references_withheld: u64,
    ) -> Result<()> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        let entry = self.read_entry(&writer.view, index)?;
        let events = entry.document["events"]
            .as_array()
            .context("batch has no events")?;
        let delivered_subset = events.iter().any(|event| {
            event["id"]
                .as_str()
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .is_none_or(|id| !accepted_ids.contains(&id))
        });
        let mut state = self.state_of(&writer.view, index)?;
        anyhow::ensure!(
            state.status == DeliveryStatus::Queued && state.attempts > 0,
            "only a claimed batch can be accepted"
        );
        state.status = DeliveryStatus::Delivered;
        state.delivered_subset = Some(delivered_subset);
        state.instance_references_withheld = Some(instance_references_withheld);
        state.next_attempt_at = None;
        state.last_error = None;
        state.hold_reason = None;
        self.record(writer, vec![(index, state)])
    }

    /// Explicitly start another schedule for dead batches. Retain the last
    /// attempt and error until another claim; queued and delivered are untouched.
    pub fn requeue(&self, index: Option<u64>, now: DateTime<Utc>) -> Result<u64> {
        let mut guard = self.writer()?;
        let writer = guard.as_mut().context(READ_ONLY)?;
        if let Some(index) = index {
            let state = self
                .state_of(&writer.view, index)
                .with_context(|| format!("no such spool batch: {index}"))?;
            anyhow::ensure!(
                state.status == DeliveryStatus::Dead,
                "batch {index} is not dead; requeue only admits dead batches"
            );
        }
        // A dead batch always has recorded metadata: only a claim or a
        // failure makes one.
        let changes: Vec<_> = writer
            .view
            .explicit
            .iter()
            .filter(|(i, state)| {
                state.status == DeliveryStatus::Dead && index.is_none_or(|index| index == **i)
            })
            .map(|(i, state)| {
                let mut state = state.clone();
                state.status = DeliveryStatus::Queued;
                state.attempts = 0;
                state.next_attempt_at = Some(now);
                (*i, state)
            })
            .collect();
        let count = changes.len() as u64;
        self.record(writer, changes)?;
        Ok(count)
    }

    /// Legacy acknowledgement prefix, used only when importing old spools.
    pub fn acked(&self) -> Result<u64> {
        if !self.ack_path.exists() {
            return Ok(0);
        }
        let text = std::fs::read_to_string(&self.ack_path).context("read ack file")?;
        text.trim().parse().context("parse ack offset")
    }

    fn writer(&self) -> Result<MutexGuard<'_, Option<Writer>>> {
        self.writer
            .lock()
            .map_err(|_| anyhow::anyhow!("a spool operation panicked; reopen the spool"))
    }

    /// Run `f` over the current view and the queue bytes: the writer's own
    /// state, or a fresh consistent read for a read-only spool.
    fn read<T>(&self, f: impl FnOnce(&View, &[u8]) -> Result<T>) -> Result<T> {
        if let Some(writer) = self.writer()?.as_ref() {
            return f(&writer.view, &read_or_empty(&self.queue_path)?);
        }
        let loaded = self.load()?;
        f(&loaded.view, &loaded.queue)
    }

    /// Read the queue, the snapshot and the journal as one generation. The
    /// journal is read before and after the other two: an unchanged
    /// generation means no compaction replaced it in between, and replaying
    /// its records over either the old or the new snapshot gives the same
    /// states. The writer holds the lock, so its first attempt succeeds.
    ///
    /// The generation does not cover the queue, which only grows between
    /// compactions: a reader can read the queue, and then a journal in which
    /// the writer has since enqueued and claimed a further batch. A read-only
    /// load takes a state for a batch its queue copy lacks as that stale
    /// read and reads again. Under the lock nothing else writes, so the same
    /// finding is damage and the writer reports it at once.
    fn load(&self) -> Result<Loaded> {
        let mut names_missing_batches = false;
        for _ in 0..LOAD_ATTEMPTS {
            let before = parse_journal(&read_or_empty(&self.journal_path)?)?.generation;
            let queue = read_or_empty(&self.queue_path)?;
            let mut explicit: BTreeMap<u64, DeliveryState> =
                match std::fs::read(&self.delivery_path) {
                    Ok(bytes) => {
                        serde_json::from_slice(&bytes).context("parse outbound.delivery.json")?
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                    Err(error) => return Err(error).context("read outbound.delivery.json"),
                };
            #[cfg(test)]
            if let Some(interleaved) = self.between_reads.lock().unwrap().take() {
                interleaved();
            }
            // Read before the journal's second read: a prune writes its
            // journal line before the marker, so a marker seen here beside
            // no journal there is a journal lost, never one not yet written.
            let pruning_recorded = self.pruning_recorded();
            let journal = parse_journal(&read_or_empty(&self.journal_path)?)?;
            if journal.generation != before {
                continue;
            }
            let mut queue_lines = parse_queue(&queue)?;
            let journal_records = journal.states.len();
            explicit.extend(journal.states);
            let pruned: BTreeSet<u64> = journal.prune.iter().copied().collect();
            queue_lines
                .lines
                .retain(|line| !pruned.contains(&line.index));
            explicit.retain(|index, _| !pruned.contains(index));
            names_missing_batches = explicit.keys().any(|index| {
                queue_lines
                    .lines
                    .binary_search_by_key(index, |line| line.index)
                    .is_err()
            });
            if names_missing_batches {
                anyhow::ensure!(self.lock.is_none(), MISSING_BATCHES);
                continue;
            }
            // The journal alone counts pruned deliveries. A spool that shows
            // pruning beside no journal has lost the count.
            let pruned_delivered = if journal.generation.is_none()
                && (queue_lines.pruning_evident || pruning_recorded)
            {
                None
            } else {
                journal
                    .pruned_delivered
                    .map(|count| count + pruned.len() as u64)
            };
            let next_index = journal
                .next_index
                .max(queue_lines.physical_lines)
                .max(queue_lines.lines.last().map_or(0, |line| line.index + 1));
            return Ok(Loaded {
                view: View {
                    lines: queue_lines.lines,
                    explicit,
                    generation: journal.generation,
                    next_index,
                    pruned_delivered,
                    journal_records,
                },
                queue,
                queue_len: queue_lines.len,
                queue_lacks_newline: queue_lines.lacks_newline,
                journal_len: journal.len,
                interrupted_prune: journal.prune,
            });
        }
        // Eight reads in a row were overtaken or stale. The caller reports
        // the spool as unavailable for this read; nothing is guessed. Only a
        // read-only load reaches here with the flag set, and it cannot tell a
        // queue copy a writer has overtaken from a queue that lost lines.
        anyhow::ensure!(!names_missing_batches, STALE_OR_MISSING_BATCHES);
        anyhow::bail!("the spool kept changing while it was read; retry")
    }

    /// Whether a prune left its marker. A marker whose presence cannot be
    /// established counts as present, because zero would be a guess.
    fn pruning_recorded(&self) -> bool {
        self.pruned_marker_path.try_exists().unwrap_or(true)
    }

    /// Drop what a crash left after the last complete line of the queue and
    /// the journal. Neither an enqueue nor a state change returned for those
    /// bytes, so nothing acted on them; appending after them would corrupt
    /// the next record. One case is kept: an unterminated final queue line
    /// that parses as a whole entry is given its newline, because a restored
    /// or hand-edited file may end that way. If it was an interrupted
    /// enqueue, the batch is delivered as queued and its event ids are read
    /// as already spooled, so nothing is projected twice.
    fn repair_tails(&self, loaded: &Loaded) -> Result<()> {
        if loaded.queue_lacks_newline {
            let mut file = OpenOptions::new().append(true).open(&self.queue_path)?;
            file.write_all(b"\n")?;
            file.sync_all().context("fsync spool")?;
        } else if loaded.queue.len() as u64 != loaded.queue_len {
            let file = OpenOptions::new().write(true).open(&self.queue_path)?;
            file.set_len(loaded.queue_len)?;
            file.sync_all().context("fsync spool")?;
        }
        if std::fs::metadata(&self.journal_path).is_ok_and(|meta| meta.len() != loaded.journal_len)
        {
            let file = OpenOptions::new().write(true).open(&self.journal_path)?;
            file.set_len(loaded.journal_len)?;
            file.sync_all().context("fsync delivery journal")?;
        }
        Ok(())
    }

    /// Every retained batch's state: recorded metadata, else the legacy
    /// import, else queued and unclaimed.
    fn full_states(&self, view: &View, queue: &[u8]) -> Result<BTreeMap<u64, DeliveryState>> {
        let acked = self.acked()?;
        let mut delivered = None;
        let mut states = view.explicit.clone();
        for line in &view.lines {
            if states.contains_key(&line.index) {
                continue;
            }
            let mut state = DeliveryState::queued();
            if line.index < acked {
                let delivered = match &delivered {
                    Some(delivered) => delivered,
                    None => delivered.insert(self.relay_state.delivered()?),
                };
                // Old acknowledgements also covered consent-withheld batches.
                // Only the delivered index proves receiver acceptance.
                if parse_entry(line, queue)?.document["events"]
                    .as_array()
                    .is_some_and(|events| {
                        !events.is_empty()
                            && events.iter().all(|event| {
                                event["id"]
                                    .as_str()
                                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                                    .is_some_and(|id| delivered.contains(&id))
                            })
                    })
                {
                    state.status = DeliveryStatus::Delivered;
                }
            }
            states.insert(line.index, state);
        }
        Ok(states)
    }

    fn state_of(&self, view: &View, index: u64) -> Result<DeliveryState> {
        if let Some(state) = view.explicit.get(&index) {
            return Ok(state.clone());
        }
        if index >= self.acked()? {
            anyhow::ensure!(view.line(index).is_some(), "no such spool batch");
            return Ok(DeliveryState::queued());
        }
        self.full_states(view, &read_or_empty(&self.queue_path)?)?
            .remove(&index)
            .context("no such spool batch")
    }

    fn read_entry(&self, view: &View, index: u64) -> Result<SpoolEntry> {
        let line = view.line(index).context("no such spool batch")?;
        let mut file = File::open(&self.queue_path).context("open spool")?;
        file.seek(SeekFrom::Start(line.start))?;
        let mut raw = vec![0; line.len];
        file.read_exact(&mut raw).context("read spool line")?;
        serde_json::from_slice(&raw).with_context(|| format!("parse spool line {index}"))
    }

    /// Append state changes to the journal and make them durable before they
    /// take effect in memory: a claim must survive a crash before its request.
    fn record(&self, writer: &mut Writer, changes: Vec<(u64, DeliveryState)>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        for (index, state) in &changes {
            serde_json::to_writer(
                &mut bytes,
                &JournalRecord::State {
                    index: *index,
                    state: state.clone(),
                },
            )?;
            bytes.push(b'\n');
        }
        self.append_to_journal(writer, &bytes)?;
        writer.view.journal_records += changes.len();
        writer.view.explicit.extend(changes);
        Ok(())
    }

    /// Append whole records. A failed or partial append is cut back to the
    /// last durable record and ends this writer's changes: bytes left in the
    /// middle of the journal would make every later read a parse error.
    fn append_to_journal(&self, writer: &mut Writer, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(!writer.failed, FAILED_WRITER);
        if writer.journal.is_none() {
            if writer.view.generation.is_none() {
                self.replace_journal(writer)?;
            }
            writer.journal = Some(
                OpenOptions::new()
                    .append(true)
                    .open(&self.journal_path)
                    .context("open delivery journal")?,
            );
        }
        let appended = writer.write_record(bytes);
        if appended.is_err() {
            writer.failed = true;
            // Best effort: the fault that failed the append can fail this
            // too, and the next writer truncates an unterminated tail.
            if let Some(journal) = writer.journal.take() {
                let _ = journal
                    .set_len(writer.journal_len)
                    .and_then(|()| journal.sync_all());
            }
            return appended;
        }
        writer.journal_len += bytes.len() as u64;
        Ok(())
    }

    /// Start the next journal generation: a header and no records.
    fn replace_journal(&self, writer: &mut Writer) -> Result<()> {
        let generation = writer.view.generation.unwrap_or(0) + 1;
        let mut bytes = serde_json::to_vec(&JournalRecord::Header {
            version: JOURNAL_VERSION,
            generation,
            next_index: writer.view.next_index,
            pruned_delivered: writer.view.pruned_delivered,
        })?;
        bytes.push(b'\n');
        self.replace(&self.journal_path, &bytes)
            .context("replace delivery journal")?;
        writer.journal = None;
        writer.journal_len = bytes.len() as u64;
        writer.view.generation = Some(generation);
        writer.view.journal_records = 0;
        Ok(())
    }

    fn replace(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        self.sync_directory()
    }

    /// Fold the journal into the snapshot, pruning what the retention rule
    /// releases. Runs when the writer closes; does nothing when neither a
    /// state change nor a prune is outstanding.
    fn compact(&self, now: DateTime<Utc>) -> Result<()> {
        let mut guard = self.writer()?;
        let Some(writer) = guard.as_mut() else {
            return Ok(());
        };
        anyhow::ensure!(!writer.failed, FAILED_WRITER);
        let queue = read_or_empty(&self.queue_path)?;
        let states = self.full_states(&writer.view, &queue)?;
        let prune = self.prunable(&writer.view, &states, &queue, now)?;
        if writer.view.journal_records == 0 && prune.is_empty() {
            return Ok(());
        }
        if !prune.is_empty() {
            let mut bytes = serde_json::to_vec(&JournalRecord::Prune {
                indices: prune.clone(),
                next_index: writer.view.next_index,
            })?;
            bytes.push(b'\n');
            self.append_to_journal(writer, &bytes)?;
            writer.reached(CompactionStep::PruneAnnounced)?;
            let pruned: BTreeSet<u64> = prune.iter().copied().collect();
            writer
                .view
                .lines
                .retain(|line| !pruned.contains(&line.index));
            writer
                .view
                .explicit
                .retain(|index, _| !pruned.contains(index));
            writer.view.pruned_delivered = writer
                .view
                .pruned_delivered
                .map(|count| count + prune.len() as u64);
        }
        self.fold(writer, &queue, !prune.is_empty())
    }

    /// The durable steps after a prune is announced, in the order that lets
    /// a crash at any point be completed from the journal: the queue loses
    /// the pruned batches, the snapshot absorbs the journal, and the journal
    /// is replaced last, because until then it still states everything the
    /// other two files may lack. `queue` is the file as read before the
    /// view dropped the pruned lines; the view's offsets refer to it.
    fn fold(&self, writer: &mut Writer, queue: &[u8], pruning: bool) -> Result<()> {
        // The snapshot has always carried every retained batch, so imported
        // and unclaimed states are written with the recorded ones.
        writer.view.explicit = self.full_states(&writer.view, queue)?;
        if pruning {
            self.record_pruning()?;
            let mut rewritten = Vec::new();
            for line in &mut writer.view.lines {
                let raw = &queue[line.start as usize..line.start as usize + line.len];
                let start = rewritten.len() as u64;
                if explicit_index(raw).is_some() {
                    rewritten.extend_from_slice(raw);
                    rewritten.push(b'\n');
                } else {
                    // A line from before indices were stored took its index
                    // from its position, which this rewrite changes.
                    rewritten.extend(stored_line(line.index, &parse_entry(line, queue)?)?.bytes());
                }
                *line = Line {
                    index: line.index,
                    start,
                    len: rewritten.len() - 1 - start as usize,
                };
            }
            self.replace(&self.queue_path, &rewritten)
                .context("replace spool")?;
            writer.queue_len = rewritten.len() as u64;
            writer.reached(CompactionStep::QueueRewritten)?;
        }
        self.replace(
            &self.delivery_path,
            &serde_json::to_vec_pretty(&writer.view.explicit)?,
        )
        .context("replace delivery metadata")?;
        writer.reached(CompactionStep::SnapshotReplaced)?;
        self.replace_journal(writer)
    }

    /// Leave `outbound.pruned` before the queue first loses a batch. A queue
    /// whose newest batches alone were pruned shows no gap, so without the
    /// marker a later loss of the journal would read as a count of zero.
    /// The marker is never removed and carries no count.
    fn record_pruning(&self) -> Result<()> {
        if self.pruned_marker_path.try_exists().unwrap_or(false) {
            return Ok(());
        }
        self.replace(
            &self.pruned_marker_path,
            b"Delivered batches have been pruned from this spool. Only \
              outbound.delivery.journal holds their count; keep this file.\n",
        )
        .context("write outbound.pruned")
    }

    /// Delivered batches the retention rule releases and no exemption keeps,
    /// lowest index first; empty until they are at least one in
    /// `PRUNE_FRACTION` of the retained batches, so the queue file is not
    /// rewritten for every delivery.
    fn prunable(
        &self,
        view: &View,
        states: &BTreeMap<u64, DeliveryState>,
        queue: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Vec<u64>> {
        let delivered: Vec<(u64, &DeliveryState)> = states
            .iter()
            .filter(|(_, state)| state.status == DeliveryStatus::Delivered)
            .map(|(index, state)| (*index, state))
            .collect();
        let beyond_count = delivered.len().saturating_sub(RETAIN_DELIVERED_BATCHES);
        let released: Vec<u64> = delivered
            .iter()
            .enumerate()
            .filter(|(position, (_, state))| {
                // The retained payload of a batch delivered without its
                // `instance` members is the only spooled copy that carries
                // them for the issuing hub. It gets the whole period whatever
                // the delivery volume.
                let members_withheld = state.instance_references_withheld.unwrap_or(0) > 0;
                (*position < beyond_count && !members_withheld)
                    || state
                        .last_attempt_at
                        .is_some_and(|at| now - at >= Duration::days(RETAIN_DELIVERED_DAYS))
            })
            .map(|(_, (index, _))| *index)
            .collect();
        if released.is_empty() || released.len() * PRUNE_FRACTION < view.lines.len() {
            return Ok(Vec::new());
        }
        // A delivered payload is the only record of the agent id first sent
        // for a session delivered before identity pins existed. Keep it
        // until the relay has pinned that session.
        let pins = self.relay_state.session_agents()?;
        let mut prune = Vec::new();
        for index in released {
            let line = view.line(index).context("no such spool batch")?;
            let document = parse_entry(line, queue)?.document;
            let session = document["session_id"]
                .as_str()
                .and_then(|id| uuid::Uuid::parse_str(id).ok());
            if document["agent_id"].is_string()
                && session.is_some_and(|session| !pins.contains_key(&session))
            {
                continue;
            }
            prune.push(index);
        }
        // Exempt batches stay released close after close. Counting them
        // would rewrite the queue file for every batch beside them.
        if prune.len() * PRUNE_FRACTION < view.lines.len() {
            return Ok(Vec::new());
        }
        Ok(prune)
    }

    /// Fold the journal as the writer closes and keep the reason if that
    /// fails, because `Drop` cannot return it. A failed close loses nothing:
    /// the journal stays in place and the next writer replays it, completing
    /// an announced prune. Repeated failures would grow the journal unseen,
    /// so status shows the reason (`SpoolSnapshot::close_error`). Writing
    /// the reason is best effort: the fault that failed the fold can fail
    /// this too.
    fn close(&self) {
        match self.compact(Utc::now()) {
            Ok(()) => {
                if self.close_error_path.exists() {
                    let _ = std::fs::remove_file(&self.close_error_path);
                }
            }
            Err(error) => {
                let _ = std::fs::write(&self.close_error_path, format!("{error:#}\n"));
            }
        }
    }

    /// The reason the last close left, for status alone. The file is
    /// written without a rename or fsync and nothing else depends on it, so
    /// no fault in it may stop a read of the spool or make one costly. The
    /// first `CLOSE_ERROR_LIMIT` bytes are read, whatever the file's size,
    /// because every status and console poll reads it. Bytes that are not
    /// UTF-8 are shown with replacement characters, and control characters
    /// become spaces, because status prints the reason to a terminal. A file
    /// that does not read is the `Err`, which says nothing about the close.
    /// An empty file is a write in progress.
    fn close_error(&self) -> std::result::Result<Option<String>, String> {
        let mut bytes = Vec::new();
        let read = File::open(&self.close_error_path)
            .and_then(|file| file.take(CLOSE_ERROR_LIMIT).read_to_end(&mut bytes));
        match read {
            Ok(_) => {
                let reason: String = String::from_utf8_lossy(&bytes)
                    .chars()
                    .map(|character| {
                        if character.is_control() {
                            ' '
                        } else {
                            character
                        }
                    })
                    .collect();
                Ok(Some(reason.trim().to_owned()).filter(|reason| !reason.is_empty()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        File::open(self.queue_path.parent().context("spool directory")?)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Ok(Some(writer)) = self.writer.get_mut().map(|writer| writer.as_ref())
            && writer.abandoned
        {
            return;
        }
        if self.lock.is_some() {
            self.close();
        }
    }
}

/// What status reads of `outbound.close-error`. A reason is one line.
const CLOSE_ERROR_LIMIT: u64 = 4096;
const READ_ONLY: &str = "read-only spool cannot be changed";
const MISSING_BATCHES: &str = "delivery metadata names batches the spool queue lacks; nothing is \
    delivered until the lines are restored or, for a line removed as damaged, its metadata is \
    removed (docs/contracts/session-evidence.md §Delivery state)";
const STALE_OR_MISSING_BATCHES: &str = "delivery metadata names batches this read of the spool \
    queue lacks; retry, and if it persists with no relay running, the spool has lost lines";
const FAILED_WRITER: &str = "a delivery journal append failed earlier in this process; \
    nothing further is written until the spool is reopened";

impl View {
    fn line(&self, index: u64) -> Option<&Line> {
        self.lines
            .binary_search_by_key(&index, |line| line.index)
            .ok()
            .map(|at| &self.lines[at])
    }
}

impl Writer {
    fn write_record(&mut self, bytes: &[u8]) -> Result<()> {
        let journal = self.journal.as_mut().context("delivery journal")?;
        #[cfg(test)]
        if let Some(written) = self.fail_append_after.take() {
            journal.write_all(&bytes[..written])?;
            anyhow::bail!("injected append failure after {written} bytes");
        }
        journal
            .write_all(bytes)
            .context("append delivery journal")?;
        journal.sync_all().context("fsync delivery journal")
    }

    /// Fault injection for the crash-recovery tests; nothing in a release
    /// build can set a crash point.
    fn reached(&self, step: CompactionStep) -> Result<()> {
        #[cfg(test)]
        anyhow::ensure!(self.crash_at != Some(step), "injected crash at {step:?}");
        let _ = step;
        Ok(())
    }
}

fn read_or_empty(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

/// One queue line: the entry with its spool index as the first member, so a
/// scan finds the index without parsing the payload.
fn stored_line(index: u64, entry: &SpoolEntry) -> Result<String> {
    let entry = serde_json::to_string(entry).context("serialise spool entry")?;
    let members = entry
        .strip_prefix('{')
        .context("spool entry is not an object")?;
    Ok(format!("{{\"index\":{index},{members}\n"))
}

fn explicit_index(raw: &[u8]) -> Option<u64> {
    let digits = raw.strip_prefix(b"{\"index\":")?;
    let end = digits.iter().position(|byte| !byte.is_ascii_digit())?;
    std::str::from_utf8(&digits[..end]).ok()?.parse().ok()
}

fn parse_entry(line: &Line, queue: &[u8]) -> Result<SpoolEntry> {
    let raw = queue
        .get(line.start as usize..line.start as usize + line.len)
        .context("spool changed while it was read; retry")?;
    serde_json::from_slice(raw).with_context(|| format!("parse spool line {}", line.index))
}

struct QueueLines {
    lines: Vec<Line>,
    /// Lines of any kind: the index of the next batch in a spool written
    /// before indices were stored, where a batch's index is its line number.
    physical_lines: u64,
    len: u64,
    lacks_newline: bool,
    /// A stored index above the one that would follow the line before it.
    /// Enqueues number batches consecutively, so only a prune leaves a gap.
    pruning_evident: bool,
}

/// `SpoolEntry` without building its document: what every queue line must
/// parse as. A line that does not is damage, reported whenever the spool is
/// read (by `open`, status and a dry run) and never skipped. Delivered lines
/// are otherwise parsed only by a prune, whose failure at close would go
/// unseen by the run that caused it.
#[derive(Deserialize)]
struct WholeEntry {
    #[serde(rename = "origin")]
    _origin: String,
    #[serde(default, rename = "queued_at")]
    _queued_at: Option<DateTime<Utc>>,
    #[serde(rename = "document")]
    _document: serde::de::IgnoredAny,
    #[serde(default, rename = "directory_selection")]
    _directory_selection: bool,
}

fn parse_queue(queue: &[u8]) -> Result<QueueLines> {
    let mut parsed = QueueLines {
        lines: Vec::new(),
        physical_lines: 0,
        len: 0,
        lacks_newline: false,
        pruning_evident: false,
    };
    let mut start = 0;
    while start < queue.len() {
        let (end, terminated) = match queue[start..].iter().position(|byte| *byte == b'\n') {
            Some(at) => (start + at, true),
            None => (queue.len(), false),
        };
        let raw = &queue[start..end];
        // An unterminated tail is an interrupted enqueue unless it is a whole
        // entry, as a hand-edited or restored file may end.
        if !terminated && serde_json::from_slice::<WholeEntry>(raw).is_err() {
            break;
        }
        if !raw.trim_ascii().is_empty() {
            let stored = explicit_index(raw);
            let index = stored.unwrap_or(parsed.physical_lines);
            anyhow::ensure!(
                parsed.lines.last().is_none_or(|last| last.index < index),
                "spool line {} is out of order; nothing is delivered until the line is repaired \
                 (docs/contracts/session-evidence.md §Delivery state)",
                parsed.physical_lines
            );
            serde_json::from_slice::<WholeEntry>(raw).with_context(|| {
                format!(
                    "parse spool line {index}; nothing is delivered until the line is repaired \
                     (docs/contracts/session-evidence.md §Delivery state)"
                )
            })?;
            let follows = parsed.lines.last().map_or(0, |last| last.index + 1);
            parsed.pruning_evident |= stored.is_some_and(|stored| stored > follows);
            parsed.lines.push(Line {
                index,
                start: start as u64,
                len: raw.len(),
            });
        }
        parsed.physical_lines += 1;
        parsed.lacks_newline = !terminated;
        start = end + 1;
        parsed.len = (end + usize::from(terminated)) as u64;
    }
    Ok(parsed)
}

fn parse_journal(bytes: &[u8]) -> Result<Journal> {
    let mut journal = Journal {
        generation: None,
        next_index: 0,
        pruned_delivered: Some(0),
        states: Vec::new(),
        prune: Vec::new(),
        len: 0,
    };
    let mut start = 0;
    // A record without its newline was never made durable as a whole.
    while let Some(at) = bytes[start..].iter().position(|byte| *byte == b'\n') {
        let record = serde_json::from_slice(&bytes[start..start + at])
            .with_context(|| format!("parse outbound.delivery.journal at byte {start}"))?;
        match record {
            JournalRecord::Header {
                version,
                generation,
                next_index,
                pruned_delivered,
            } => {
                anyhow::ensure!(
                    start == 0 && version == JOURNAL_VERSION,
                    "outbound.delivery.journal has an unsupported header; use the relay that wrote it"
                );
                journal.generation = Some(generation);
                journal.next_index = next_index;
                journal.pruned_delivered = pruned_delivered;
            }
            JournalRecord::State { index, state } => journal.states.push((index, state)),
            JournalRecord::Prune {
                indices,
                next_index,
            } => {
                journal.prune.extend(indices);
                journal.next_index = journal.next_index.max(next_index);
            }
        }
        anyhow::ensure!(
            journal.generation.is_some(),
            "outbound.delivery.journal has no header; restore the spool before delivery"
        );
        start += at + 1;
        journal.len = start as u64;
    }
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn entry(document: Value) -> SpoolEntry {
        SpoolEntry {
            origin: "test".into(),
            queued_at: Some(Utc::now()),
            directory_selection: false,
            document,
        }
    }

    fn one_event() -> (Uuid, Value) {
        let id = Uuid::new_v4();
        (id, json!({"events": [{"id": id}]}))
    }

    /// Enqueue a batch and record its acceptance at `at`.
    fn deliver(spool: &Spool, document: Value, at: DateTime<Utc>) -> u64 {
        let ids: Vec<Uuid> = document["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].as_str().unwrap().parse().unwrap())
            .collect();
        let index = spool.enqueue(&entry(document)).unwrap();
        assert!(spool.claim(index, at).unwrap());
        spool.record_acceptance(index, &ids, 0).unwrap();
        index
    }

    /// Release the lock without compaction: the durable state a killed
    /// process leaves.
    fn abandon(mut spool: Spool) {
        spool.writer.get_mut().unwrap().as_mut().unwrap().abandoned = true;
    }

    fn file(home: &Path, name: &str) -> Vec<u8> {
        std::fs::read(home.join("relay/spool").join(name)).unwrap()
    }

    fn journal_lines(home: &Path) -> Vec<Value> {
        String::from_utf8(file(home, "outbound.delivery.journal"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The layout written before the journal: payload lines without an
    /// index, every state in the snapshot, no journal file.
    #[test]
    fn an_existing_spool_directory_opens_without_loss() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("relay/spool");
        std::fs::create_dir_all(&dir).unwrap();
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let queue: String = ids
            .iter()
            .map(|id| {
                format!(
                    "{}\n",
                    json!({"origin": "earlier relay", "queued_at": "2026-09-01T10:00:00Z",
                        "document": {"agent_id": "commonmeasure", "events": [{"id": id}]},
                        "directory_selection": true})
                )
            })
            .collect();
        std::fs::write(dir.join("outbound.ndjson"), &queue).unwrap();
        // Batch 3 has no metadata: enqueued by a process killed before its store.
        let snapshot = json!({
            "0": {"status": "delivered", "attempts": 1, "next_attempt_at": null,
                "last_attempt_at": "2026-09-01T10:00:05Z", "last_error": null,
                "hold_reason": null, "delivered_subset": true},
            "1": {"status": "queued", "attempts": 3,
                "next_attempt_at": "2026-09-01T10:07:00Z",
                "last_attempt_at": "2026-09-01T10:03:00Z",
                "last_error": "receiver answered 503",
                "hold_reason": "Held by current directory consent or source policy; undelivered."},
            "2": {"status": "dead", "attempts": 10, "next_attempt_at": null,
                "last_attempt_at": "2026-09-01T14:03:00Z", "last_error": "receiver answered 503"},
        });
        std::fs::write(
            dir.join("outbound.delivery.json"),
            serde_json::to_vec_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let check = |spool: &Spool| {
            let entries = spool.entries().unwrap();
            assert_eq!(
                entries.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                [0, 1, 2, 3]
            );
            for ((_, entry), id) in entries.iter().zip(&ids) {
                assert_eq!(entry.origin, "earlier relay");
                assert!(entry.directory_selection);
                assert_eq!(entry.document["events"][0]["id"], json!(id));
            }
            let states = spool.delivery_states().unwrap();
            for index in 0..3u64 {
                let mut expected = snapshot[index.to_string()].clone();
                let mut actual = serde_json::to_value(&states[&index]).unwrap();
                for state in [&mut expected, &mut actual] {
                    let members = state.as_object_mut().unwrap();
                    members.entry("hold_reason").or_insert(Value::Null);
                }
                assert_eq!(actual, expected, "batch {index}");
            }
            assert_eq!(states[&3].status, DeliveryStatus::Queued);
            assert_eq!(states[&3].attempts, 0);
            assert_eq!(
                spool
                    .pending()
                    .unwrap()
                    .iter()
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>(),
                [1, 2, 3]
            );
        };
        check(&Spool::read_only(home.path()));
        assert!(
            !dir.join("outbound.delivery.journal").exists() && !dir.join("delivery.lock").exists(),
            "a read-only view creates nothing"
        );

        let spool = Spool::open(home.path()).unwrap();
        check(&spool);
        let (_, document) = one_event();
        assert_eq!(spool.enqueue(&entry(document)).unwrap(), 4);
        assert!(spool.claim(3, Utc::now()).unwrap());
        assert_eq!(spool.requeue(Some(2), Utc::now()).unwrap(), 1);
        drop(spool);

        let reopened = Spool::read_only(home.path());
        let states = reopened.delivery_states().unwrap();
        assert_eq!(states.len(), 5);
        assert_eq!(states[&0].delivered_subset, Some(true));
        assert_eq!(states[&1].attempts, 3);
        assert_eq!(states[&2].status, DeliveryStatus::Queued);
        assert_eq!(states[&3].attempts, 1);
        assert_eq!(states[&4].attempts, 0);
        assert_eq!(reopened.snapshot().unwrap().pruned_delivered, Some(0));
        assert!(
            file(home.path(), "outbound.ndjson").starts_with(queue.as_bytes()),
            "earlier payload lines are not rewritten"
        );
    }

    #[test]
    fn a_state_change_is_one_journal_record_and_the_close_folds_it_into_the_snapshot() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let spool = Spool::open(home.path()).unwrap();
        for _ in 0..3 {
            spool.enqueue(&entry(one_event().1)).unwrap();
        }
        assert!(
            !home
                .path()
                .join("relay/spool/outbound.delivery.journal")
                .exists(),
            "an unclaimed batch needs no metadata"
        );
        assert!(spool.claim(1, now).unwrap());
        drop(spool);
        let snapshot = file(home.path(), "outbound.delivery.json");
        assert_eq!(journal_lines(home.path()).len(), 1, "header only");

        let spool = Spool::open(home.path()).unwrap();
        spool.record_failure(1, "receiver answered 503").unwrap();
        assert!(spool.claim(2, now).unwrap());
        assert!(file(home.path(), "outbound.delivery.json") == snapshot);
        let journal = journal_lines(home.path());
        assert_eq!(journal.len(), 3);
        assert_eq!(journal[1]["state"]["index"], 1);
        assert_eq!(journal[2]["state"]["index"], 2);
        // A reader sees the journalled changes while the writer still runs.
        let seen = Spool::read_only(home.path()).delivery_states().unwrap();
        assert_eq!(
            seen[&1].last_error.as_deref(),
            Some("receiver answered 503")
        );
        assert_eq!(seen[&2].attempts, 1);
        let generation = journal[0]["header"]["generation"].as_u64().unwrap();
        drop(spool);

        let folded: BTreeMap<u64, DeliveryState> =
            serde_json::from_slice(&file(home.path(), "outbound.delivery.json")).unwrap();
        assert_eq!(folded.len(), 3);
        assert_eq!(
            folded[&1].last_error.as_deref(),
            Some("receiver answered 503")
        );
        assert_eq!(folded[&2].attempts, 1);
        let journal = journal_lines(home.path());
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0]["header"]["generation"], generation + 1);

        // A close with nothing to fold writes nothing.
        let before = file(home.path(), "outbound.delivery.journal");
        drop(Spool::open(home.path()).unwrap());
        assert!(file(home.path(), "outbound.delivery.journal") == before);
    }

    /// Fault injection: the process stops after journal appends and before
    /// compaction, including part-way through an append and an enqueue.
    #[test]
    fn a_crash_before_compaction_keeps_claims_and_acceptances() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let spool = Spool::open(home.path()).unwrap();
        let accepted = deliver(&spool, one_event().1, now);
        let claimed = spool.enqueue(&entry(one_event().1)).unwrap();
        assert!(spool.claim(claimed, now).unwrap());
        abandon(spool);
        assert!(
            !home
                .path()
                .join("relay/spool/outbound.delivery.json")
                .exists(),
            "nothing was compacted"
        );
        for name in ["outbound.delivery.journal", "outbound.ndjson"] {
            let mut file = OpenOptions::new()
                .append(true)
                .open(home.path().join("relay/spool").join(name))
                .unwrap();
            file.write_all(br#"{"state":{"index":1,"sta"#).unwrap();
        }

        let check = |spool: &Spool| {
            let states = spool.delivery_states().unwrap();
            assert_eq!(states.len(), 2);
            assert_eq!(states[&accepted].status, DeliveryStatus::Delivered);
            assert_eq!(states[&claimed].status, DeliveryStatus::Queued);
            assert_eq!(states[&claimed].attempts, 1);
            assert_eq!(
                states[&claimed].next_attempt_at,
                Some(now + Duration::seconds(RETRY_BASE_SECONDS))
            );
            assert_eq!(spool.pending().unwrap().len(), 1);
        };
        check(&Spool::read_only(home.path()));
        let spool = Spool::open(home.path()).unwrap();
        check(&spool);
        assert!(
            !spool.claim(claimed, now).unwrap(),
            "the claim's deadline holds"
        );
        // Appends after the torn tails are whole records.
        assert_eq!(spool.enqueue(&entry(one_event().1)).unwrap(), 2);
        spool.record_failure(claimed, "outcome lost").unwrap();
        abandon(spool);
        let spool = Spool::open(home.path()).unwrap();
        assert_eq!(spool.entries().unwrap().len(), 3);
        assert_eq!(
            spool.delivery_states().unwrap()[&claimed]
                .last_error
                .as_deref(),
            Some("outcome lost")
        );
        // The replayed journal is folded by the next clean close, although
        // this writer recorded nothing itself: the bound on journal growth
        // across killed processes.
        assert!(journal_lines(home.path()).len() > 1);
        drop(spool);
        assert_eq!(journal_lines(home.path()).len(), 1, "header only");
        let folded: BTreeMap<u64, DeliveryState> =
            serde_json::from_slice(&file(home.path(), "outbound.delivery.json")).unwrap();
        assert_eq!(folded.len(), 3);
        assert_eq!(folded[&accepted].status, DeliveryStatus::Delivered);
        assert_eq!(folded[&claimed].last_error.as_deref(), Some("outcome lost"));
    }

    /// Fault injection: the process stops between each pair of durable steps
    /// of a compaction that prunes. Every reader and the next writer see the
    /// completed result, and no index is reused.
    #[test]
    fn a_crash_at_any_compaction_step_completes_the_prune() {
        for step in [
            CompactionStep::PruneAnnounced,
            CompactionStep::QueueRewritten,
            CompactionStep::SnapshotReplaced,
        ] {
            let home = tempfile::tempdir().unwrap();
            let now = Utc::now();
            let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS + 1);
            let spool = Spool::open(home.path()).unwrap();
            let queued = spool.enqueue(&entry(one_event().1)).unwrap();
            let dead = spool.enqueue(&entry(one_event().1)).unwrap();
            let mut at = long_ago;
            for _ in 0..MAX_ATTEMPTS {
                assert!(spool.claim(dead, at).unwrap());
                at += Duration::seconds(RETRY_CEILING_SECONDS);
            }
            spool.record_failure(dead, "receiver answered 503").unwrap();
            let recent = deliver(&spool, one_event().1, now);
            // The newest batch is pruned, so its index is only in the journal.
            let old: Vec<u64> = (0..3)
                .map(|_| deliver(&spool, one_event().1, long_ago))
                .collect();
            spool.writer.lock().unwrap().as_mut().unwrap().crash_at = Some(step);
            let error = spool.compact(now).unwrap_err();
            assert!(format!("{error:#}").contains("injected crash"), "{error:#}");
            abandon(spool);
            // Once the queue has lost a batch the marker is already there.
            let marker = home.path().join("relay/spool/outbound.pruned");
            assert_eq!(
                marker.exists(),
                step != CompactionStep::PruneAnnounced,
                "{step:?}"
            );

            let check = |spool: &Spool| {
                let snapshot = spool.snapshot().unwrap();
                assert_eq!(
                    snapshot.states.keys().copied().collect::<Vec<_>>(),
                    [queued, dead, recent],
                    "{step:?}"
                );
                assert_eq!(snapshot.states[&dead].status, DeliveryStatus::Dead);
                assert_eq!(snapshot.states[&recent].status, DeliveryStatus::Delivered);
                assert_eq!(snapshot.pruned_delivered, Some(old.len() as u64));
                assert_eq!(
                    snapshot
                        .undelivered
                        .iter()
                        .map(|(index, _)| *index)
                        .collect::<Vec<_>>(),
                    [queued, dead]
                );
                assert_eq!(spool.entries().unwrap().len(), 3);
            };
            check(&Spool::read_only(home.path()));
            let spool = Spool::open(home.path()).unwrap();
            check(&spool);
            assert!(marker.exists(), "{step:?}: completing the prune leaves it");
            let journal = journal_lines(home.path());
            assert_eq!(
                journal.len(),
                1,
                "{step:?}: the prune was completed at open"
            );
            assert_eq!(journal[0]["header"]["pruned_delivered"], 3);
            let retained = String::from_utf8(file(home.path(), "outbound.ndjson")).unwrap();
            assert_eq!(retained.lines().count(), 3);
            let folded: BTreeMap<u64, DeliveryState> =
                serde_json::from_slice(&file(home.path(), "outbound.delivery.json")).unwrap();
            assert_eq!(
                folded.keys().copied().collect::<Vec<_>>(),
                [queued, dead, recent]
            );
            assert_eq!(
                spool.enqueue(&entry(one_event().1)).unwrap(),
                old[2] + 1,
                "{step:?}: a pruned index is not given to a new batch"
            );
            assert_eq!(spool.requeue(None, now).unwrap(), 1);
            drop(spool);
            check_indices_survive_reopen(home.path(), old[2] + 2);
        }
    }

    fn check_indices_survive_reopen(home: &Path, expected: u64) {
        let spool = Spool::open(home).unwrap();
        assert_eq!(spool.enqueue(&entry(one_event().1)).unwrap(), expected);
    }

    #[test]
    fn retention_prunes_old_delivered_batches_and_nothing_else() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS);
        let pinned = Uuid::new_v4();
        let unpinned = Uuid::new_v4();
        crate::state::RelayState::open(home.path())
            .unwrap()
            .pin_session_agent(pinned, "commonmeasure")
            .unwrap();
        let session_batch = |session: Uuid| {
            json!({"session_id": session, "agent_id": "commonmeasure",
                "events": [{"id": Uuid::new_v4()}]})
        };
        let spool = Spool::open(home.path()).unwrap();
        let old_pinned = deliver(&spool, session_batch(pinned), long_ago);
        let old_unpinned = deliver(&spool, session_batch(unpinned), long_ago);
        let recent = deliver(
            &spool,
            session_batch(pinned),
            long_ago + Duration::seconds(1),
        );
        let queued = spool.enqueue(&entry(session_batch(pinned))).unwrap();
        spool.compact(now).unwrap();
        assert_eq!(
            spool
                .delivery_states()
                .unwrap()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [old_unpinned, recent, queued],
            "the unpinned session's payload still carries its first agent id"
        );
        assert_eq!(spool.snapshot().unwrap().pruned_delivered, Some(1));
        assert!(
            spool.claim(old_pinned, now).is_err(),
            "a pruned batch is gone, not requeued"
        );

        // Below one released batch in eight retained, the queue file is left alone.
        for _ in 0..14 {
            spool.enqueue(&entry(session_batch(pinned))).unwrap();
        }
        let before = file(home.path(), "outbound.ndjson");
        spool.compact(now + Duration::days(1)).unwrap();
        assert!(file(home.path(), "outbound.ndjson") == before);
        assert!(spool.delivery_states().unwrap().contains_key(&recent));
    }

    /// Deliveries with no recorded time, as a legacy import leaves them, are
    /// bounded by count.
    #[test]
    fn retention_keeps_the_newest_delivered_batches_when_their_age_is_unknown() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("relay/spool");
        std::fs::create_dir_all(&dir).unwrap();
        let total = RETAIN_DELIVERED_BATCHES as u64 + 200;
        let mut queue = String::new();
        let mut states = serde_json::Map::new();
        for index in 0..total {
            queue.push_str(&json!({"origin": "legacy", "document": one_event().1}).to_string());
            queue.push('\n');
            states.insert(
                index.to_string(),
                json!({"status": "delivered", "attempts": 1, "next_attempt_at": null,
                    "last_attempt_at": null, "last_error": null}),
            );
        }
        std::fs::write(dir.join("outbound.ndjson"), queue).unwrap();
        std::fs::write(
            dir.join("outbound.delivery.json"),
            serde_json::to_vec(&states).unwrap(),
        )
        .unwrap();
        drop(Spool::open(home.path()).unwrap());
        let snapshot = Spool::read_only(home.path()).snapshot().unwrap();
        assert_eq!(snapshot.pruned_delivered, Some(200));
        assert_eq!(
            snapshot.states.keys().copied().collect::<Vec<_>>(),
            (200..total).collect::<Vec<_>>()
        );
        // Rewritten legacy lines now carry the index their position gave them.
        let entries = Spool::read_only(home.path()).entries().unwrap();
        assert_eq!(entries[0].0, 200);
        assert_eq!(entries[0].1.origin, "legacy");
        assert_eq!(
            Spool::open(home.path())
                .unwrap()
                .enqueue(&entry(one_event().1))
                .unwrap(),
            total
        );
    }

    /// Fault injection: a compaction completes after a reader has read the
    /// queue and snapshot and before it reads the journal. Taking the new,
    /// empty journal with the old snapshot would lose the journalled
    /// acceptance; taking the old queue with it would show a pruned batch as
    /// queued. The reader must notice the new generation and read again.
    #[test]
    fn a_reader_overtaken_by_a_compaction_reads_again() {
        for prune in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let now = Utc::now();
            let writer = std::sync::Arc::new(Spool::open(home.path()).unwrap());
            let queued = writer.enqueue(&entry(one_event().1)).unwrap();
            writer.compact(now).unwrap();
            let accepted = deliver(&writer, one_event().1, now);

            let reader = Spool::read_only(home.path());
            let compacting = writer.clone();
            let ahead = Duration::days(if prune { RETAIN_DELIVERED_DAYS } else { 0 });
            *reader.between_reads.lock().unwrap() =
                Some(Box::new(move || compacting.compact(now + ahead).unwrap()));
            let snapshot = reader.snapshot().unwrap();
            assert!(
                reader.between_reads.lock().unwrap().is_none(),
                "the compaction ran"
            );
            assert_eq!(snapshot.pruned_delivered, Some(u64::from(prune)));
            assert_eq!(snapshot.states.contains_key(&accepted), !prune);
            if !prune {
                assert_eq!(snapshot.states[&accepted].status, DeliveryStatus::Delivered);
            }
            assert_eq!(
                snapshot
                    .undelivered
                    .iter()
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>(),
                [queued]
            );
        }
    }

    /// A reader runs beside a writer that journals, compacts and prunes. A
    /// smoke test: it establishes that renames, appends in progress and
    /// retries produce no error under real scheduling, that the delivered
    /// count never falls, and that it stays within one of the undelivered
    /// count, as every cycle of this writer leaves it. It does not establish
    /// the interleavings: it passes with the generation check removed, which
    /// is why the seam tests above and below exist.
    #[test]
    fn a_concurrent_reader_sees_whole_generations() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let home = tempfile::tempdir().unwrap();
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let spool = Spool::open(home.path()).unwrap();
        let reader = {
            let home = home.path().to_owned();
            let done = done.clone();
            std::thread::spawn(move || {
                let (mut reads, mut delivered) = (0u64, 0u64);
                while !done.load(Ordering::SeqCst) {
                    let snapshot = match Spool::read_only(&home).snapshot() {
                        Ok(snapshot) => snapshot,
                        Err(error) if format!("{error:#}").contains("kept changing") => continue,
                        Err(error) => panic!("{error:#}"),
                    };
                    let seen = snapshot.pruned_delivered.unwrap()
                        + snapshot
                            .states
                            .values()
                            .filter(|state| state.status == DeliveryStatus::Delivered)
                            .count() as u64;
                    assert!(
                        seen >= delivered,
                        "delivered fell from {delivered} to {seen}"
                    );
                    // Each cycle delivers one batch and leaves one queued, so
                    // a whole view has the two counts within one of each
                    // other. A pruned count taken with the states it replaced
                    // would count those deliveries twice.
                    assert!(
                        seen.abs_diff(snapshot.undelivered.len() as u64) <= 1,
                        "{seen} delivered beside {} undelivered",
                        snapshot.undelivered.len()
                    );
                    delivered = seen;
                    reads += 1;
                }
                (reads, delivered)
            })
        };
        let now = Utc::now();
        let cycles = 60;
        for cycle in 0..cycles {
            deliver(&spool, one_event().1, now);
            spool.enqueue(&entry(one_event().1)).unwrap();
            // Every third compaction runs far enough ahead to prune.
            let ahead = if cycle % 3 == 0 {
                RETAIN_DELIVERED_DAYS + 1
            } else {
                0
            };
            spool.compact(now + Duration::days(ahead)).unwrap();
        }
        done.store(true, Ordering::SeqCst);
        let (reads, _) = reader.join().unwrap();
        assert!(reads > 0);
        let snapshot = spool.snapshot().unwrap();
        assert_eq!(snapshot.undelivered.len(), cycles);
        assert!(snapshot.pruned_delivered.unwrap() > 0);
    }

    /// Fault injection: the writer enqueues and claims a batch after a reader
    /// has read the queue and before it reads the journal again. The
    /// generation is unchanged, so only the journal's state for a batch the
    /// reader's queue copy lacks shows the read is stale. It must read again,
    /// not report a healthy spool as damaged.
    #[test]
    fn a_reader_overtaken_by_an_enqueue_and_claim_reads_again() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let writer = std::sync::Arc::new(Spool::open(home.path()).unwrap());
        let first = writer.enqueue(&entry(one_event().1)).unwrap();
        assert!(writer.claim(first, now).unwrap());

        let reader = Spool::read_only(home.path());
        let racing = writer.clone();
        *reader.between_reads.lock().unwrap() = Some(Box::new(move || {
            let index = racing.enqueue(&entry(one_event().1)).unwrap();
            assert!(racing.claim(index, now).unwrap());
        }));
        let snapshot = reader.snapshot().unwrap();
        assert!(
            reader.between_reads.lock().unwrap().is_none(),
            "the enqueue and claim ran"
        );
        assert_eq!(
            snapshot
                .undelivered
                .iter()
                .map(|(index, _)| *index)
                .collect::<Vec<_>>(),
            [first, first + 1]
        );
        assert_eq!(snapshot.states[&(first + 1)].attempts, 1);
    }

    /// Metadata for a batch the queue really lacks is still an error: from a
    /// reader once its attempts are spent, from a writer at once. The reader
    /// cannot tell lost lines from a queue copy a writer overtook eight
    /// times, so only the writer, which holds the lock, says to restore.
    #[test]
    fn metadata_for_a_batch_the_queue_lacks_is_reported() {
        let home = tempfile::tempdir().unwrap();
        let spool = Spool::open(home.path()).unwrap();
        deliver(&spool, one_event().1, Utc::now());
        deliver(&spool, one_event().1, Utc::now());
        drop(spool);
        let queue = String::from_utf8(file(home.path(), "outbound.ndjson")).unwrap();
        let first_line = queue.lines().next().unwrap();
        std::fs::write(
            home.path().join("relay/spool/outbound.ndjson"),
            format!("{first_line}\n"),
        )
        .unwrap();
        let reader = format!(
            "{:#}",
            Spool::read_only(home.path()).snapshot().err().unwrap()
        );
        assert!(
            reader.contains("this read of the spool queue lacks; retry"),
            "{reader}"
        );
        assert!(!reader.contains("restore the spool"), "{reader}");
        let writer = format!("{:#}", Spool::open(home.path()).err().unwrap());
        assert!(
            writer.contains("names batches the spool queue lacks; nothing is delivered"),
            "{writer}"
        );
        assert!(writer.contains("§Delivery state"), "{writer}");
    }

    /// Fault injection: a journal append writes part of its record and then
    /// fails. This establishes the rollback and the refusal that follow a
    /// reported append error; it does not reproduce a device's own failure.
    /// With a prune due, a close that compacted would append its `prune`
    /// line after whatever the failed append left; with none due, it would
    /// fold a journal this writer could not append to.
    #[test]
    fn a_failed_journal_append_is_cut_back_and_the_close_folds_nothing() {
        for prune_due in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let now = Utc::now();
            let age = Duration::days(if prune_due { RETAIN_DELIVERED_DAYS } else { 0 });
            let spool = Spool::open(home.path()).unwrap();
            deliver(&spool, one_event().1, now - age);
            let queued = spool.enqueue(&entry(one_event().1)).unwrap();
            let journal = file(home.path(), "outbound.delivery.journal");
            let queue = file(home.path(), "outbound.ndjson");

            spool
                .writer
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .fail_append_after = Some(10);
            let error = spool.claim(queued, now).unwrap_err();
            assert!(
                format!("{error:#}").contains("injected append failure"),
                "{error:#}"
            );
            assert!(
                file(home.path(), "outbound.delivery.journal") == journal,
                "the partial record is cut back"
            );
            assert_eq!(spool.delivery_states().unwrap()[&queued].attempts, 0);
            let error = spool.claim(queued, now).unwrap_err();
            assert!(
                format!("{error:#}").contains("journal append failed"),
                "{error:#}"
            );
            drop(spool);
            assert!(file(home.path(), "outbound.delivery.journal") == journal);
            assert!(file(home.path(), "outbound.ndjson") == queue);
            assert!(
                !home
                    .path()
                    .join("relay/spool/outbound.delivery.json")
                    .exists(),
                "the close folded nothing"
            );
            let reader = Spool::read_only(home.path()).snapshot().unwrap();
            assert!(
                reader
                    .close_error
                    .as_deref()
                    .is_some_and(|error| error.contains("journal append failed")),
                "{:?}",
                reader.close_error
            );

            // The next writer works from the intact journal and its close prunes.
            let spool = Spool::open(home.path()).unwrap();
            assert!(spool.claim(queued, now).unwrap());
            drop(spool);
            let reader = Spool::read_only(home.path()).snapshot().unwrap();
            assert_eq!(reader.pruned_delivered, Some(u64::from(prune_due)));
            assert_eq!(reader.states[&queued].attempts, 1);
            assert_eq!(reader.close_error, None);
        }
    }

    /// Replace one line of a spool file with its first half, newline kept.
    fn damage_line(home: &Path, name: &str, line: usize) {
        let text = String::from_utf8(file(home, name)).unwrap();
        let damaged: String = text
            .lines()
            .enumerate()
            .map(|(at, text)| {
                let kept = if at == line {
                    &text[..text.len() / 2]
                } else {
                    text
                };
                format!("{kept}\n")
            })
            .collect();
        std::fs::write(home.join("relay/spool").join(name), damaged).unwrap();
    }

    /// Damage before the last line of either file is an explicit error for
    /// the reader, the writer and status, and nothing is skipped, truncated
    /// or rewritten. A damaged line of a delivered batch is found when the
    /// spool is read, before any delivery or prune needs its payload.
    #[test]
    fn damage_in_the_middle_of_a_file_is_an_error_and_never_a_skipped_line() {
        for (name, line, expected) in [
            (
                "outbound.delivery.journal",
                1,
                "parse outbound.delivery.journal at byte",
            ),
            ("outbound.ndjson", 0, "parse spool line 0"),
            ("outbound.ndjson", 1, "parse spool line 1"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let now = Utc::now();
            let spool = Spool::open(home.path()).unwrap();
            deliver(&spool, one_event().1, now);
            let queued = spool.enqueue(&entry(one_event().1)).unwrap();
            spool.enqueue(&entry(one_event().1)).unwrap();
            assert!(spool.claim(queued, now).unwrap());
            abandon(spool);
            damage_line(home.path(), name, line);
            let damaged = file(home.path(), name);

            for error in [
                Spool::read_only(home.path()).snapshot().err().unwrap(),
                Spool::read_only(home.path())
                    .delivery_states()
                    .err()
                    .unwrap(),
                Spool::open(home.path()).err().unwrap(),
            ] {
                assert!(format!("{error:#}").contains(expected), "{name}: {error:#}");
            }
            let status = crate::state::egress_report(home.path());
            assert!(
                status["unavailable"]
                    .as_str()
                    .is_some_and(|error| error.contains(expected)),
                "{name}: {status}"
            );
            for count in ["queued", "dead", "delivered_batches", "pending"] {
                assert!(status[count].is_null(), "{name}: {count} is unknown");
            }
            assert!(
                file(home.path(), name) == damaged,
                "{name} is left as found"
            );
        }
    }

    /// In a pruned queue a line that has lost the start of its `index` member
    /// takes its line number as its index and fails the order check before
    /// it is parsed. That error names the contract's remedies as the parse
    /// error does.
    #[test]
    fn a_damaged_line_reported_as_out_of_order_names_the_remedy() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("relay/spool");
        std::fs::create_dir_all(&dir).unwrap();
        let mut queue = stored_line(5, &entry(one_event().1)).unwrap();
        let damaged = stored_line(6, &entry(one_event().1)).unwrap();
        queue.push_str(&damaged[damaged.len() / 2..]);
        std::fs::write(dir.join("outbound.ndjson"), queue).unwrap();
        let error = format!("{:#}", Spool::open(home.path()).err().unwrap());
        assert!(
            error.contains("spool line 1 is out of order; nothing is delivered until the line")
                && error.contains("§Delivery state"),
            "{error}"
        );
    }

    /// A close that cannot fold the journal leaves the reason where status
    /// reads it, and the next clean close removes it. The cause here is an
    /// unreadable `session-agents.json`, which the pin exemption consults.
    #[test]
    fn a_failed_close_is_shown_by_status_until_a_close_succeeds() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let pins = home.path().join("relay/session-agents.json");
        let spool = Spool::open(home.path()).unwrap();
        let delivered = deliver(
            &spool,
            one_event().1,
            now - Duration::days(RETAIN_DELIVERED_DAYS),
        );
        std::fs::write(&pins, "not json").unwrap();
        drop(spool);

        let snapshot = Spool::read_only(home.path()).snapshot().unwrap();
        assert_eq!(
            snapshot.states[&delivered].status,
            DeliveryStatus::Delivered,
            "the journal still states the acceptance"
        );
        assert!(journal_lines(home.path()).len() > 1, "nothing was folded");
        assert!(
            snapshot
                .close_error
                .as_deref()
                .is_some_and(|error| error.contains("session-agents.json")),
            "{:?}",
            snapshot.close_error
        );
        let status = crate::state::egress_report(home.path());
        assert_eq!(status["close_error"], json!(snapshot.close_error));
        assert_eq!(status["delivered_batches"], 1);
        assert!(status["unavailable"].is_null());
        assert!(crate::state::egress_text(&status).contains("did not fold the delivery journal"));

        std::fs::remove_file(&pins).unwrap();
        drop(Spool::open(home.path()).unwrap());
        assert_eq!(journal_lines(home.path()).len(), 1, "header only");
        let status = crate::state::egress_report(home.path());
        assert!(status["close_error"].is_null());
        assert!(!crate::state::egress_text(&status).contains("did not fold"));
    }

    /// `outbound.close-error` is advisory: whatever it holds, the relay still
    /// reads its pending batches and status still counts. Bytes that are not
    /// UTF-8 are shown lossily, control characters never reach the status
    /// text, a large file is cut to `CLOSE_ERROR_LIMIT`, a path that does not
    /// read as a file is reported apart from a failed close, and an empty
    /// file is no reason.
    #[test]
    fn a_close_error_file_that_does_not_read_stops_neither_delivery_nor_status() {
        let home = tempfile::tempdir().unwrap();
        let spool = Spool::open(home.path()).unwrap();
        deliver(&spool, one_event().1, Utc::now());
        let queued = spool.enqueue(&entry(one_event().1)).unwrap();
        drop(spool);
        let path = home.path().join("relay/spool/outbound.close-error");

        std::fs::write(&path, b"fold failed at /home/\xff\xfe/relay\n").unwrap();
        let spool = Spool::open(home.path()).unwrap();
        let pending: Vec<u64> = spool
            .pending()
            .unwrap()
            .iter()
            .map(|(index, _)| *index)
            .collect();
        assert_eq!(pending, [queued]);
        abandon(spool);
        let status = crate::state::egress_report(home.path());
        assert_eq!(status["queued"], 1);
        assert_eq!(status["delivered_batches"], 1);
        assert!(status["unavailable"].is_null());
        assert_eq!(
            status["close_error"],
            "fold failed at /home/\u{fffd}\u{fffd}/relay"
        );

        std::fs::write(&path, b"fold \x1b[31mfailed\x07\r\nsecond line\n").unwrap();
        let status = crate::state::egress_report(home.path());
        assert_eq!(status["close_error"], "fold  [31mfailed   second line");
        assert!(
            !crate::state::egress_text(&status)
                .contains(|character: char| { character.is_control() && character != '\n' }),
            "{status}"
        );

        std::fs::write(&path, "x".repeat(3 * CLOSE_ERROR_LIMIT as usize)).unwrap();
        let reason = Spool::read_only(home.path())
            .snapshot()
            .unwrap()
            .close_error
            .unwrap();
        assert_eq!(reason.len() as u64, CLOSE_ERROR_LIMIT);

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let snapshot = Spool::read_only(home.path()).snapshot().unwrap();
        assert_eq!(snapshot.undelivered.len(), 1);
        assert_eq!(snapshot.close_error, None, "no failed close is claimed");
        assert!(snapshot.close_error_unreadable.is_some());
        let status = crate::state::egress_report(home.path());
        assert!(status["close_error"].is_null());
        assert_eq!(
            status["close_error_unreadable"],
            json!(snapshot.close_error_unreadable)
        );
        assert_eq!(status["queued"], 1);
        let text = crate::state::egress_text(&status);
        assert!(text.contains("outbound.close-error did not read"), "{text}");
        assert!(!text.contains("did not fold"), "{text}");

        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "\n").unwrap();
        let snapshot = Spool::read_only(home.path()).snapshot().unwrap();
        assert_eq!(snapshot.close_error, None);
        assert_eq!(snapshot.close_error_unreadable, None);
    }

    /// The remedy the contract gives for a damaged line of a delivered
    /// batch: a placeholder that keeps the index. The spool then opens,
    /// delivers what is queued, closes cleanly and prunes the placeholder
    /// under the retention rule.
    #[test]
    fn a_placeholder_with_the_same_index_repairs_a_damaged_delivered_line() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let spool = Spool::open(home.path()).unwrap();
        let delivered = deliver(
            &spool,
            one_event().1,
            now - Duration::days(RETAIN_DELIVERED_DAYS),
        );
        let (id, document) = one_event();
        let queued = spool.enqueue(&entry(document)).unwrap();
        abandon(spool);
        damage_line(home.path(), "outbound.ndjson", delivered as usize);
        assert!(Spool::open(home.path()).is_err());

        let path = home.path().join("relay/spool/outbound.ndjson");
        let queue = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        let repaired: Vec<String> = queue
            .lines()
            .enumerate()
            .map(|(at, line)| {
                if at == delivered as usize {
                    json!({"index": delivered, "origin": "damaged line replaced",
                        "document": {"events": []}})
                    .to_string()
                } else {
                    line.to_owned()
                }
            })
            .collect();
        std::fs::write(&path, repaired.join("\n") + "\n").unwrap();

        let spool = Spool::open(home.path()).unwrap();
        assert_eq!(spool.pending().unwrap().len(), 1);
        assert!(spool.claim(queued, now).unwrap());
        spool.record_acceptance(queued, &[id], 0).unwrap();
        drop(spool);
        let snapshot = Spool::read_only(home.path()).snapshot().unwrap();
        assert_eq!(snapshot.close_error, None);
        assert_eq!(snapshot.pruned_delivered, Some(1));
        assert_eq!(
            snapshot.states.keys().copied().collect::<Vec<_>>(),
            [queued]
        );
    }

    /// Released batches an exemption keeps do not count towards the
    /// threshold: beside eight of them, one prunable batch is under one in
    /// eight and the queue file is left alone.
    #[test]
    fn the_prune_threshold_counts_only_batches_no_exemption_keeps() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS);
        let pinned = Uuid::new_v4();
        let unpinned = Uuid::new_v4();
        let state = crate::state::RelayState::open(home.path()).unwrap();
        state.pin_session_agent(pinned, "commonmeasure").unwrap();
        let session_batch = |session: Uuid| {
            json!({"session_id": session, "agent_id": "commonmeasure",
                "events": [{"id": Uuid::new_v4()}]})
        };
        let spool = Spool::open(home.path()).unwrap();
        for _ in 0..8 {
            deliver(&spool, session_batch(unpinned), long_ago);
        }
        deliver(&spool, session_batch(pinned), long_ago);
        let before = file(home.path(), "outbound.ndjson");
        spool.compact(now).unwrap();
        assert!(file(home.path(), "outbound.ndjson") == before);
        assert_eq!(spool.snapshot().unwrap().pruned_delivered, Some(0));

        state.pin_session_agent(unpinned, "commonmeasure").unwrap();
        spool.compact(now).unwrap();
        assert_eq!(spool.snapshot().unwrap().pruned_delivered, Some(9));
        assert!(file(home.path(), "outbound.ndjson").is_empty());
    }

    /// A batch delivered to a receiver that did not issue its `instance`
    /// members is released by age only. Beyond the retained count it stays,
    /// beside a batch that records no withheld member, until the period ends.
    #[test]
    fn a_batch_delivered_without_its_instance_members_waits_for_the_period() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let spool = Spool::open(home.path()).unwrap();
        let (id, document) = one_event();
        let index = spool.enqueue(&entry(document)).unwrap();
        assert!(spool.claim(index, now).unwrap());
        spool.record_acceptance(index, &[id], 2).unwrap();
        assert_eq!(
            journal_lines(home.path()).last().unwrap()["state"]["state"]["instance_references_withheld"],
            2
        );
        abandon(spool);

        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("relay/spool");
        std::fs::create_dir_all(&dir).unwrap();
        let total = RETAIN_DELIVERED_BATCHES as u64 + 200;
        let withheld = [0, 5];
        let mut queue = String::new();
        let mut states = serde_json::Map::new();
        for index in 0..total {
            queue.push_str(&json!({"origin": "fixture", "document": one_event().1}).to_string());
            queue.push('\n');
            let mut state = json!({"status": "delivered", "attempts": 1, "next_attempt_at": null,
                "last_attempt_at": now - Duration::hours(1), "last_error": null});
            if withheld.contains(&index) {
                state["instance_references_withheld"] = json!(2);
            } else if index == 1 {
                state["instance_references_withheld"] = json!(0);
            }
            states.insert(index.to_string(), state);
        }
        std::fs::write(dir.join("outbound.ndjson"), queue).unwrap();
        std::fs::write(
            dir.join("outbound.delivery.json"),
            serde_json::to_vec(&states).unwrap(),
        )
        .unwrap();

        let spool = Spool::open(home.path()).unwrap();
        spool.compact(now).unwrap();
        let snapshot = spool.snapshot().unwrap();
        assert_eq!(snapshot.pruned_delivered, Some(198));
        let kept: Vec<u64> = snapshot.states.keys().copied().collect();
        assert_eq!(kept[..3], [0, 5, 200]);
        assert_eq!(snapshot.states[&5].instance_references_withheld, Some(2));

        spool
            .compact(now + Duration::days(RETAIN_DELIVERED_DAYS))
            .unwrap();
        let snapshot = spool.snapshot().unwrap();
        assert_eq!(snapshot.pruned_delivered, Some(total));
        assert!(snapshot.states.is_empty());
    }

    /// The journal alone counts pruned deliveries. Once it is missing beside
    /// a queue that shows pruning, the count and the delivered batch total
    /// are unknown, not the retained part of them, and stay unknown.
    #[test]
    fn the_pruned_count_is_unknown_once_its_journal_is_missing() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS);
        let spool = Spool::open(home.path()).unwrap();
        deliver(&spool, one_event().1, long_ago);
        deliver(&spool, one_event().1, long_ago);
        let queued = spool.enqueue(&entry(one_event().1)).unwrap();
        drop(spool);
        let reader = Spool::read_only(home.path());
        assert_eq!(reader.snapshot().unwrap().pruned_delivered, Some(2));
        assert_eq!(
            crate::state::egress_report(home.path())["delivered_batches"],
            2
        );

        std::fs::remove_file(home.path().join("relay/spool/outbound.delivery.journal")).unwrap();
        assert_eq!(reader.snapshot().unwrap().pruned_delivered, None);
        let status = crate::state::egress_report(home.path());
        assert!(status["delivered_batches"].is_null());
        assert_eq!(status["queued"], 1);
        assert!(status["unavailable"].is_null());
        assert!(crate::state::egress_text(&status).contains("unknown delivered"));

        let spool = Spool::open(home.path()).unwrap();
        assert!(spool.claim(queued, now).unwrap());
        drop(spool);
        assert!(journal_lines(home.path())[0]["header"]["pruned_delivered"].is_null());
        assert_eq!(reader.snapshot().unwrap().pruned_delivered, None);
    }

    /// Two prunes leave no gap in the queue's indices: one that removes
    /// every batch, and one that removes only the newest. With the journal
    /// missing, the marker shows both. A spool that has never enqueued has
    /// no marker and reads zero.
    #[test]
    fn a_prune_that_leaves_no_gap_still_makes_a_missing_journal_unknown() {
        let now = Utc::now();
        let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS);
        let fresh = tempfile::tempdir().unwrap();
        assert_eq!(
            Spool::read_only(fresh.path())
                .snapshot()
                .unwrap()
                .pruned_delivered,
            Some(0)
        );

        let emptied = tempfile::tempdir().unwrap();
        let spool = Spool::open(emptied.path()).unwrap();
        deliver(&spool, one_event().1, long_ago);
        drop(spool);
        assert!(file(emptied.path(), "outbound.ndjson").is_empty());
        let dir = emptied.path().join("relay/spool");
        std::fs::remove_file(dir.join("outbound.delivery.journal")).unwrap();
        assert!(dir.join("outbound.pruned").exists());
        let reader = Spool::read_only(emptied.path());
        assert_eq!(reader.snapshot().unwrap().pruned_delivered, None);
        assert!(crate::state::egress_report(emptied.path())["delivered_batches"].is_null());

        let newest = tempfile::tempdir().unwrap();
        let spool = Spool::open(newest.path()).unwrap();
        let queued = spool.enqueue(&entry(one_event().1)).unwrap();
        deliver(&spool, one_event().1, long_ago);
        deliver(&spool, one_event().1, long_ago);
        drop(spool);
        let reader = Spool::read_only(newest.path());
        let snapshot = reader.snapshot().unwrap();
        assert_eq!(snapshot.pruned_delivered, Some(2));
        assert_eq!(
            snapshot.states.keys().copied().collect::<Vec<_>>(),
            [queued]
        );
        std::fs::remove_file(newest.path().join("relay/spool/outbound.delivery.journal")).unwrap();
        assert_eq!(reader.snapshot().unwrap().pruned_delivered, None);
        let status = crate::state::egress_report(newest.path());
        assert!(status["delivered_batches"].is_null());
        assert_eq!(status["queued"], 1);
    }

    /// Fault injection: the marker cannot be written (a directory holds its
    /// temporary name). The prune stops there with the queue whole, at close
    /// and again when the next open completes the announced prune, so no
    /// queue ever loses a batch without the marker beside it. Writing the
    /// marker after the queue rewrite fails this test.
    #[test]
    fn a_prune_that_cannot_leave_its_marker_removes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let long_ago = now - Duration::days(RETAIN_DELIVERED_DAYS + 1);
        let spool = Spool::open(home.path()).unwrap();
        for _ in 0..3 {
            deliver(&spool, one_event().1, long_ago);
        }
        let dir = home.path().join("relay/spool");
        std::fs::create_dir(dir.join("outbound.pruned.tmp")).unwrap();
        let error = spool.compact(now).unwrap_err();
        assert!(
            format!("{error:#}").contains("write outbound.pruned"),
            "{error:#}"
        );
        abandon(spool);
        let lines = |home: &Path| file(home, "outbound.ndjson").split(|b| *b == b'\n').count() - 1;
        assert_eq!(lines(home.path()), 3, "the queue is whole");
        assert!(!dir.join("outbound.pruned").exists());

        let error = Spool::open(home.path()).err().unwrap();
        assert!(
            format!("{error:#}").contains("write outbound.pruned"),
            "{error:#}"
        );
        assert_eq!(lines(home.path()), 3, "the queue is whole");

        std::fs::remove_dir(dir.join("outbound.pruned.tmp")).unwrap();
        drop(Spool::open(home.path()).unwrap());
        assert!(dir.join("outbound.pruned").exists());
        assert_eq!(lines(home.path()), 0);
        assert_eq!(
            Spool::read_only(home.path())
                .snapshot()
                .unwrap()
                .pruned_delivered,
            Some(3)
        );
    }

    /// Rewrite the queue as every release to v0.3.3 wrote it: no `index`
    /// member, so a batch's index is its line number.
    fn make_positional(home: &Path) {
        let queue = String::from_utf8(file(home, "outbound.ndjson")).unwrap();
        let positional: String = queue
            .lines()
            .map(|line| {
                let at = line.find(',').unwrap();
                assert!(line.starts_with("{\"index\":"), "{line}");
                format!("{{{}\n", &line[at + 1..])
            })
            .collect();
        std::fs::write(home.join("relay/spool/outbound.ndjson"), positional).unwrap();
    }

    /// The contract's remedy for a damaged line of an undelivered batch, in
    /// a positional queue left by a killed relay, so the journal still holds
    /// the batch's records: the line is emptied with its newline kept, and
    /// the batch's journal records are removed (the snapshot never had it).
    /// Every later batch keeps its index and state. Deleting the line
    /// instead is shown to move them.
    #[test]
    fn emptying_a_damaged_undelivered_line_keeps_every_later_batch_in_place() {
        let home = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let spool = Spool::open(home.path()).unwrap();
        deliver(&spool, one_event().1, now);
        let damaged = spool.enqueue(&entry(one_event().1)).unwrap();
        assert!(spool.claim(damaged, now).unwrap());
        spool
            .record_failure(damaged, "receiver answered 503")
            .unwrap();
        let later_delivered = deliver(&spool, one_event().1, now);
        let (claimed_id, claimed_document) = one_event();
        let claimed = spool.enqueue(&entry(claimed_document)).unwrap();
        assert!(spool.claim(claimed, now).unwrap());
        let (unclaimed_id, unclaimed_document) = one_event();
        let unclaimed = spool.enqueue(&entry(unclaimed_document)).unwrap();
        abandon(spool);
        make_positional(home.path());
        let states = |home: &Path| {
            let mut states = Spool::read_only(home).delivery_states().unwrap();
            states.remove(&damaged);
            serde_json::to_value(states).unwrap()
        };
        let before = states(home.path());
        damage_line(home.path(), "outbound.ndjson", damaged as usize);
        let error = format!("{:#}", Spool::open(home.path()).err().unwrap());
        assert!(
            error.contains("parse spool line 1; nothing is delivered"),
            "{error}"
        );

        let dir = home.path().join("relay/spool");
        let queue = String::from_utf8(file(home.path(), "outbound.ndjson")).unwrap();
        let journal = String::from_utf8(file(home.path(), "outbound.delivery.journal")).unwrap();
        let without_records: String = journal
            .lines()
            .filter(|line| {
                let record: Value = serde_json::from_str(line).unwrap();
                record["state"]["index"] != json!(damaged)
            })
            .map(|line| format!("{line}\n"))
            .collect();
        assert_ne!(
            without_records, journal,
            "the journal held the batch's records"
        );
        std::fs::write(dir.join("outbound.delivery.journal"), &without_records).unwrap();

        // What the contract forbids: with the line deleted, each later line
        // reads under the index, and so the state, of the batch before it.
        let deleted: String = queue
            .lines()
            .enumerate()
            .filter(|(at, _)| *at != damaged as usize)
            .map(|(_, line)| format!("{line}\n"))
            .collect();
        std::fs::write(dir.join("outbound.ndjson"), deleted).unwrap();
        let reader = Spool::read_only(home.path());
        let (shifted, _) = reader
            .entries()
            .unwrap()
            .into_iter()
            .find(|(_, entry)| entry.document["events"][0]["id"] == json!(claimed_id))
            .unwrap();
        assert_eq!(shifted, later_delivered);
        assert_eq!(
            reader.delivery_states().unwrap()[&shifted].status,
            DeliveryStatus::Delivered,
            "with the line deleted a queued batch reads as delivered, and nothing errors"
        );

        let emptied: String = queue
            .lines()
            .enumerate()
            .map(|(at, line)| {
                if at == damaged as usize {
                    "\n".to_owned()
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        std::fs::write(dir.join("outbound.ndjson"), emptied).unwrap();
        assert_eq!(states(home.path()), before);
        let spool = Spool::open(home.path()).unwrap();
        assert_eq!(
            spool
                .pending()
                .unwrap()
                .iter()
                .map(|(index, entry)| (*index, entry.document["events"][0]["id"].clone()))
                .collect::<Vec<_>>(),
            [
                (claimed, json!(claimed_id)),
                (unclaimed, json!(unclaimed_id))
            ]
        );
        assert_eq!(
            spool.enqueue(&entry(one_event().1)).unwrap(),
            unclaimed + 1,
            "the emptied line still counts towards the next index"
        );
        drop(spool);
        let mut after = states(home.path());
        after
            .as_object_mut()
            .unwrap()
            .remove(&(unclaimed + 1).to_string());
        assert_eq!(after, before, "a clean close changes no other batch");
    }

    #[test]
    fn acceptance_retains_whether_this_attempt_delivered_a_subset() {
        let home = tempfile::tempdir().unwrap();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let spool = Spool::open(home.path()).unwrap();
        let entry = SpoolEntry {
            origin: "test".into(),
            queued_at: Some(Utc::now()),
            directory_selection: true,
            document: json!({"events": [{"id": first}, {"id": second}]}),
        };
        for accepted in [vec![first], vec![first, second]] {
            let index = spool.enqueue(&entry).unwrap();
            assert!(spool.claim(index, Utc::now()).unwrap());
            spool.relay_state.record_delivered(&accepted).unwrap();
            spool.record_acceptance(index, &accepted, 0).unwrap();
            let state = &Spool::read_only(home.path()).delivery_states().unwrap()[&index];
            assert_eq!(state.status, DeliveryStatus::Delivered);
            assert_eq!(state.delivered_subset, Some(accepted.len() == 1));
            assert_eq!(
                spool.entries().unwrap()[index as usize].1.document,
                entry.document
            );
        }
        // Other deliveries of an event must not make a partial attempt look whole.
        let index = spool.enqueue(&entry).unwrap();
        spool.claim(index, Utc::now()).unwrap();
        spool.record_acceptance(index, &[first], 0).unwrap();
        assert_eq!(
            spool.delivery_states().unwrap()[&index].delivered_subset,
            Some(true)
        );
    }

    #[test]
    fn older_delivery_metadata_has_unknown_subset() {
        let state: DeliveryState = serde_json::from_value(json!({
            "status": "delivered", "attempts": 1, "next_attempt_at": null,
            "last_attempt_at": null, "last_error": null
        }))
        .unwrap();
        assert_eq!(state.delivered_subset, None);
        assert!(
            serde_json::to_value(state)
                .unwrap()
                .get("delivered_subset")
                .is_none()
        );
    }
}
