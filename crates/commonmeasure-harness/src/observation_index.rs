//! The validation index for host observations: the facts
//! `SessionLog::record_host_observation` checks a request against, derived
//! from the session log a few lines at a time so that a request reads only
//! what was appended since the last one.
//!
//! The session log is the evidence and this is derived from it. The index
//! holds handles, generation and output identifiers, hashes and byte offsets
//! into the log: nothing the log does not already hold, and no URL or text.
//! It is kept in memory by the log's writer and in a journal under the
//! operator home, `observation-index/<session>.ndjson`, so a restarted
//! server resumes where the last one stopped.
//!
//! The journal is not authenticated. Each entry names the byte range of the
//! log it covers and a digest of the log's bytes just before the end of that
//! range. A journal that is missing, malformed, of another version,
//! discontinuous or about a different log is discarded and the index is
//! rebuilt from the log. A journal whose range and digest match the log is
//! believed whole: its facts are not compared with the records they came
//! from, and the digest is a plain SHA-256 that anyone who can read the log
//! can compute, so a well-formed entry with the right range and digest is
//! admitted whatever facts it lists. This is the log's own trust limit and
//! no new boundary. The log is unauthenticated NDJSON beside the journal
//! under the same user, and whoever can write the journal can append the
//! same claim to the log. Both rest on the operator home's file permissions;
//! the journal and its directory are created owner-only. One difference
//! remains: a forged log line stays in the evidence, and a forged journal
//! fact leaves no record of itself.
//!
//! A journal that cannot be written costs the next process a rebuild and
//! changes no answer: validation runs from the in-memory index, which came
//! from the log. The journal is not fsynced for the same reason; a torn tail
//! is a malformed journal. A journal grows by one line per request that read
//! anything and is replaced by one line when a process loads more than
//! [`COMPACT_AFTER`] of them.
//!
//! Reading the log is strict, as it was when every request read the whole
//! file: a malformed line, or a last line with no newline, makes the request
//! unavailable, and the index does not advance past it. What the index cannot
//! notice is damage to bytes it has already read, beyond the digest's window.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

/// How many bytes before the covered offset the digest spans. Records carry
/// timestamps and random identifiers, so two logs agreeing here and differing
/// earlier is not something an append-only writer produces.
const TAIL: u64 = 4096;

/// A journal loaded with more entries than this is written whole again by the
/// request that follows, so a restart parses at most this many lines plus
/// those of one process's lifetime.
pub(super) const COMPACT_AFTER: usize = 1024;

/// Whether a request can ask about this identifier. Requests carry UUIDs and
/// are compared in the hyphenated lowercase form; a host's own turn
/// identifier is any string and matches none.
fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value)
}

/// One thing a log record established, in the form the journal stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case", deny_unknown_fields)]
enum Fact {
    /// A mediated crossing or a refusal exists, so opt-in is too late.
    Crossing,
    /// A `crossing_mediated` record issued this handle.
    Acquisition {
        id: String,
        content_hash: Option<String>,
    },
    ContextEntry {
        acquisition: String,
        generation: String,
        representation_hash: Option<String>,
    },
    /// An `output_associated` record starts at byte `at` of the log. The
    /// record is read from there when a retry needs its payload.
    Output {
        generation: Option<String>,
        output: Option<String>,
        at: u64,
    },
    Completed {
        turn: String,
    },
    Action {
        id: String,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Facts {
    crossing: bool,
    acquisitions: HashMap<String, Vec<String>>,
    entries: HashMap<(String, String), Vec<String>>,
    outputs_by_generation: HashMap<String, u64>,
    outputs_by_id: HashMap<String, u64>,
    completed: HashSet<String>,
    actions: HashSet<String>,
}

impl Facts {
    /// Apply one fact; false when it was already known or is a turn no
    /// request can name, which is then neither held nor journalled. Every arm
    /// is idempotent, because two writers can journal the same range.
    fn apply(&mut self, fact: &Fact) -> bool {
        fn add(known: &mut Vec<String>, value: &Option<String>) -> bool {
            match value {
                Some(value) if !known.contains(value) => {
                    known.push(value.clone());
                    true
                }
                _ => false,
            }
        }
        fn earliest(known: &mut HashMap<String, u64>, key: &Option<String>, at: u64) -> bool {
            let Some(key) = key else { return false };
            match known.get_mut(key) {
                Some(existing) if *existing <= at => false,
                Some(existing) => {
                    *existing = at;
                    true
                }
                None => {
                    known.insert(key.clone(), at);
                    true
                }
            }
        }
        match fact {
            Fact::Crossing => !std::mem::replace(&mut self.crossing, true),
            Fact::Acquisition { id, content_hash } => {
                let new = !self.acquisitions.contains_key(id);
                add(
                    self.acquisitions.entry(id.clone()).or_default(),
                    content_hash,
                ) || new
            }
            Fact::ContextEntry {
                acquisition,
                generation,
                representation_hash,
            } => {
                let key = (acquisition.clone(), generation.clone());
                let new = !self.entries.contains_key(&key);
                add(self.entries.entry(key).or_default(), representation_hash) || new
            }
            Fact::Output {
                generation,
                output,
                at,
            } => {
                let by_generation = earliest(&mut self.outputs_by_generation, generation, *at);
                earliest(&mut self.outputs_by_id, output, *at) || by_generation
            }
            // Hook-written boundaries carry the host's turn identifier, a free
            // string. Only a generation's UUID is ever asked about.
            Fact::Completed { turn } => canonical_uuid(turn) && self.completed.insert(turn.clone()),
            Fact::Action { id } => self.actions.insert(id.clone()),
        }
    }

    /// Index the record that starts at byte `at`, adding what it established
    /// to `new`. Identifiers are compared as the strings the log holds, which
    /// for a record CM wrote is the hyphenated form a request's UUID prints as.
    fn observe(&mut self, record: &Value, at: u64, new: &mut Vec<Fact>) {
        let payload = &record["payload"];
        let text = |key: &str| payload[key].as_str().map(str::to_owned);
        let mut facts = Vec::new();
        match record["event"].as_str() {
            Some("crossing_mediated") => {
                facts.push(Fact::Crossing);
                if let Some(id) = text("acquisition_id") {
                    facts.push(Fact::Acquisition {
                        id,
                        content_hash: text("content_hash"),
                    });
                }
            }
            Some("crossing_refused") => facts.push(Fact::Crossing),
            Some("context_entered") => {
                if let (Some(acquisition), Some(generation)) =
                    (text("acquisition_id"), text("generation_id"))
                {
                    facts.push(Fact::ContextEntry {
                        acquisition,
                        generation,
                        representation_hash: text("representation_hash"),
                    });
                }
            }
            Some("output_associated") => {
                let (generation, output) = (text("generation_id"), text("output_id"));
                if generation.is_some() || output.is_some() {
                    facts.push(Fact::Output {
                        generation,
                        output,
                        at,
                    });
                }
            }
            Some("turn_completed") => {
                facts.extend(text("turn_id").map(|turn| Fact::Completed { turn }))
            }
            Some("action_decided") => facts.extend(text("action_id").map(|id| Fact::Action { id })),
            _ => {}
        }
        new.extend(facts.into_iter().filter(|fact| self.apply(fact)));
    }

    /// Everything known, for a journal written whole.
    fn all(&self) -> Vec<Fact> {
        let mut facts = Vec::new();
        if self.crossing {
            facts.push(Fact::Crossing);
        }
        let each = |hashes: &[String]| -> Vec<Option<String>> {
            if hashes.is_empty() {
                vec![None]
            } else {
                hashes.iter().cloned().map(Some).collect()
            }
        };
        for (id, hashes) in &self.acquisitions {
            facts.extend(
                each(hashes)
                    .into_iter()
                    .map(|content_hash| Fact::Acquisition {
                        id: id.clone(),
                        content_hash,
                    }),
            );
        }
        for ((acquisition, generation), hashes) in &self.entries {
            facts.extend(
                each(hashes)
                    .into_iter()
                    .map(|representation_hash| Fact::ContextEntry {
                        acquisition: acquisition.clone(),
                        generation: generation.clone(),
                        representation_hash,
                    }),
            );
        }
        for (generation, at) in &self.outputs_by_generation {
            facts.push(Fact::Output {
                generation: Some(generation.clone()),
                output: None,
                at: *at,
            });
        }
        for (output, at) in &self.outputs_by_id {
            facts.push(Fact::Output {
                generation: None,
                output: Some(output.clone()),
                at: *at,
            });
        }
        facts.extend(
            self.completed
                .iter()
                .cloned()
                .map(|turn| Fact::Completed { turn }),
        );
        facts.extend(self.actions.iter().cloned().map(|id| Fact::Action { id }));
        facts
    }
}

/// One journal line: what the log's bytes `from..through` established.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    v: u32,
    from: u64,
    through: u64,
    /// SHA-256 of the log's last [`TAIL`] bytes before `through`.
    tail: String,
    facts: Vec<Fact>,
}

pub(super) struct ValidationIndex {
    log: PathBuf,
    journal: PathBuf,
    /// Every complete line of the log before this byte is indexed.
    offset: u64,
    tail: String,
    facts: Facts,
    /// The journal on disk does not hold this index, so the next write
    /// replaces it whole and does not append.
    rewrite: bool,
}

fn tail_digest(log: &mut File, offset: u64) -> io::Result<String> {
    let start = offset.saturating_sub(TAIL);
    log.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (offset - start) as usize];
    log.read_exact(&mut bytes)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// The journal's directory, readable by its owner alone, as is every journal
/// in it. The journal is believed when it matches the log (see the module
/// comment), so it gets no wider access than it needs.
fn create_owner_only_directory(directory: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(directory)
}

fn create_owner_only(file: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(file)
}

impl ValidationIndex {
    /// The index for one session's log, from its journal where that is
    /// usable and empty otherwise. Whether it describes this log is checked
    /// by [`Self::catch_up`], against the log.
    pub(super) fn load(home: &Path, session_id: &str, log: &Path) -> Self {
        let journal = home
            .join("observation-index")
            .join(format!("{session_id}.ndjson"));
        let mut index = Self {
            log: log.to_owned(),
            journal,
            offset: 0,
            tail: String::new(),
            facts: Facts::default(),
            rewrite: true,
        };
        index.reset();
        index.remove_staged(session_id);
        if let Some((offset, tail, facts, entries)) = Self::read_journal(&index.journal) {
            index.offset = offset;
            index.tail = tail;
            index.facts = facts;
            index.rewrite = entries > COMPACT_AFTER;
        }
        index
    }

    /// Remove what a failed or interrupted replacement of this session's
    /// journal left behind: `<session>.<pid>-<uuid>.tmp`. A live writer whose
    /// staged file goes this way fails its rename and writes the journal
    /// again at its next request.
    fn remove_staged(&self, session_id: &str) {
        let Some(directory) = self.journal.parent() else {
            return;
        };
        let staged = |name: &str| {
            name.strip_prefix(session_id)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.strip_suffix(".tmp"))
                .and_then(|rest| rest.split_once('-'))
                .is_some_and(|(process, id)| {
                    !process.is_empty()
                        && process.bytes().all(|byte| byte.is_ascii_digit())
                        && canonical_uuid(id)
                })
        };
        for file in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            if file.file_name().to_str().is_some_and(staged) {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    /// The journal's account and how many entries gave it, or nothing if any
    /// line of it is not an entry that continues the ones before it.
    fn read_journal(journal: &Path) -> Option<(u64, String, Facts, usize)> {
        let mut reader = BufReader::new(File::open(journal).ok()?);
        let (mut offset, mut tail, mut facts) = (0, None, Facts::default());
        let mut entries = 0;
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line).ok()? == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                return None;
            }
            let entry: Entry = serde_json::from_slice(&line).ok()?;
            // Two writers may cover the same range; none may skip one.
            if entry.v != VERSION || entry.from > offset || entry.through < entry.from {
                return None;
            }
            for fact in &entry.facts {
                facts.apply(fact);
            }
            if entry.through > offset || tail.is_none() {
                offset = entry.through.max(offset);
                tail = Some(entry.tail);
            }
            entries += 1;
        }
        Some((offset, tail?, facts, entries))
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.tail = format!("{:x}", Sha256::digest(b""));
        self.facts = Facts::default();
        self.rewrite = true;
    }

    /// Index what the log gained since the last call and return how many
    /// lines that was. An index that does not describe this log is rebuilt
    /// from the log's first byte. An error is a log that cannot be read as
    /// evidence: missing, unreadable, holding a malformed line, or ending in
    /// an incomplete one. The lines before the fault stay indexed.
    pub(super) fn catch_up(&mut self) -> io::Result<u64> {
        let mut log = match File::open(&self.log) {
            Ok(log) => log,
            Err(error) => {
                self.reset();
                return Err(error);
            }
        };
        if log.metadata()?.len() < self.offset || tail_digest(&mut log, self.offset)? != self.tail {
            self.reset();
        }
        let from = self.offset;
        log.seek(SeekFrom::Start(from))?;
        let mut reader = BufReader::new(&mut log);
        let (mut lines, mut new, mut line) = (0, Vec::new(), Vec::new());
        let outcome = loop {
            line.clear();
            let read = match reader.read_until(b'\n', &mut line) {
                Ok(0) => break Ok(()),
                Ok(read) => read,
                Err(error) => break Err(error),
            };
            if !line.ends_with(b"\n") {
                break Err(io::Error::other(
                    "the session log ends in an incomplete record",
                ));
            }
            if !line.iter().all(u8::is_ascii_whitespace) {
                match serde_json::from_slice::<Value>(&line) {
                    Ok(record) => self.facts.observe(&record, self.offset, &mut new),
                    Err(error) => break Err(io::Error::other(error)),
                }
            }
            self.offset += read as u64;
            lines += 1;
        };
        drop(reader);
        if self.offset > from || self.rewrite {
            self.tail = tail_digest(&mut log, self.offset)?;
            let facts = if self.rewrite { self.facts.all() } else { new };
            let entry = Entry {
                v: VERSION,
                from: if self.rewrite { 0 } else { from },
                through: self.offset,
                tail: self.tail.clone(),
                facts,
            };
            // A journal that cannot be written is rebuilt by whoever reads it
            // next; this process's index is complete without it.
            self.rewrite = self.write_journal(&entry).is_err();
        }
        outcome.map(|()| lines)
    }

    fn write_journal(&self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        if !self.rewrite {
            return std::fs::OpenOptions::new()
                .append(true)
                .open(&self.journal)?
                .write_all(&line);
        }
        if let Some(directory) = self.journal.parent() {
            create_owner_only_directory(directory)?;
        }
        // Replaced by rename, so a reader sees the old journal or the new one.
        let staged = self.journal.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        create_owner_only(&staged)
            .and_then(|mut file| file.write_all(&line))
            .and_then(|()| std::fs::rename(&staged, &self.journal))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&staged);
            })
    }

    pub(super) fn crossing_recorded(&self) -> bool {
        self.facts.crossing
    }

    pub(super) fn admitted(&self, acquisition: &str) -> bool {
        self.facts.acquisitions.contains_key(acquisition)
    }

    pub(super) fn admitted_with(&self, acquisition: &str, content_hash: &str) -> bool {
        self.facts
            .acquisitions
            .get(acquisition)
            .is_some_and(|hashes| hashes.iter().any(|hash| hash == content_hash))
    }

    fn entry(&self, acquisition: &str, generation: &str) -> Option<&Vec<String>> {
        self.facts
            .entries
            .get(&(acquisition.to_owned(), generation.to_owned()))
    }

    pub(super) fn entered(&self, acquisition: &str, generation: &str) -> bool {
        self.entry(acquisition, generation).is_some()
    }

    pub(super) fn entered_as(&self, acquisition: &str, generation: &str, hash: &str) -> bool {
        self.entry(acquisition, generation)
            .is_some_and(|hashes| hashes.iter().any(|known| known == hash))
    }

    pub(super) fn generation_has_output(&self, generation: &str) -> bool {
        self.facts.outputs_by_generation.contains_key(generation)
    }

    pub(super) fn completed(&self, generation: &str) -> bool {
        self.facts.completed.contains(generation)
    }

    pub(super) fn action_recorded(&self, action: &str) -> bool {
        self.facts.actions.contains(action)
    }

    /// The payload of the first `output_associated` record for this
    /// generation or this output, read from the log at its indexed offset.
    /// An offset that does not hold such a record means the index is wrong:
    /// it is discarded, this request is unavailable and the next rebuilds.
    pub(super) fn first_output(
        &mut self,
        generation: &str,
        output: &str,
    ) -> io::Result<Option<Value>> {
        let at = [
            self.facts.outputs_by_generation.get(generation),
            self.facts.outputs_by_id.get(output),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(&at) = at else { return Ok(None) };
        let mut log = File::open(&self.log)?;
        log.seek(SeekFrom::Start(at))?;
        let mut line = Vec::new();
        BufReader::new(log).read_until(b'\n', &mut line)?;
        let payload = serde_json::from_slice::<Value>(&line)
            .ok()
            .filter(|record| record["event"] == "output_associated")
            .map(|mut record| record["payload"].take())
            .filter(|payload| {
                payload["generation_id"] == generation || payload["output_id"] == output
            });
        if payload.is_none() {
            self.reset();
            return Err(io::Error::other(
                "the validation index did not match the session log and was discarded",
            ));
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn append(log: &Path, record: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        file.write_all(format!("{record}\n").as_bytes()).unwrap();
    }

    fn entry(acquisition: &str, generation: &str) -> Value {
        json!({"event":"context_entered", "payload":{"acquisition_id":acquisition,
            "generation_id":generation, "representation_hash":"sha256:r"}})
    }

    /// A restart reads the journal and then only the lines the log gained,
    /// and the facts equal those of an index built from the whole log.
    #[test]
    fn a_restart_resumes_from_the_journal_and_reads_only_new_lines() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(
            &log,
            &json!({"event":"crossing_mediated",
            "payload":{"acquisition_id":"a", "content_hash":"sha256:c"}}),
        );
        for filler in 0..200 {
            append(
                &log,
                &json!({"event":"crossing_observed", "payload":{"n":filler}}),
            );
        }
        let mut first = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(first.catch_up().unwrap(), 201);
        assert_eq!(first.catch_up().unwrap(), 0);
        append(&log, &entry("a", "g"));
        assert_eq!(first.catch_up().unwrap(), 1);

        append(
            &log,
            &json!({"event":"action_decided", "payload":{"action_id":"x"}}),
        );
        let mut restarted = ValidationIndex::load(home.path(), "s", &log);
        assert!(!restarted.rewrite, "the journal was usable");
        assert!(restarted.admitted_with("a", "sha256:c") && restarted.entered("a", "g"));
        assert_eq!(restarted.catch_up().unwrap(), 1);
        assert!(restarted.action_recorded("x"));

        std::fs::remove_dir_all(home.path().join("observation-index")).unwrap();
        let mut rebuilt = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(rebuilt.catch_up().unwrap(), 203);
        assert_eq!(rebuilt.facts, restarted.facts);
    }

    /// A journal entry for the log's bytes `from..through` with the digest
    /// the log bears out, so the loader and `catch_up` can refuse it only for
    /// what else it says.
    fn forged(log: &Path, from: u64, through: u64, v: u32, facts: &[Fact]) -> String {
        let tail = tail_digest(&mut File::open(log).unwrap(), through).unwrap();
        format!(
            "{}\n",
            json!({"v":v, "from":from, "through":through, "tail":tail, "facts":facts})
        )
    }

    /// Two lines indexed and journalled as one entry, then a third line the
    /// journal does not cover. Returns the log, the journal's path and text,
    /// and the log's length before and after the third line.
    fn journalled_then_appended(home: &Path) -> (PathBuf, PathBuf, String, u64, u64) {
        let log = home.join("s.ndjson");
        append(&log, &json!({"event":"crossing_refused", "payload":{}}));
        append(&log, &entry("a", "g"));
        let mut index = ValidationIndex::load(home, "s", &log);
        index.catch_up().unwrap();
        let covered = std::fs::metadata(&log).unwrap().len();
        append(&log, &entry("a", "h"));
        let length = std::fs::metadata(&log).unwrap().len();
        let journal = std::fs::read_to_string(&index.journal).unwrap();
        (log, index.journal, journal, covered, length)
    }

    fn forged_acquisition() -> [Fact; 1] {
        [Fact::Acquisition {
            id: "forged".to_owned(),
            content_hash: Some("sha256:x".to_owned()),
        }]
    }

    /// The trust limit, pinned so that a change to it is deliberate: a
    /// journal entry of this version, continuous with the ones before it,
    /// whose range and digest match the log, is believed, and the handle it
    /// lists is admitted though no `crossing_mediated` record issued it. The
    /// log is unauthenticated and beside the journal under the same user, so
    /// whoever can write this entry can append that record instead.
    ///
    /// This is also the control for `an_unusable_journal_is_rebuilt_from_the_log`:
    /// the entries forged there differ from this one only in the fault named.
    #[test]
    fn a_journal_that_matches_the_log_is_believed_as_the_log_would_be() {
        let home = tempfile::tempdir().unwrap();
        let (log, journal, good, covered, length) = journalled_then_appended(home.path());
        let entry = forged(&log, covered, length, VERSION, &forged_acquisition());
        std::fs::write(&journal, format!("{good}{entry}")).unwrap();

        let mut index = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(index.catch_up().unwrap(), 0, "the journal covered the log");
        assert!(index.admitted_with("forged", "sha256:x"));
        assert!(!index.entered("a", "h"), "the third line was taken as read");
    }

    /// Each unusable journal is rebuilt from the log, replaced on disk, and
    /// ends with the facts of the log and nothing of what it held before.
    /// The forged entries carry the digest and a range the log bears out, so
    /// each is refused for the fault its case names and no other.
    #[test]
    fn an_unusable_journal_is_rebuilt_from_the_log() {
        let home = tempfile::tempdir().unwrap();
        let (log, journal, good, covered, length) = journalled_then_appended(home.path());
        let facts = forged_acquisition();
        let wrong_tail = |through: u64| {
            format!(
                "{}\n",
                json!({"v":VERSION, "from":0, "through":through, "tail":"0", "facts":facts})
            )
        };
        let unusable = [
            ("empty", String::new()),
            ("not JSON", "{broken}\n".to_owned()),
            (
                "torn last line",
                format!("{good}{}", &good[..good.len() / 2]),
            ),
            (
                "a complete last entry with no newline",
                format!(
                    "{good}{}",
                    forged(&log, covered, length, VERSION, &facts).trim_end()
                ),
            ),
            ("zero-filled tail", format!("{good}\0\0\0\0\n")),
            (
                "another version",
                format!(
                    "{good}{}",
                    forged(&log, covered, length, VERSION + 1, &facts)
                ),
            ),
            (
                "a skipped range",
                format!(
                    "{good}{}",
                    forged(&log, covered + 1, length, VERSION, &facts)
                ),
            ),
            (
                "a reversed range",
                format!("{good}{}", forged(&log, 5, 4, VERSION, &facts)),
            ),
            (
                "unknown member",
                good.replace("\"v\":", "\"extra\":1,\"v\":"),
            ),
            ("another log's tail", wrong_tail(length)),
            ("beyond the log", wrong_tail(length + 100)),
        ];
        for (case, content) in unusable {
            std::fs::write(&journal, content).unwrap();
            let mut index = ValidationIndex::load(home.path(), "s", &log);
            assert_eq!(index.catch_up().unwrap(), 3, "{case}");
            assert!(index.entered("a", "g") && index.entered("a", "h"), "{case}");
            assert!(!index.admitted("forged"), "{case}");

            let replaced = std::fs::read_to_string(&journal).unwrap();
            assert_eq!(replaced.lines().count(), 1, "{case}");
            let mut reloaded = ValidationIndex::load(home.path(), "s", &log);
            assert!(!reloaded.rewrite, "{case}");
            assert_eq!(reloaded.catch_up().unwrap(), 0, "{case}");
            assert_eq!(reloaded.facts, index.facts, "{case}");
        }
    }

    /// A last line that is complete JSON with no newline may still be half
    /// of a longer record. It is unavailable, here and after a restart, and
    /// indexed once when its newline lands.
    #[test]
    fn a_complete_record_with_no_newline_is_unavailable_until_the_newline_lands() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("a", "g"));
        let raw = |bytes: &[u8]| {
            let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            file.write_all(bytes).unwrap();
        };
        raw(entry("b", "g").to_string().as_bytes());

        let mut index = ValidationIndex::load(home.path(), "s", &log);
        let mut restarted = ValidationIndex::load(home.path(), "s", &log);
        for index in [&mut index, &mut restarted] {
            let error = index.catch_up().unwrap_err();
            assert!(error.to_string().contains("incomplete record"), "{error}");
            assert!(index.entered("a", "g") && !index.entered("b", "g"));
        }
        raw(b"\n");
        assert_eq!(index.catch_up().unwrap(), 1);
        assert_eq!(index.catch_up().unwrap(), 0);
        assert!(index.entered("b", "g"));
    }

    /// Two writers can both append an output for one generation. A retry is
    /// compared with the first of them, whichever order the facts arrive in
    /// and after the journal is written whole.
    #[test]
    fn a_duplicate_output_leaves_the_earliest_as_the_original() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        let output = |output: &str, hash: &str| {
            json!({"event":"output_associated",
            "payload":{"generation_id":"g", "output_id":output, "output_hash":hash}})
        };
        append(&log, &entry("a", "g"));
        append(&log, &output("first", "h1"));
        append(&log, &output("second", "h2"));
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        index.catch_up().unwrap();
        let mut reloaded = ValidationIndex::load(home.path(), "s", &log);
        reloaded.catch_up().unwrap();
        for index in [&mut index, &mut reloaded] {
            for (generation, output, original) in [
                ("g", "unknown", "h1"),
                ("g", "second", "h1"),
                ("unknown", "first", "h1"),
                ("unknown", "second", "h2"),
            ] {
                let payload = index.first_output(generation, output).unwrap().unwrap();
                assert_eq!(payload["output_hash"], original, "{generation} {output}");
            }
        }

        let at = |at: u64| Fact::Output {
            generation: Some("g".to_owned()),
            output: None,
            at,
        };
        for order in [[at(10), at(90)], [at(90), at(10)]] {
            let mut facts = Facts::default();
            order.iter().for_each(|fact| {
                facts.apply(fact);
            });
            assert_eq!(facts.outputs_by_generation["g"], 10);
        }
    }

    /// A hook-written boundary carries the host's turn identifier, any
    /// string. It is not indexed and not journalled, from the log or from a
    /// journal that lists it; a generation's UUID is.
    #[test]
    fn only_a_canonical_uuid_turn_is_indexed_or_journalled() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        let generation = uuid::Uuid::new_v4().to_string();
        let turns = [
            "prompt 7: text the host chose".to_owned(),
            generation.to_uppercase(),
            generation.replace('-', ""),
            generation.clone(),
        ];
        for turn in &turns {
            append(
                &log,
                &json!({"event":"turn_completed", "payload":{"turn_id":turn}}),
            );
        }
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(index.catch_up().unwrap(), 4);
        let length = std::fs::metadata(&log).unwrap().len();
        let listed: Vec<_> = turns
            .iter()
            .map(|turn| Fact::Completed { turn: turn.clone() })
            .collect();
        for round in ["from the log", "from a journal that lists them"] {
            assert!(index.completed(&generation), "{round}");
            assert_eq!(index.facts.completed.len(), 1, "{round}");
            let journal = std::fs::read_to_string(&index.journal).unwrap();
            assert!(journal.contains(&generation), "{round}");
            for other in &turns[..3] {
                assert!(!journal.contains(other.as_str()), "{round}: {other}");
            }
            std::fs::write(&index.journal, forged(&log, 0, length, VERSION, &listed)).unwrap();
            index = ValidationIndex::load(home.path(), "s", &log);
            index.rewrite = true;
            index.catch_up().unwrap();
        }
    }

    /// Past `COMPACT_AFTER` entries the journal is replaced by one line at
    /// the next request, with the same facts; at the limit it is left alone.
    #[test]
    fn a_long_journal_is_written_whole_again_on_load() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        for line in 0..=COMPACT_AFTER {
            append(&log, &entry("a", &line.to_string()));
            index.catch_up().unwrap();
            if line + 1 == COMPACT_AFTER {
                let mut at_limit = ValidationIndex::load(home.path(), "s", &log);
                assert!(!at_limit.rewrite);
                at_limit.catch_up().unwrap();
            }
        }
        let lines = |index: &ValidationIndex| {
            let journal = std::fs::read_to_string(&index.journal).unwrap();
            journal.lines().count()
        };
        assert_eq!(lines(&index), COMPACT_AFTER + 1);

        let mut restarted = ValidationIndex::load(home.path(), "s", &log);
        assert!(restarted.rewrite);
        assert_eq!(restarted.catch_up().unwrap(), 0);
        assert_eq!(lines(&restarted), 1);
        assert_eq!(restarted.facts, index.facts);
        let mut again = ValidationIndex::load(home.path(), "s", &log);
        assert!(!again.rewrite);
        assert_eq!(again.catch_up().unwrap(), 0);
        assert_eq!(again.facts, index.facts);
    }

    /// The log replaced by another under the same name, longer or shorter,
    /// while the writer holds an index of the first.
    #[test]
    fn a_replaced_log_discards_the_index_held_in_memory() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("first", "g"));
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        index.catch_up().unwrap();
        for (lines, acquisition, previous) in [(3, "longer", "first"), (1, "shorter", "longer")] {
            std::fs::remove_file(&log).unwrap();
            for generation in 0..lines {
                append(&log, &entry(acquisition, &generation.to_string()));
            }
            assert_eq!(index.catch_up().unwrap(), lines);
            assert!(index.entered(acquisition, "0"));
            assert!(!index.entered(previous, "g") && !index.entered(previous, "0"));
        }
        std::fs::remove_file(&log).unwrap();
        assert_eq!(
            index.catch_up().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(index.facts, Facts::default());
    }

    /// Two writers journal overlapping ranges in either order; a reader of
    /// the journal ends with the same facts as a reader of the log.
    #[test]
    fn two_writers_may_journal_the_same_range() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("a", "1"));
        let mut one = ValidationIndex::load(home.path(), "s", &log);
        one.catch_up().unwrap();
        let mut two = ValidationIndex::load(home.path(), "s", &log);
        append(&log, &entry("a", "2"));
        one.catch_up().unwrap();
        append(&log, &entry("a", "3"));
        assert_eq!(two.catch_up().unwrap(), 2);
        assert_eq!(one.catch_up().unwrap(), 1);
        append(
            &log,
            &json!({"event":"output_associated",
            "payload":{"generation_id":"3", "output_id":"o"}}),
        );
        two.catch_up().unwrap();

        let journal = std::fs::read_to_string(&one.journal).unwrap();
        assert_eq!(journal.lines().count(), 5);
        let mut reader = ValidationIndex::load(home.path(), "s", &log);
        assert!(!reader.rewrite);
        assert_eq!(reader.catch_up().unwrap(), 0);
        assert_eq!(reader.facts, two.facts);
        assert!(reader.first_output("3", "unknown").unwrap().is_some());
    }

    /// A journal that cannot be written changes no answer.
    #[test]
    fn an_unwritable_journal_leaves_validation_at_full_strength() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        std::fs::write(home.path().join("observation-index"), b"not a directory").unwrap();
        append(&log, &entry("a", "g"));
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(index.catch_up().unwrap(), 1);
        append(&log, &entry("b", "g"));
        assert_eq!(index.catch_up().unwrap(), 1);
        assert!(index.entered("a", "g") && index.entered("b", "g") && index.rewrite);
    }

    /// The journal and its directory are made for their owner alone, whatever
    /// the umask allows, when first written and when replaced.
    #[cfg(unix)]
    #[test]
    fn the_journal_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("a", "g"));
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for replacement in 0..2 {
            index.rewrite = true;
            index.catch_up().unwrap();
            assert_eq!(mode(&index.journal) & 0o077, 0, "{replacement}");
            assert_eq!(mode(index.journal.parent().unwrap()) & 0o077, 0);
        }
        append(&log, &entry("b", "g"));
        index.catch_up().unwrap();
        assert_eq!(mode(&index.journal) & 0o077, 0, "after an append");
    }

    /// A replacement that fails leaves no staged file, and one left by a
    /// process that died before its rename goes at the next load. Other
    /// sessions' files stay.
    #[test]
    fn a_staged_journal_does_not_outlive_a_failed_write_or_the_next_load() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("a", "g"));
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        let directory = index.journal.parent().unwrap().to_owned();
        // A directory where the journal belongs: the staged file is written
        // and the rename fails.
        std::fs::create_dir_all(&index.journal).unwrap();
        assert_eq!(index.catch_up().unwrap(), 1);
        assert!(index.rewrite && index.entered("a", "g"));
        let names = || {
            let mut names: Vec<_> = std::fs::read_dir(&directory)
                .unwrap()
                .map(|file| file.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(), ["s.ndjson"]);

        std::fs::remove_dir(&index.journal).unwrap();
        let id = uuid::Uuid::new_v4();
        let abandoned = format!("s.4242-{id}.tmp");
        let others = [
            format!("s.other.4242-{id}.tmp"),
            "s.notes.tmp".to_owned(),
            "t.ndjson".to_owned(),
        ];
        for name in others.iter().chain([&abandoned]) {
            std::fs::write(directory.join(name), b"{}").unwrap();
        }
        let mut next = ValidationIndex::load(home.path(), "s", &log);
        next.catch_up().unwrap();
        let mut expected = others.to_vec();
        expected.push("s.ndjson".to_owned());
        expected.sort();
        assert_eq!(names(), expected);
    }

    /// Two writers, each with its own index, appending to the log and
    /// journalling at once, restarting as they go so that replacements of a
    /// long journal race the other's appends. A lost or discarded entry may
    /// cost a rebuild; what a fresh load ends with is what the log holds.
    #[test]
    fn concurrent_writers_leave_a_journal_that_agrees_with_the_log() {
        const LINES: usize = 700;
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &json!({"event":"crossing_refused", "payload":{}}));
        let writers: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|writer| {
                let (home, log) = (home.path().to_owned(), log.clone());
                std::thread::spawn(move || {
                    let mut index = ValidationIndex::load(&home, "s", &log);
                    for line in 0..LINES {
                        if line % 50 == 49 {
                            index = ValidationIndex::load(&home, "s", &log);
                        }
                        append(&log, &entry(writer, &line.to_string()));
                        // The other writer's append may be part written.
                        let mut tries = 0;
                        while let Err(error) = index.catch_up() {
                            tries += 1;
                            assert!(tries < 100, "{error}");
                        }
                        assert!(index.entered(writer, &line.to_string()));
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        let mut loaded = ValidationIndex::load(home.path(), "s", &log);
        loaded.catch_up().unwrap();
        std::fs::remove_file(&loaded.journal).unwrap();
        let mut rebuilt = ValidationIndex::load(home.path(), "s", &log);
        assert_eq!(rebuilt.catch_up().unwrap(), 1 + 2 * LINES as u64);
        assert_eq!(rebuilt.facts.entries.len(), 2 * LINES);
        assert_eq!(loaded.facts, rebuilt.facts);
    }

    /// An offset that holds some other record: unavailable once, with the
    /// index discarded, and right on the next request.
    #[test]
    fn an_output_offset_the_log_does_not_bear_out_discards_the_index() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("s.ndjson");
        append(&log, &entry("a", "g"));
        append(
            &log,
            &json!({"event":"output_associated",
            "payload":{"generation_id":"g", "output_id":"o", "output_hash":"h"}}),
        );
        let mut index = ValidationIndex::load(home.path(), "s", &log);
        index.catch_up().unwrap();
        let journal = std::fs::read_to_string(&index.journal).unwrap();
        let at = index.facts.outputs_by_id["o"];
        std::fs::write(
            &index.journal,
            journal.replace(&format!("\"at\":{at}"), "\"at\":0"),
        )
        .unwrap();

        let mut tampered = ValidationIndex::load(home.path(), "s", &log);
        tampered.catch_up().unwrap();
        assert!(tampered.first_output("g", "o").is_err());
        tampered.catch_up().unwrap();
        let payload = tampered.first_output("g", "o").unwrap().unwrap();
        assert_eq!(payload["output_hash"], "h");
    }

    /// The index against the full parse it replaced. Random records, some
    /// with members missing, repeated or of the wrong type, and lines that
    /// are not objects or are blank, are appended while up to three indexes
    /// sharing one journal are loaded, dropped and brought up to date at
    /// random. After every step one of them must answer every question a
    /// request can ask exactly as the checks on `main` before the index did,
    /// which read the whole log; those checks are the closures below.
    #[test]
    fn the_index_answers_as_a_full_parse_of_the_log_does() {
        struct Random(u64);
        impl Random {
            fn below(&mut self, limit: usize) -> usize {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                (self.0 % limit as u64) as usize
            }
            fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
                from[self.below(from.len())]
            }
        }
        const IDS: [&str; 3] = [
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000003",
        ];
        const HASHES: [&str; 2] = ["sha256:1", "sha256:2"];
        const EVENTS: [&str; 8] = [
            r#""crossing_mediated""#,
            r#""crossing_refused""#,
            r#""context_entered""#,
            r#""output_associated""#,
            r#""turn_completed""#,
            r#""action_decided""#,
            r#""crossing_observed""#,
            "7",
        ];
        const MEMBERS: [&str; 7] = [
            "acquisition_id",
            "generation_id",
            "output_id",
            "turn_id",
            "action_id",
            "content_hash",
            "representation_hash",
        ];
        const OTHER_LINES: [&str; 5] = ["[1,2]", r#""text""#, "7", "  ", "{}"];

        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for round in 0..150 {
            let home = tempfile::tempdir().unwrap();
            let log = home.path().join("s.ndjson");
            std::fs::write(&log, b"").unwrap();
            let mut indexes: Vec<ValidationIndex> = Vec::new();
            for step in 0..40 {
                match random.below(10) {
                    0 if indexes.len() < 3 => {
                        indexes.push(ValidationIndex::load(home.path(), "s", &log));
                    }
                    1 if !indexes.is_empty() => {
                        indexes.swap_remove(random.below(indexes.len()));
                    }
                    2 => {
                        let line = random.pick(&OTHER_LINES);
                        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
                        file.write_all(format!("{line}\n").as_bytes()).unwrap();
                    }
                    _ => {
                        let members: Vec<_> = (0..random.below(5))
                            .map(|_| {
                                let member = random.pick(&MEMBERS);
                                let value = match (random.below(8), member.ends_with("_hash")) {
                                    (0, _) => "7".to_owned(),
                                    (1, _) => "null".to_owned(),
                                    (2, _) => format!("{:?}", IDS[0].to_uppercase()),
                                    (_, true) => format!("{:?}", random.pick(&HASHES)),
                                    (_, false) => format!("{:?}", random.pick(&IDS)),
                                };
                                format!("{member:?}:{value}")
                            })
                            .collect();
                        let line = format!(
                            "{{\"event\":{},\"payload\":{{{}}}}}\n",
                            random.pick(&EVENTS),
                            members.join(",")
                        );
                        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
                        file.write_all(line.as_bytes()).unwrap();
                    }
                }
                if indexes.is_empty() {
                    indexes.push(ValidationIndex::load(home.path(), "s", &log));
                }
                let chosen = random.below(indexes.len());
                let index = &mut indexes[chosen];
                index.catch_up().unwrap();

                let records: Vec<Value> = std::fs::read_to_string(&log)
                    .unwrap()
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                let any = |check: &dyn Fn(&Value, &Value) -> bool| {
                    records
                        .iter()
                        .any(|record| check(&record["event"], &record["payload"]))
                };
                let at = format!("round {round}, step {step}");
                assert_eq!(
                    index.crossing_recorded(),
                    any(&|event, _| matches!(
                        event.as_str(),
                        Some("crossing_mediated" | "crossing_refused")
                    )),
                    "{at}"
                );
                for one in IDS {
                    assert_eq!(
                        index.admitted(one),
                        any(&|event, payload| event == "crossing_mediated"
                            && payload["acquisition_id"] == one),
                        "{at}"
                    );
                    assert_eq!(
                        index.generation_has_output(one),
                        any(&|event, payload| event == "output_associated"
                            && payload["generation_id"] == one),
                        "{at}"
                    );
                    assert_eq!(
                        index.completed(one),
                        any(&|event, payload| event == "turn_completed"
                            && payload["turn_id"] == one),
                        "{at}"
                    );
                    assert_eq!(
                        index.action_recorded(one),
                        any(&|event, payload| event == "action_decided"
                            && payload["action_id"] == one),
                        "{at}"
                    );
                    for hash in HASHES {
                        assert_eq!(
                            index.admitted_with(one, hash),
                            any(&|event, payload| event == "crossing_mediated"
                                && payload["acquisition_id"] == one
                                && payload["content_hash"] == hash),
                            "{at}"
                        );
                    }
                    for two in IDS {
                        assert_eq!(
                            index.entered(one, two),
                            any(&|event, payload| event == "context_entered"
                                && payload["acquisition_id"] == one
                                && payload["generation_id"] == two),
                            "{at}"
                        );
                        for hash in HASHES {
                            assert_eq!(
                                index.entered_as(one, two, hash),
                                any(&|event, payload| event == "context_entered"
                                    && payload["acquisition_id"] == one
                                    && payload["generation_id"] == two
                                    && payload["representation_hash"] == hash),
                                "{at}"
                            );
                        }
                        let original = records
                            .iter()
                            .find(|record| {
                                record["event"] == "output_associated"
                                    && (record["payload"]["output_id"] == two
                                        || record["payload"]["generation_id"] == one)
                            })
                            .map(|record| record["payload"].clone());
                        assert_eq!(index.first_output(one, two).unwrap(), original, "{at}");
                    }
                }
            }
        }
    }
}
