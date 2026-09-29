//! How a log in the evidence format is read, and how a torn line is told
//! from a line a reader must not skip.
//!
//! One scan serves every reader: the strict read, the relay's read for
//! delivery, and the reporting ruling's check that the relay would read the
//! log. They differ only in what a record is parsed into and in what they do
//! about a final line that ends without its newline ([`TornTail`]), so a
//! ruling cannot pass a log the relay then skips
//! (`docs/contracts/session-evidence.md` §Reading a log).
//!
//! Every append holds an exclusive advisory lock on the log (`flock(2)` on
//! macOS and Linux) from reading the tail to the end of its write
//! ([`super::EvidenceLog::append`]). A final line without its newline, seen
//! while holding the lock, therefore belongs to no writer still writing: the
//! kernel releases a lock when the process holding it exits, however it
//! exits, and a writer that is still alive releases it only after its write
//! has returned. Such a line was cut short, and the next append terminates
//! it and writes an `evidence_gap` record naming its length. A reader skips
//! a line that does not parse only where that record is the next line.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize as _;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How long a writer, or a reader waiting for a write in progress, waits
/// for another process to release the append lock. An append holds it for
/// one write and one fsync; a process stopped while holding it (`SIGSTOP`,
/// a debugger) would otherwise hang every writer of the session, and the
/// host's hooks with them.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The payload member of an `evidence_gap` record that marks the line
/// before it as torn, holding that line's length in bytes without its
/// newline.
pub const TORN_LINE_BYTES: &str = "torn_line_bytes";

/// Where a scan of a log stands: the byte offset just past a line and that
/// line's number, counting from one. The default is the start of the log.
/// A reader resuming at an offset whose line number it did not keep passes
/// `line: 0`, and a fault then names the line by its byte offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogPosition {
    pub offset: u64,
    pub line: u64,
}

/// A line cut short by a write that did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tear {
    /// The line's number, counting from one.
    pub line: u64,
    /// Its length in bytes, without a newline.
    pub bytes: u64,
    /// Whether the `evidence_gap` record that marks it follows it. A tear
    /// not yet marked is the log's final line, which the next append or the
    /// relay's next read for delivery marks ([`TornTail::Forecast`]).
    pub marked: bool,
}

/// What a scan does about a final line that ends without its newline.
///
/// Each mode first waits for a writer still writing: it takes the append
/// lock and reads the line again, and a line a live writer was writing is
/// then whole. The modes differ over a line that is still unterminated
/// once the lock is held, which no process is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TornTail {
    /// The log does not read. Nothing is written.
    Refuse,
    /// Read the log as it will read once the tear is marked, and write
    /// nothing: the line is reported as an unmarked [`Tear`], and a line
    /// that parses is kept as the record it is. The reporting ruling and a
    /// forecast relay run read this way, because a relay run for delivery
    /// marks the tear before it reads ([`TornTail::Repair`]).
    Forecast,
    /// Mark the tear as an append would: terminate the line and write its
    /// `evidence_gap` record, then read on. The relay reads for delivery
    /// this way, so a session whose last writer died mid-append is
    /// delivered whether or not anything appends to it again.
    Repair,
}

/// What a scan established.
#[derive(Debug, Default)]
pub struct Scanned {
    /// Just past the last line the scan settled: a record, a blank line, or
    /// a torn line with the record that marks it. A reader that reads a log
    /// in parts resumes here.
    pub end: LogPosition,
    /// The torn lines skipped, in order, and an unmarked final one under
    /// [`TornTail::Forecast`].
    pub tears: Vec<Tear>,
    /// Why the log does not read past [`Self::end`], if it does not. The
    /// records before it were passed to the caller.
    pub fault: Option<io::Error>,
}

/// A line that did not parse, held until the next line says whether it
/// was torn.
struct Held {
    line: u64,
    name: String,
    bytes: u64,
    error: serde_json::Error,
}

impl Held {
    fn fault(&self, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} does not parse ({}), and {why}", self.name, self.error),
        )
    }
}

/// How a fault names a line: by number where the scan knows it, otherwise
/// by the byte offset it starts at.
fn line_name(numbered: bool, line: u64, start: u64) -> String {
    if numbered {
        format!("line {line}")
    } else {
        format!("the line at byte {start}")
    }
}

/// Scan the log at `path` from `from`, passing each record, parsed as `T`,
/// with the offset its line starts at, to `emit`.
///
/// `T` is [`Value`] for a reader that keeps the records and [`Validated`]
/// for one that only needs to know they parse; both go through the same
/// `serde_json` entry point, so they accept exactly the same lines.
///
/// A line that does not parse is skipped only where the next line is an
/// `evidence_gap` record whose `torn_line_bytes` is that line's length. Any
/// other line that does not parse ends the scan with a fault naming it.
pub fn scan<T: DeserializeOwned>(
    path: &Path,
    from: LogPosition,
    tail: TornTail,
    mut emit: impl FnMut(T, u64),
) -> io::Result<Scanned> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from.offset))?;
    let mut reader = BufReader::new(file);
    let mut scanned = Scanned {
        end: from,
        ..Scanned::default()
    };
    // Just past the last line consumed, which may be held.
    let mut cursor = from;
    let mut held: Option<Held> = None;
    let numbered = from.offset == 0 || from.line > 0;
    // Held once a final line without its newline has been seen, for the
    // rest of the scan: no writer can append while it is held, so what is
    // read under it is final.
    let mut shared_lock = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            let number = cursor.line + 1;
            if shared_lock {
                // No process is writing this line.
                match tail {
                    TornTail::Refuse => {
                        scanned.fault = Some(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{} ends without its newline and no process is writing it: a \
                                 write was cut short; the next append to the log, or the next \
                                 relay run, marks it with an evidence_gap record and readers \
                                 then skip it",
                                line_name(numbered, number, cursor.offset)
                            ),
                        ));
                    }
                    TornTail::Forecast => {
                        if let Some(held) = held.take() {
                            scanned.fault =
                                Some(held.fault("the next line is not the record of a torn line"));
                        } else {
                            // Marking a whole record only adds its newline.
                            if let Ok(record) = serde_json::from_slice::<T>(&line) {
                                emit(record, cursor.offset);
                            }
                            scanned.tears.push(Tear {
                                line: number,
                                bytes: line.len() as u64,
                                marked: false,
                            });
                        }
                    }
                    TornTail::Repair => unreachable!("a repairing scan takes no shared lock"),
                }
                return Ok(scanned);
            }
            match tail {
                TornTail::Refuse | TornTail::Forecast => {
                    lock_within(reader.get_ref(), Lock::Shared, path)?;
                    shared_lock = true;
                }
                // Marks the line if no process is writing it, and waits
                // for the process that is writing it otherwise.
                TornTail::Repair => {
                    super::repair_torn_tail(path)?;
                }
            }
            reader.seek(SeekFrom::Start(cursor.offset))?;
            continue;
        }
        cursor.line += 1;
        let start = cursor.offset;
        cursor.offset += read as u64;
        let content = &line[..line.len() - 1];
        if content.iter().all(u8::is_ascii_whitespace) {
            if let Some(held) = held.take() {
                scanned.fault = Some(held.fault("the next line is blank"));
                return Ok(scanned);
            }
            scanned.end = cursor;
            continue;
        }
        match serde_json::from_slice::<T>(content) {
            Ok(record) => {
                if let Some(torn) = held.take() {
                    if torn_line_gap(content) != Some(torn.bytes) {
                        scanned.fault = Some(torn.fault(
                            "the next line is not the evidence_gap record of a torn line of \
                             that length",
                        ));
                        return Ok(scanned);
                    }
                    scanned.tears.push(Tear {
                        line: torn.line,
                        bytes: torn.bytes,
                        marked: true,
                    });
                }
                emit(record, start);
                scanned.end = cursor;
            }
            Err(error) => {
                if let Some(earlier) = held.take() {
                    scanned.fault = Some(earlier.fault("the next line does not parse either"));
                    return Ok(scanned);
                }
                held = Some(Held {
                    line: cursor.line,
                    name: line_name(numbered, cursor.line, start),
                    bytes: content.len() as u64,
                    error,
                });
            }
        }
    }
    if let Some(held) = held {
        scanned.fault = Some(held.fault("no evidence_gap record of a torn line follows it"));
    }
    Ok(scanned)
}

/// The `torn_line_bytes` of a line that is an `evidence_gap` record marking
/// the line before it as torn. Only a line naming the member is parsed, and
/// such records are short.
fn torn_line_gap(line: &[u8]) -> Option<u64> {
    let named = TORN_LINE_BYTES.as_bytes();
    if !line.windows(named.len()).any(|window| window == named) {
        return None;
    }
    let record: Value = serde_json::from_slice(line).ok()?;
    if record["event"] != "evidence_gap" {
        return None;
    }
    record["payload"][TORN_LINE_BYTES].as_u64()
}

/// A JSON value parsed as [`Value`] parses one, and not kept.
///
/// `serde::de::IgnoredAny` would be cheaper, but `serde_json` skips a string
/// it is told to ignore without checking its UTF-8 or its surrogate
/// escapes, so it accepts lines a [`Value`] parse refuses; the ruling would
/// then pass a log the relay skips. This visitor asks for every string and
/// key as `deserialize_any` does for [`Value`] and drops it, so memory is
/// that of one line.
#[derive(Debug, Clone, Copy)]
pub struct Validated;

impl<'de> serde::Deserialize<'de> for Validated {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValidatedVisitor)
    }
}

struct ValidatedVisitor;

impl<'de> serde::de::Visitor<'de> for ValidatedVisitor {
    type Value = Validated;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_i128<E>(self, _: i128) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_u128<E>(self, _: u128) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_str<E>(self, _: &str) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_unit<E>(self) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_none<E>(self) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Validated, D::Error> {
        Validated::deserialize(deserializer)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Validated, A::Error> {
        while seq.next_element::<Validated>()?.is_some() {}
        Ok(Validated)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Validated, A::Error> {
        while map.next_key::<Validated>()?.is_some() {
            map.next_value::<Validated>()?;
        }
        Ok(Validated)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Lock {
    Shared,
    Exclusive,
}

/// Take the append lock on `file` within [`LOCK_WAIT`].
pub(super) fn lock_within(file: &File, lock: Lock, path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        let attempt = match lock {
            Lock::Shared => file.try_lock_shared(),
            Lock::Exclusive => file.try_lock(),
        };
        match attempt {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "another process held the append lock on {} for more than {} seconds",
                        path.display(),
                        LOCK_WAIT.as_secs()
                    ),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// The length of the final line of `file` if it ends without its newline.
/// The caller holds the append lock, so no process is writing it.
pub(super) fn torn_tail(file: &mut File) -> io::Result<Option<u64>> {
    let length = file.metadata()?.len();
    if length == 0 {
        return Ok(None);
    }
    let mut last = [0u8];
    file.seek(SeekFrom::Start(length - 1))?;
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(None);
    }
    // Walk back to the newline before it. A record is a few kilobytes, so
    // this reads little.
    let mut end = length;
    let mut chunk = vec![0u8; 8192];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let size = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk[..size])?;
        if let Some(at) = chunk[..size].iter().rposition(|byte| *byte == b'\n') {
            return Ok(Some(length - (start + at as u64 + 1)));
        }
        end = start;
    }
    Ok(Some(length))
}

/// Open `path` for an append that may repair its tail: reading, to find a
/// torn tail, and appending.
pub(super) fn open_for_append(path: &Path, create: bool) -> io::Result<File> {
    OpenOptions::new()
        .create(create)
        .read(true)
        .append(true)
        .open(path)
}
