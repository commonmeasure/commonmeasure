//! The SQLite index over the session evidence logs.
//!
//! One row per log line, verbatim, keyed by `(source_path, line)` — so
//! re-ingesting a file is idempotent by construction, the same discipline as
//! the receiver-side `INSERT ... ON CONFLICT DO NOTHING` this is raided from.
//! A file that shrank since its last ingest was rebuilt (the logs are
//! append-only by contract), so its rows are dropped and re-read rather than
//! left describing a file that no longer exists.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::attribution::{Attribution, UNATTRIBUTED};

pub struct Store {
    connection: Connection,
    path: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub new_records: usize,
    /// Files whose row count went backwards: the store was rebuilt, so their
    /// index rows were dropped and re-read.
    pub rebuilt_files: usize,
}

impl Store {
    /// Open (or create) the index database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open telemetry index {}", path.display()))?;
        // Before the batch, not inside it: setting the journal mode needs a
        // brief exclusive lock, so a second opener racing the first on a fresh
        // home has to be willing to wait for it rather than failing outright.
        connection.busy_timeout(std::time::Duration::from_millis(2000))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS files (
                 path  TEXT PRIMARY KEY,
                 lines INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS records (
                 source_path TEXT NOT NULL,
                 line        INTEGER NOT NULL,
                 session_id  TEXT NOT NULL,
                 event       TEXT NOT NULL,
                 record      TEXT NOT NULL,
                 PRIMARY KEY (source_path, line)
             );
             CREATE INDEX IF NOT EXISTS records_by_session ON records (session_id);",
        )?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    /// Bring the index up to date with every session log under `sessions`.
    ///
    /// Incremental: only lines beyond each file's watermark are read. A line
    /// that does not parse — for any reason, invalid JSON or invalid UTF-8
    /// alike — is kept with event `unreadable` rather than skipped, because an
    /// index that silently holds less than the log would make every count it
    /// serves an understatement.
    pub fn ingest_sessions(&mut self, sessions: &Path) -> Result<IngestReport> {
        let mut report = IngestReport::default();
        let logs: Vec<PathBuf> = match std::fs::read_dir(sessions) {
            Ok(entries) => {
                let mut logs: Vec<PathBuf> = entries
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "ndjson"))
                    .collect();
                logs.sort();
                logs
            }
            // A sessions directory that is not there holds no logs and the
            // index must follow them down. One we merely could not read is no
            // evidence that its logs are gone, so leave what we hold standing.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Ok(report),
        };

        let transaction = self.connection.transaction()?;
        let mut seen: HashSet<String> = HashSet::new();
        for log in &logs {
            let source_path = log.display().to_string();
            seen.insert(source_path.clone());
            let session_id = log
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown-session".to_owned());
            let watermark: i64 = transaction
                .query_row(
                    "SELECT lines FROM files WHERE path = ?1",
                    params![source_path],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let file = std::fs::File::open(log)
                .with_context(|| format!("open session log {}", log.display()))?;
            let mut reader = std::io::BufReader::new(file);
            let mut buffer = Vec::new();
            let mut line_number = 0i64;
            let mut watermark = watermark;
            let mut checked_rebuild = false;
            loop {
                buffer.clear();
                let read = reader
                    .read_until(b'\n', &mut buffer)
                    .with_context(|| format!("read session log {}", log.display()))?;
                if read == 0 {
                    break;
                }
                let Some(bytes) = buffer.strip_suffix(b"\n") else {
                    // A last line with no newline is a write still in flight.
                    // Stopping short of it holds the watermark below the line
                    // so the next pass reads it whole; indexing the fragment
                    // would strand the truncation in the index forever, since
                    // the watermark is durable and nothing revisits it.
                    break;
                };
                line_number += 1;
                // Decoded lossily on purpose: a line that is not UTF-8 is
                // unparseable like any other unparseable line, and failing the
                // batch over it would cost every valid line beside it, in this
                // file and in every other.
                let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                let line = String::from_utf8_lossy(bytes).into_owned();
                // Detect a rebuilt store lazily: if the watermark exceeds the
                // file's length we only learn that at EOF, so compare against
                // the first line instead — cheap, and a rebuilt file differs
                // there in practice because session ids and timestamps do.
                if !checked_rebuild {
                    checked_rebuild = true;
                    if watermark > 0 {
                        let held: Option<String> = transaction
                            .query_row(
                                "SELECT record FROM records WHERE source_path = ?1 AND line = 1",
                                params![source_path],
                                |row| row.get(0),
                            )
                            .ok();
                        if held.is_some_and(|held| held != line) {
                            transaction.execute(
                                "DELETE FROM records WHERE source_path = ?1",
                                params![source_path],
                            )?;
                            watermark = 0;
                            report.rebuilt_files += 1;
                        }
                    }
                }
                if line_number <= watermark {
                    continue;
                }
                if line.trim().is_empty() {
                    continue;
                }
                let event = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|record| record["event"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unreadable".to_owned());
                let inserted = transaction.execute(
                    "INSERT OR IGNORE INTO records
                         (source_path, line, session_id, event, record)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![source_path, line_number, session_id, event, line],
                )?;
                report.new_records += inserted;
            }
            if watermark > line_number {
                // The file shrank and even its first line matched (or it is
                // now empty): drop what the index holds beyond it.
                transaction.execute(
                    "DELETE FROM records WHERE source_path = ?1 AND line > ?2",
                    params![source_path, line_number],
                )?;
                report.rebuilt_files += 1;
            }
            transaction.execute(
                "INSERT INTO files (path, lines) VALUES (?1, ?2)
                 ON CONFLICT (path) DO UPDATE SET lines = excluded.lines",
                params![source_path, line_number],
            )?;
        }
        // Rows for a log that is no longer on disk describe evidence nobody
        // can re-read, so they leave with it rather than propping up totals
        // whose source is gone.
        let stale: Vec<String> = {
            let mut statement = transaction.prepare("SELECT path FROM files")?;
            let held = statement.query_map([], |row| row.get::<_, String>(0))?;
            held.filter_map(std::result::Result::ok)
                .filter(|path| !seen.contains(path))
                .collect()
        };
        for path in stale {
            transaction.execute("DELETE FROM records WHERE source_path = ?1", params![path])?;
            transaction.execute("DELETE FROM files WHERE path = ?1", params![path])?;
        }
        transaction.commit()?;
        Ok(report)
    }

    /// Every session, with its evidence-grade counts kept apart, each read
    /// under the engagement the attribution rules resolve for it. `engagement`
    /// filters to one; `unattributed` is a name like any other.
    pub fn sessions(&self, attribution: &Attribution, engagement: Option<&str>) -> Result<Value> {
        let mut list = self.session_rollups()?;
        // Most recent activity first, unknown-dated sessions last.
        list.sort_by_key(|entry| std::cmp::Reverse(entry.1.last));
        Ok(Value::Array(
            list.into_iter()
                .filter_map(|(id, rollup)| {
                    let session_engagement = rollup.engagement(attribution);
                    engagement
                        .is_none_or(|wanted| wanted == session_engagement)
                        .then(|| rollup.into_json(&id, &session_engagement))
                })
                .collect(),
        ))
    }

    fn session_rollups(&self) -> Result<Vec<(String, SessionRollup)>> {
        let rows = self.all_records()?;
        let mut sessions: HashMap<String, SessionRollup> = HashMap::new();
        for (session_id, event, record) in &rows {
            let rollup = sessions.entry(session_id.clone()).or_default();
            rollup.absorb(event, record);
        }
        Ok(sessions.into_iter().collect())
    }

    /// Every record of one session, in log order, or `None` for a session the
    /// index has never seen.
    pub fn session_records(&self, session_id: &str) -> Result<Option<Value>> {
        let mut statement = self.connection.prepare(
            "SELECT record FROM records WHERE session_id = ?1 ORDER BY source_path, line",
        )?;
        let records: Vec<Value> = statement
            .query_map(params![session_id], |row| row.get::<_, String>(0))?
            .filter_map(std::result::Result::ok)
            .map(|line| {
                serde_json::from_str(&line)
                    .unwrap_or_else(|_| json!({"event": "unreadable", "raw": line}))
            })
            .collect();
        Ok((!records.is_empty()).then_some(Value::Array(records)))
    }

    /// The content view: crossings folded by URL, trailing slashes folded so
    /// `/x` and `/x/` are one row. Witnessed and reconstructed stay separate
    /// columns; a single total would claim this runtime saw traffic a
    /// transcript merely reports.
    ///
    /// A crossing the capture path stamped `internal` is a row here like any
    /// other, marked `"internal": true` rather than left out: it is part of
    /// the record, and a content list without it would be short by exactly
    /// the crossings the operator consented to record. Its context footprint
    /// is not here; [`Self::internal_use`] is the one view that lists it.
    pub fn content(
        &self,
        limit: usize,
        attribution: &Attribution,
        engagement: Option<&str>,
    ) -> Result<Value> {
        let rows = self.all_records()?;
        let mut by_url: HashMap<String, ContentRollup> = HashMap::new();
        for (session_id, event, record) in &rows {
            if !event.starts_with("crossing_") {
                continue;
            }
            let payload = &record["payload"];
            let Some(url) = payload["url"].as_str() else {
                continue;
            };
            // Attributed per crossing, not per session: a session that crossed
            // worktrees contributes each crossing to the engagement its own
            // cwd resolves to.
            let crossing_engagement = attribution.engagement_of(payload["cwd"].as_str());
            if engagement.is_some_and(|wanted| wanted != crossing_engagement) {
                continue;
            }
            let folded = url.trim_end_matches('/').to_owned();
            let rollup = by_url.entry(folded).or_default();
            rollup.absorb(session_id, event, payload, crossing_engagement);
        }
        let mut list: Vec<(String, ContentRollup)> = by_url.into_iter().collect();
        list.sort_by(|a, b| {
            (b.1.witnessed + b.1.reconstructed).cmp(&(a.1.witnessed + a.1.reconstructed))
        });
        list.truncate(limit);
        Ok(Value::Array(
            list.into_iter()
                .map(|(url, rollup)| rollup.into_json(&url))
                .collect(),
        ))
    }

    /// The internal-use view: crossings the capture path stamped `internal`,
    /// folded by URL with the same trailing-slash fold as [`Self::content`].
    ///
    /// The stamp is evidence, not configuration: a row here says this URL
    /// matched a `record_internal_prefixes` entry *when it was crossed*, so a
    /// prefix edited or removed since still leaves its recorded rows standing.
    /// Which prefixes are declared right now is the policy projection's answer,
    /// not this store's.
    ///
    /// Beside the grade counts, each row carries the context footprint in the
    /// basis its own crossings recorded, with the crossings that admitted
    /// bytes and recorded no estimate counted beside the figure — the same
    /// lower-bound discipline the session rollup holds — and the first and
    /// last time the path was crossed, which is the "over time" half of the
    /// question this view answers.
    pub fn internal_use(
        &self,
        limit: usize,
        attribution: &Attribution,
        engagement: Option<&str>,
    ) -> Result<Value> {
        let rows = self.all_records()?;
        let mut by_url: HashMap<String, InternalRollup> = HashMap::new();
        for (session_id, event, record) in &rows {
            if !event.starts_with("crossing_") {
                continue;
            }
            let payload = &record["payload"];
            if payload["internal"] != Value::Bool(true) {
                continue;
            }
            let Some(url) = payload["url"].as_str() else {
                continue;
            };
            // Attributed per crossing, like the content view: a session that
            // crossed worktrees contributes each crossing to the engagement
            // its own cwd resolves to.
            let crossing_engagement = attribution.engagement_of(payload["cwd"].as_str());
            if engagement.is_some_and(|wanted| wanted != crossing_engagement) {
                continue;
            }
            let folded = url.trim_end_matches('/').to_owned();
            let rollup = by_url.entry(folded).or_default();
            rollup.absorb(session_id, event, payload, crossing_engagement);
        }
        let mut list: Vec<(String, InternalRollup)> = by_url.into_iter().collect();
        list.sort_by(|a, b| {
            (b.1.witnessed + b.1.reconstructed).cmp(&(a.1.witnessed + a.1.reconstructed))
        });
        list.truncate(limit);
        Ok(Value::Array(
            list.into_iter()
                .map(|(url, rollup)| rollup.into_json(&url))
                .collect(),
        ))
    }

    /// What the index holds and what leaves the machine. The egress block is
    /// the relay's own durable account (`commonmeasure_relay::egress_report`), passed in
    /// by the caller who knows the home; this store indexes evidence and must
    /// not invent claims about delivery.
    pub fn status(
        &self,
        egress: Value,
        attribution: &Attribution,
        engagement: Option<&str>,
    ) -> Result<Value> {
        let (files, records): (i64, i64) = self.connection.query_row(
            "SELECT (SELECT COUNT(*) FROM files), (SELECT COUNT(*) FROM records)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let unreadable: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM records WHERE event = 'unreadable'",
            [],
            |row| row.get(0),
        )?;
        // The engagement summary, resolved fresh from the rules like every
        // other aggregate. The store totals above stay whole-store: one operator
        // record, and the filter narrows a projection, never the record.
        // Grades stay apart here as everywhere: witnessed (observed and
        // mediated), reconstructed and refused are different strengths of
        // claim, and a summary that totalled them would manufacture a count
        // no single grade supports.
        let mut engagements: HashMap<String, (u64, u64, u64, u64)> = HashMap::new();
        for (_, rollup) in self.session_rollups()? {
            let entry = engagements
                .entry(rollup.engagement(attribution))
                .or_default();
            entry.0 += 1;
            entry.1 += rollup.observed + rollup.mediated;
            entry.2 += rollup.reconstructed;
            entry.3 += rollup.refused;
        }
        let mut engagements: Vec<(String, (u64, u64, u64, u64))> = engagements
            .into_iter()
            .filter(|(name, _)| engagement.is_none_or(|wanted| wanted == name))
            .collect();
        engagements.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(json!({
            "database": self.path.display().to_string(),
            "files": files,
            "records": records,
            "unreadable_lines": unreadable,
            "engagements": engagements
                .into_iter()
                .map(|(name, (sessions, witnessed, reconstructed, refused))| {
                    json!({
                        "engagement": name,
                        "sessions": sessions,
                        "witnessed": witnessed,
                        "reconstructed": reconstructed,
                        "refused": refused,
                    })
                })
                .collect::<Vec<_>>(),
            "egress": egress,
            // Installed processors are a fact about the serving binary, not
            // about this index, so the serving layer adds them
            // (`serve.rs`); the store reports only what it holds.
        }))
    }

    /// The recorded working directories, as facts: every distinct cwd with
    /// how many records carried it, and how many sessions recorded no cwd at
    /// all. What a directory *means* — which engagement reads it, which
    /// policy scope governs it — is resolved by the callers that project
    /// declarations over these facts, so the store never has an opinion two
    /// projections could disagree with.
    pub fn cwd_facts(&self) -> Result<Value> {
        let mut cwds: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        let mut sessions_without_cwd = 0u64;
        for (_, rollup) in self.session_rollups()? {
            if rollup.cwds.is_empty() {
                sessions_without_cwd += 1;
            }
            for (cwd, seen) in &rollup.cwds {
                *cwds.entry(cwd.clone()).or_default() += seen;
            }
        }
        Ok(json!({
            "cwds": cwds
                .into_iter()
                .map(|(cwd, records)| json!({"cwd": cwd, "records": records}))
                .collect::<Vec<_>>(),
            "sessions_without_cwd": sessions_without_cwd,
        }))
    }

    /// Every recorded crossing as the facts a policy forecast needs: what
    /// was crossed, from where, by which principal, under which licence, and
    /// at which grade. In log order, one entry per `crossing_*` record, with
    /// a refused crossing carried as its own grade rather than folded into
    /// either of the others. A crossing record with no `url` cannot be
    /// judged and is counted, so a forecast can say what it left out.
    ///
    /// Facts only. Which scope governs each directory, and what a draft
    /// policy would have done, is the runtime loader's answer and is resolved
    /// above this store (`console/forecast.rs`).
    pub fn crossing_facts(&self) -> Result<CrossingFacts> {
        let mut facts = Vec::new();
        let mut not_evaluated = 0;
        for (_, event, record) in self.all_records()? {
            let grade = match grade_of(&event) {
                Some(Grade::Witnessed) => "witnessed",
                Some(Grade::Reconstructed) => "reconstructed",
                Some(Grade::Refused) => "refused",
                None => continue,
            };
            let payload = &record["payload"];
            let Some(url) = payload["url"].as_str() else {
                not_evaluated += 1;
                continue;
            };
            facts.push(CrossingFact {
                grade,
                url: url.to_owned(),
                host: payload["host_name"].as_str().unwrap_or_default().to_owned(),
                cwd: payload["cwd"].as_str().map(str::to_owned),
                principal: payload["principal"].as_str().map(str::to_owned),
                // A licence the record does not carry, or carries in a shape
                // this runtime does not define, is unknown: the forecast must
                // not credit a source with a licence nobody declared.
                licence: serde_json::from_value(payload["licence"].clone())
                    .unwrap_or(commonmeasure_types::LicenceState::Unknown),
            });
        }
        Ok(CrossingFacts {
            facts,
            not_evaluated,
        })
    }

    /// Where one host's crossings happened, for the surface that offers to
    /// block it: crossings per recorded working directory, so the caller can
    /// resolve which policy scopes actually govern this host's traffic, plus
    /// the crossings under no recorded directory, which no scope can govern.
    ///
    /// Facts only, like [`Self::cwd_facts`]. Which scope a directory falls
    /// under is the declared policy's answer and is resolved by the runtime's
    /// own loader above this store, never guessed at here.
    ///
    /// The host is matched as recorded. Normalising it is the enforcement
    /// path's job and is done with the enforcement path's own function; a
    /// second spelling rule here could offer a block over crossings the block
    /// would not then cover.
    pub fn host_crossings(&self, host: &str) -> Result<Value> {
        let mut by_cwd: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        let mut without_cwd = 0u64;
        let mut total = 0u64;
        for (_, event, record) in self.all_records()? {
            if !event.starts_with("crossing_") {
                continue;
            }
            let payload = &record["payload"];
            if payload["host_name"].as_str() != Some(host) {
                continue;
            }
            total += 1;
            match payload["cwd"].as_str() {
                Some(cwd) => *by_cwd.entry(cwd.to_owned()).or_default() += 1,
                None => without_cwd += 1,
            }
        }
        Ok(json!({
            "host": host,
            "crossings": total,
            "cwds": by_cwd
                .into_iter()
                .map(|(cwd, crossings)| json!({"cwd": cwd, "crossings": crossings}))
                .collect::<Vec<_>>(),
            "crossings_without_cwd": without_cwd,
        }))
    }

    /// What each engagement's recorded work consumed, as facts: sessions,
    /// crossings by grade, and the context footprint with the basis it was
    /// counted in.
    ///
    /// Facts only, like [`Self::cwd_facts`] — no charge and no provider,
    /// because no session record carries either. What that absence *means* is
    /// the console's to state, not this store's to guess at.
    pub fn engagement_budgets(&self, attribution: &Attribution) -> Result<Value> {
        let mut budgets: std::collections::BTreeMap<String, EngagementBudget> =
            std::collections::BTreeMap::new();
        for (_, rollup) in self.session_rollups()? {
            budgets
                .entry(rollup.engagement(attribution))
                .or_default()
                .absorb(&rollup);
        }
        Ok(Value::Array(
            budgets
                .into_iter()
                .map(|(name, budget)| budget.into_json(&name))
                .collect(),
        ))
    }

    fn all_records(&self) -> Result<Vec<(String, String, Value)>> {
        let mut statement = self
            .connection
            .prepare("SELECT session_id, event, record FROM records ORDER BY source_path, line")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .filter_map(std::result::Result::ok)
            .map(|(session, event, line)| {
                let record = serde_json::from_str(&line).unwrap_or(Value::Null);
                (session, event, record)
            })
            .collect();
        Ok(rows)
    }
}

/// The recorded crossings as a forecast reads them (`Store::crossing_facts`).
#[derive(Debug, Clone)]
pub struct CrossingFacts {
    pub facts: Vec<CrossingFact>,
    /// Crossing records that carry no URL, which no policy can judge.
    pub not_evaluated: u64,
}

/// One recorded crossing as a forecast reads it.
#[derive(Debug, Clone)]
pub struct CrossingFact {
    /// `witnessed`, `reconstructed` or `refused`; never merged.
    pub grade: &'static str,
    pub url: String,
    /// The host as recorded, already in the normalised form admission uses.
    pub host: String,
    pub cwd: Option<String>,
    /// The principal a mediated crossing recorded; an observed or
    /// reconstructed one recorded none.
    pub principal: Option<String>,
    pub licence: commonmeasure_types::LicenceState,
}

/// The three grades of evidence, told apart in one place so the session view
/// and the content view can never file the same event differently.
enum Grade {
    Witnessed,
    Reconstructed,
    Refused,
}

/// The grade an event carries, or `None` for an event this runtime does not
/// define. A future or malformed `crossing_*` is not evidence of anything, and
/// least of all of the strongest grade.
fn grade_of(event: &str) -> Option<Grade> {
    match event {
        "crossing_observed" | "crossing_mediated" => Some(Grade::Witnessed),
        "crossing_reconstructed" => Some(Grade::Reconstructed),
        "crossing_refused" => Some(Grade::Refused),
        _ => None,
    }
}

#[derive(Default)]
struct SessionRollup {
    observed: u64,
    mediated: u64,
    refused: u64,
    reconstructed: u64,
    breached: u64,
    turns: u64,
    unreadable: u64,
    grounded_witnessed: u64,
    grounded_reconstructed: u64,
    footprint: Footprint,
    first: Option<DateTime<Utc>>,
    last: Option<DateTime<Utc>>,
    hosts: HashMap<String, u64>,
    host: Option<String>,
    /// Every cwd this session recorded, with how many records carried it.
    /// Facts only; what a directory means is resolved in [`Self::engagement`].
    cwds: HashMap<String, u64>,
}

impl SessionRollup {
    fn absorb(&mut self, event: &str, record: &Value) {
        let payload = &record["payload"];
        // Crossings carry the cwd at top level; turn boundaries nest it under
        // `detail`. Both are the same fact.
        if let Some(cwd) = payload["cwd"]
            .as_str()
            .or_else(|| payload["detail"]["cwd"].as_str())
        {
            *self.cwds.entry(cwd.to_owned()).or_default() += 1;
        }
        match event {
            "crossing_observed" => self.observed += 1,
            "crossing_mediated" => self.mediated += 1,
            "crossing_refused" => self.refused += 1,
            "crossing_reconstructed" => self.reconstructed += 1,
            "turn_started" | "turn_completed" => self.turns += 1,
            "unreadable" => self.unreadable += 1,
            _ => {}
        }
        if payload["breach"].is_string() {
            self.breached += 1;
        }
        if payload["grounded"] == Value::Bool(true) {
            // Grounding is a claim about a crossing that happened. A refused
            // one did not, so it grounds nothing either way.
            match grade_of(event) {
                Some(Grade::Witnessed) => self.grounded_witnessed += 1,
                Some(Grade::Reconstructed) => self.grounded_reconstructed += 1,
                Some(Grade::Refused) | None => {}
            }
        }
        self.footprint.absorb(event, payload);
        if let Some(host) = payload["host"].as_str() {
            self.host.get_or_insert_with(|| host.to_owned());
        }
        if let Some(name) = payload["host_name"]
            .as_str()
            .filter(|name| !name.is_empty())
        {
            *self.hosts.entry(name.to_owned()).or_default() += 1;
        }
        if let Some(stamp) = payload["timestamp"]
            .as_str()
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
        {
            self.first = Some(self.first.map_or(stamp, |held| held.min(stamp)));
            self.last = Some(self.last.map_or(stamp, |held| held.max(stamp)));
        }
    }

    /// The engagement this session is read under: the one most of its
    /// cwd-bearing records resolve to (a session can cross worktrees), ties to
    /// the alphabetically first, and `unattributed` when no rule matched
    /// anything — or nothing carried a cwd at all.
    fn engagement(&self, attribution: &Attribution) -> String {
        let mut counts: HashMap<&str, u64> = HashMap::new();
        for (cwd, seen) in &self.cwds {
            if let Some(engagement) = attribution.resolve(cwd) {
                *counts.entry(engagement).or_default() += seen;
            }
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(name, _)| name.to_owned())
            .unwrap_or_else(|| UNATTRIBUTED.to_owned())
    }

    fn into_json(self, session_id: &str, engagement: &str) -> Value {
        let mut hosts: Vec<(String, u64)> = self.hosts.into_iter().collect();
        hosts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        hosts.truncate(3);
        json!({
            "session_id": session_id,
            "engagement": engagement,
            "host": self.host,
            "observed": self.observed,
            "mediated": self.mediated,
            "refused": self.refused,
            "reconstructed": self.reconstructed,
            "breached": self.breached,
            "turns": self.turns,
            "unreadable": self.unreadable,
            "grounded_witnessed": self.grounded_witnessed,
            "grounded_reconstructed": self.grounded_reconstructed,
            "estimated_tokens": self.footprint.total(),
            "token_basis": self.footprint.basis(),
            "estimated_tokens_by_basis": self.footprint.by_basis,
            "total_withheld": self.footprint.withheld(),
            "crossings_without_estimate": self.footprint.crossings_without_estimate,
            "first": self.first.map(|stamp| stamp.to_rfc3339()),
            "last": self.last.map(|stamp| stamp.to_rfc3339()),
            "top_hosts": hosts
                .into_iter()
                .map(|(name, count)| json!({"host": name, "crossings": count}))
                .collect::<Vec<_>>(),
        })
    }
}

/// One engagement's recorded consumption, folded from its sessions.
///
/// Witnessed and reconstructed stay apart here as everywhere, and the
/// footprint is a [`Footprint`]: per basis, with no total across two.
#[derive(Default)]
struct EngagementBudget {
    sessions: u64,
    witnessed: u64,
    reconstructed: u64,
    refused: u64,
    footprint: Footprint,
    hosts: HashMap<String, u64>,
    last: Option<DateTime<Utc>>,
}

impl EngagementBudget {
    fn absorb(&mut self, rollup: &SessionRollup) {
        self.sessions += 1;
        self.witnessed += rollup.observed + rollup.mediated;
        self.reconstructed += rollup.reconstructed;
        self.refused += rollup.refused;
        self.footprint.fold(&rollup.footprint);
        for (host, seen) in &rollup.hosts {
            *self.hosts.entry(host.clone()).or_default() += seen;
        }
        if let Some(stamp) = rollup.last {
            self.last = Some(self.last.map_or(stamp, |held| held.max(stamp)));
        }
    }

    fn into_json(self, engagement: &str) -> Value {
        let mut hosts: Vec<(String, u64)> = self.hosts.into_iter().collect();
        hosts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        hosts.truncate(3);
        json!({
            "engagement": engagement,
            "sessions": self.sessions,
            "witnessed": self.witnessed,
            "reconstructed": self.reconstructed,
            "refused": self.refused,
            "estimated_tokens": self.footprint.total(),
            "token_basis": self.footprint.basis(),
            "estimated_tokens_by_basis": self.footprint.by_basis,
            "total_withheld": self.footprint.withheld(),
            "crossings_without_estimate": self.footprint.crossings_without_estimate,
            "last": self.last.map(|stamp| stamp.to_rfc3339()),
            "top_hosts": hosts
                .into_iter()
                .map(|(name, count)| json!({"host": name, "crossings": count}))
                .collect::<Vec<_>>(),
        })
    }
}

/// One internal corpus path's recorded use, folded from its crossings.
///
/// Witnessed and reconstructed stay apart here as everywhere, and the
/// footprint is a [`Footprint`]: per basis, with no total across two.
#[derive(Default)]
struct InternalRollup {
    witnessed: u64,
    reconstructed: u64,
    refused: u64,
    grounded_witnessed: u64,
    grounded_reconstructed: u64,
    sessions: HashSet<String>,
    /// Every engagement this path was crossed under. A set, not a single
    /// name: a path two engagements both read belongs to both projections.
    engagements: HashSet<String>,
    footprint: Footprint,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

impl InternalRollup {
    fn absorb(&mut self, session_id: &str, event: &str, payload: &Value, engagement: &str) {
        self.engagements.insert(engagement.to_owned());
        self.sessions.insert(session_id.to_owned());
        let grounded = payload["grounded"] == Value::Bool(true);
        match grade_of(event) {
            Some(Grade::Witnessed) => {
                self.witnessed += 1;
                self.grounded_witnessed += u64::from(grounded);
            }
            Some(Grade::Reconstructed) => {
                self.reconstructed += 1;
                self.grounded_reconstructed += u64::from(grounded);
            }
            Some(Grade::Refused) => self.refused += 1,
            None => {}
        }
        self.footprint.absorb(event, payload);
        if let Some(stamp) = payload["timestamp"]
            .as_str()
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
        {
            self.first_seen = Some(self.first_seen.map_or(stamp, |held| held.min(stamp)));
            self.last_seen = Some(self.last_seen.map_or(stamp, |held| held.max(stamp)));
        }
    }

    fn into_json(self, url: &str) -> Value {
        let mut engagements: Vec<String> = self.engagements.into_iter().collect();
        engagements.sort();
        json!({
            "url": url,
            "engagements": engagements,
            "witnessed": self.witnessed,
            "reconstructed": self.reconstructed,
            "refused": self.refused,
            "grounded_witnessed": self.grounded_witnessed,
            "grounded_reconstructed": self.grounded_reconstructed,
            "sessions": self.sessions.len(),
            "estimated_tokens": self.footprint.total(),
            "token_basis": self.footprint.basis(),
            "estimated_tokens_by_basis": self.footprint.by_basis,
            "total_withheld": self.footprint.withheld(),
            "crossings_without_estimate": self.footprint.crossings_without_estimate,
            "first_seen": self.first_seen.map(|stamp| stamp.to_rfc3339()),
            "last_seen": self.last_seen.map(|stamp| stamp.to_rfc3339()),
        })
    }
}

/// A context footprint folded from crossings, kept per `token_basis`.
///
/// Estimates on different bases are never added together: a `characters/4`
/// figure plus a tokeniser's count is not a number. `estimated_tokens_by_basis`
/// always carries every figure. A combined `estimated_tokens` and its
/// `token_basis` are served only while exactly one basis is present. With two
/// or more, `estimated_tokens` is `null`, `token_basis` is `"mixed"` and
/// `total_withheld` names the reason, so a client reading the null is told
/// why in a field rather than left to guess. With none — nothing recorded an
/// estimate — `estimated_tokens` and `token_basis` are both `null` and
/// nothing is withheld: there is no figure, which is not a figure of zero.
#[derive(Default)]
struct Footprint {
    by_basis: BTreeMap<String, u64>,
    /// Crossings that admitted bytes and recorded no estimate. Each figure
    /// is a lower bound by exactly these, and the reader is told so rather
    /// than being handed a total that silently is not one.
    crossings_without_estimate: u64,
}

/// The `total_withheld` value when more than one basis is present.
const MIXED_BASES: &str = "mixed_bases";

impl Footprint {
    fn absorb(&mut self, event: &str, payload: &Value) {
        match payload["estimated_tokens"].as_u64() {
            Some(tokens) => {
                let basis = payload["token_basis"].as_str().unwrap_or("unknown");
                *self.by_basis.entry(basis.to_owned()).or_default() += tokens;
            }
            // A crossing that put bytes in context and recorded no estimate is
            // what makes a figure a lower bound rather than a total. A refused
            // crossing fetched nothing, so it has no footprint to be missing.
            None => {
                if matches!(
                    grade_of(event),
                    Some(Grade::Witnessed | Grade::Reconstructed)
                ) {
                    self.crossings_without_estimate += 1;
                }
            }
        }
    }

    /// Folds another footprint in, basis by basis. A footprint that counted
    /// nothing carries no basis, so folding it cannot turn one basis into two.
    fn fold(&mut self, other: &Footprint) {
        for (basis, tokens) in &other.by_basis {
            *self.by_basis.entry(basis.clone()).or_default() += tokens;
        }
        self.crossings_without_estimate += other.crossings_without_estimate;
    }

    /// The combined figure: a sum only while there is one basis to sum on.
    ///
    /// `None` where nothing carried an estimate, and `None` across bases. A
    /// footprint that measured nothing must not serve `0`: unmeasured and
    /// measured-zero are different facts (`docs/FAIL-POLICY.md` §7).
    fn total(&self) -> Option<u64> {
        match self.by_basis.len() {
            1 => Some(self.by_basis.values().sum()),
            _ => None,
        }
    }

    fn basis(&self) -> Option<&str> {
        match self.by_basis.len() {
            0 => None,
            1 => self.by_basis.keys().next().map(String::as_str),
            _ => Some("mixed"),
        }
    }

    fn withheld(&self) -> Option<&'static str> {
        (self.by_basis.len() > 1).then_some(MIXED_BASES)
    }
}

#[derive(Default)]
struct ContentRollup {
    witnessed: u64,
    reconstructed: u64,
    grounded_witnessed: u64,
    grounded_reconstructed: u64,
    refused: u64,
    sessions: std::collections::HashSet<String>,
    /// Every engagement this URL was crossed under. A set, not a single name:
    /// a
    /// URL two engagements both read belongs to both projections.
    engagements: std::collections::HashSet<String>,
    /// The host these crossings named, as recorded. Carried so the console can
    /// offer to block the source without re-deriving a host from the URL: a
    /// second spelling rule beside the recorded one is how a block comes to
    /// name something admission does not.
    host: Option<String>,
    /// Set when a crossing folded into this row carried the capture-time
    /// `internal` stamp. The row is marked, not hidden: it is part of the
    /// record, and its footprint is listed in the internal-use view alone.
    internal: bool,
    last_seen: Option<DateTime<Utc>>,
}

impl ContentRollup {
    fn absorb(&mut self, session_id: &str, event: &str, payload: &Value, engagement: &str) {
        self.engagements.insert(engagement.to_owned());
        self.internal |= payload["internal"] == Value::Bool(true);
        if self.host.is_none() {
            self.host = payload["host_name"].as_str().map(str::to_owned);
        }
        let grounded = payload["grounded"] == Value::Bool(true);
        match grade_of(event) {
            Some(Grade::Witnessed) => {
                self.witnessed += 1;
                self.grounded_witnessed += u64::from(grounded);
            }
            Some(Grade::Reconstructed) => {
                self.reconstructed += 1;
                self.grounded_reconstructed += u64::from(grounded);
            }
            Some(Grade::Refused) => self.refused += 1,
            None => {}
        }
        self.sessions.insert(session_id.to_owned());
        if let Some(stamp) = payload["timestamp"]
            .as_str()
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
        {
            self.last_seen = Some(self.last_seen.map_or(stamp, |held| held.max(stamp)));
        }
    }

    fn into_json(self, url: &str) -> Value {
        let mut engagements: Vec<String> = self.engagements.into_iter().collect();
        engagements.sort();
        json!({
            "url": url,
            "host": self.host,
            "internal": self.internal,
            "engagements": engagements,
            "witnessed": self.witnessed,
            "reconstructed": self.reconstructed,
            "grounded_witnessed": self.grounded_witnessed,
            "grounded_reconstructed": self.grounded_reconstructed,
            "refused": self.refused,
            "sessions": self.sessions.len(),
            "last_seen": self.last_seen.map(|stamp| stamp.to_rfc3339()),
        })
    }
}
