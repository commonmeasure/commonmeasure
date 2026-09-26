//! Durable relay state: what has actually been delivered, and to whom.
//!
//! Artefacts under `<home>/relay/`, each inspectable with a text editor:
//!
//! - `delivered.idx` — one event id per line, appended after the receiver
//!   accepts the batch carrying it. The set of ids, not a counter, so a
//!   redelivered batch can never double-count and the delivered figure is
//!   reproducible from the file alone.
//! - `receipts.json` — the last delivery outcome, written atomically. This is
//!   what the console's egress block reports, so it must never say more than
//!   the index can prove.
//! - `session-agents.json` — the agent id fixed before a wire session first
//!   leaves, retained across later batches and retries.
//! - `refused-delivered.json` — per wire session, the highest refused count
//!   a receiver has accepted on a batch, so a session whose count has moved
//!   since can be told from one the receiver already has right.
//! - `skipped-sessions.json` — the sessions the relay last skipped because
//!   their logs did not read, so the egress account names them after a run
//!   that delivered everything else. Absent when none is skipped.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::{RelayConfig, receiver_origin, same_receiver};
use crate::spool::{RefusedSpool, Spool};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Receipts {
    /// The receiver of the last delivery attempt, whatever came of it.
    #[serde(default)]
    pub receiver: Option<String>,
    /// The receiver that accepted the delivery `last_delivered_at` records.
    /// Separate from `receiver`, which a later failed attempt to another
    /// receiver overwrites while the accepted delivery stands: reading the
    /// two as one attributes a delivery to a receiver that never took it.
    /// Written with every accepted delivery; 0.3.4 and earlier wrote
    /// receipts without it, which [`last_delivery_text`] does not read.
    #[serde(default)]
    pub delivered_to: Option<String>,
    #[serde(default)]
    pub last_delivered_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Replace one relay state file whole, readable by its owner only, because
/// the files hold session and agent ids, session log paths, the receiver URL
/// and its last error. The temporary is the writer's own, so a leftover
/// `<name>.json.tmp` is neither reused nor removed.
fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    commonmeasure_harness::declaration::replace_private(path, bytes).map_err(anyhow::Error::msg)
}

/// One session the relay skipped because its log did not read, as the
/// durable account keeps it ([`crate::UnreadableSession`] is the run's own).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedSession {
    pub session: String,
    pub path: String,
    pub cause: String,
    /// Due batches spooled from the session that the run left queued.
    #[serde(default)]
    pub batches_held: u64,
    /// When the last run that met the log skipped it.
    pub skipped_at: String,
}

pub struct RelayState {
    delivered_path: PathBuf,
    receipts_path: PathBuf,
    refused_path: PathBuf,
    agents_path: PathBuf,
    skipped_path: PathBuf,
}

impl RelayState {
    pub fn open(home: &Path) -> Result<Self> {
        let dir = home.join("relay");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self::read_only(home))
    }

    /// Locate relay state without creating any directories or files.
    pub fn read_only(home: &Path) -> Self {
        let dir = home.join("relay");
        Self {
            delivered_path: dir.join("delivered.idx"),
            receipts_path: dir.join("receipts.json"),
            refused_path: dir.join("refused-delivered.json"),
            agents_path: dir.join("session-agents.json"),
            skipped_path: dir.join("skipped-sessions.json"),
        }
    }

    /// Agent identifiers fixed for wire sessions; absent before first delivery.
    pub fn session_agents(&self) -> Result<HashMap<Uuid, String>> {
        match std::fs::read(&self.agents_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("parse session-agents.json"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(error) => Err(error).context("read session-agents.json"),
        }
    }

    /// Persist identities before delivery, including a request whose response
    /// may be lost. A retry must use the identity the receiver may already hold.
    pub fn pin_session_agent(&self, session: Uuid, agent: &str) -> Result<()> {
        let mut agents = self.session_agents()?;
        if let Some(known) = agents.get(&session) {
            anyhow::ensure!(
                known == agent,
                "wire session {session} is pinned to another agent id"
            );
            return Ok(());
        }
        agents.insert(session, agent.to_owned());
        let encoded = serde_json::to_vec_pretty(&agents).context("serialise session agents")?;
        replace_private(&self.agents_path, &encoded).context("write session-agents.json")
    }

    /// The highest refused count a receiver has accepted for each wire
    /// session, ever; empty before any delivery.
    pub fn refused_delivered(&self) -> Result<HashMap<Uuid, u64>> {
        match std::fs::read(&self.refused_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("parse refused-delivered.json"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(error) => Err(error).context("read refused-delivered.json"),
        }
    }

    /// Record that a receiver accepted a batch carrying `refused` for
    /// `session`, keeping the larger of what it held and this, as the
    /// receiver does. Written atomically after the event ids are recorded.
    pub fn record_refused_delivered(&self, session: Uuid, refused: u64) -> Result<()> {
        let mut known = self.refused_delivered()?;
        let entry = known.entry(session).or_default();
        *entry = (*entry).max(refused);
        let encoded = serde_json::to_vec_pretty(&known).context("serialise refused-delivered")?;
        replace_private(&self.refused_path, &encoded).context("write refused-delivered.json")
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

    /// The sessions the relay last skipped because their logs did not read;
    /// empty when the file is absent.
    pub fn skipped_sessions(&self) -> Result<Vec<SkippedSession>> {
        match std::fs::read(&self.skipped_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("parse skipped-sessions.json"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error).context("read skipped-sessions.json"),
        }
    }

    /// Record what one run skipped. A run of the whole home replaces the
    /// list. A run scoped with `--session` names the sessions it `examined`
    /// and replaces only their entries and those of the sessions it skipped:
    /// it did not open the other logs, so it cannot clear them.
    pub fn record_skipped_sessions(
        &self,
        examined: Option<&[&str]>,
        skipped: &[crate::UnreadableSession],
        now: chrono::DateTime<Utc>,
    ) -> Result<()> {
        let mut sessions = match examined {
            Some(examined) => {
                let mut kept = self.skipped_sessions()?;
                kept.retain(|known| {
                    !examined.contains(&known.session.as_str())
                        && !skipped.iter().any(|new| new.session == known.session)
                });
                kept
            }
            None => Vec::new(),
        };
        sessions.extend(skipped.iter().map(|session| SkippedSession {
            session: session.session.clone(),
            path: session.path.display().to_string(),
            cause: format!("{:#}", session.cause),
            batches_held: session.batches_held,
            skipped_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        }));
        if sessions.is_empty() {
            return match std::fs::remove_file(&self.skipped_path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(error).context("remove skipped-sessions.json")
                }
                _ => Ok(()),
            };
        }
        sessions.sort_by(|a, b| a.session.cmp(&b.session));
        let encoded = serde_json::to_vec_pretty(&sessions).context("serialise skipped sessions")?;
        replace_private(&self.skipped_path, &encoded).context("write skipped-sessions.json")
    }

    pub fn receipts(&self) -> Receipts {
        std::fs::read(&self.receipts_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Replace the receipts atomically.
    pub fn write_receipts(&self, receipts: &Receipts) -> Result<()> {
        let encoded = serde_json::to_vec_pretty(receipts).context("serialise receipts")?;
        replace_private(&self.receipts_path, &encoded).context("write receipts.json")
    }

    pub fn record_success(&self, receiver: &str) -> Result<()> {
        self.write_receipts(&Receipts {
            receiver: Some(receiver.to_owned()),
            delivered_to: Some(receiver.to_owned()),
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

/// The last accepted delivery from `relay/receipts.json` and its age at
/// `now`, as `doctor` prints it. A file that is absent says no delivery is
/// recorded; one that does not read, or a time that does not parse, says so
/// rather than reading as never delivered.
///
/// The receiver is named only where it is not the one `relay.json` names,
/// which the caller has already printed: a delivery made under `--receiver`
/// went somewhere else and the line has to say so, and one to the configured
/// receiver would otherwise name it twice. It is named by its origin alone
/// ([`receipt_receiver_text`]).
pub fn last_delivery_text(home: &Path, now: chrono::DateTime<Utc>) -> String {
    let path = home.join("relay").join("receipts.json");
    let receipts: Receipts = match std::fs::read(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return "last delivery: none recorded".to_owned();
        }
        Err(error) => {
            return format!(
                "last delivery: unknown, {} does not read: {error}",
                path.display()
            );
        }
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(receipts) => receipts,
            Err(error) => {
                return format!(
                    "last delivery: unknown, {} does not parse: {error}",
                    path.display()
                );
            }
        },
    };
    let Some(at) = receipts.last_delivered_at else {
        return "last delivery: none recorded".to_owned();
    };
    let Some(delivered_to) = receipts.delivered_to else {
        return format!(
            "last delivery: unknown, {} was written by commonmeasure 0.3.4 or earlier and names \
             no receiver; the next accepted delivery rewrites it",
            path.display()
        );
    };
    let configured = RelayConfig::load(home).ok().flatten();
    let to = if configured.is_some_and(|config| same_receiver(&config.receiver, &delivered_to)) {
        String::new()
    } else {
        format!(" to {}", receipt_receiver_text(&delivered_to))
    };
    match chrono::DateTime::parse_from_rfc3339(&at) {
        Ok(delivered) => format!(
            "last delivery: {at}{to}, {} ago",
            age_text((now - delivered.with_timezone(&Utc)).num_seconds())
        ),
        Err(_) => format!("last delivery: {at}{to}, age unknown"),
    }
}

/// How status names a receiver a receipt records: by its origin
/// ([`receiver_origin`]), or as "an earlier receiver" where it has none.
/// Receipts written by 0.4.1 and earlier hold the receiver as it was
/// configured, and that release accepted a key in its credentials or query.
/// The relay now refuses such a `relay.json`, and the receipt stands until
/// the next accepted delivery.
fn receipt_receiver_text(receiver: &str) -> String {
    receiver_origin(receiver).unwrap_or_else(|| "an earlier receiver".to_owned())
}

/// A recorded delivery error as status shows it. The transport's one error
/// that quotes its URL, `parse url <url>: <reason>`, is reduced to the
/// reason, since the URL is a receiver that did not parse and no part of it
/// can be told apart from a key: 0.4.1 posted to such a `relay.json`
/// receiver and recorded the text, and a `--receiver` value that does not
/// parse still records it. The `url` crate's reasons are fixed strings with
/// no `: ` in them.
fn delivery_error_text(error: &str) -> String {
    match error
        .strip_prefix("parse url ")
        .and_then(|rest| rest.rsplit_once(": "))
    {
        Some((_, reason)) => format!("the receiver is not a URL: {reason}"),
        None => error.to_owned(),
    }
}

/// A non-negative age in its two largest units, e.g. `6d 2h` or `4m 10s`.
fn age_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, hours, minutes, secs) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
        seconds % 60,
    );
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
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
    let state = RelayState::read_only(home);
    let delivered_result = state.delivered();
    let delivered = delivered_result.as_ref().ok().map(|ids| ids.len() as u64);
    let receipts = state.receipts();
    let queue = queue_report(home, Utc::now());
    let pending = queue.as_ref().ok().map(|q| q.pending);
    // Read only when the spool is refused, and then only its current parts.
    let refused_spool = queue
        .as_ref()
        .err()
        .and_then(|_| Spool::read_only(home).refused().ok().flatten());
    let skipped_result = state.skipped_sessions();
    let state_error = delivered_result
        .err()
        .map(|e| format!("{e:#}"))
        .or_else(|| queue.as_ref().err().map(|e| format!("{e:#}")))
        .or_else(|| skipped_result.as_ref().err().map(|e| format!("{e:#}")));
    let skipped = skipped_result.ok();
    let last_error = queue
        .as_ref()
        .ok()
        .and_then(|q| q.last_error.clone())
        .or_else(|| receipts.last_error.clone())
        .map(|error| delivery_error_text(&error));
    let receiver = configured.as_ref().map(|config| config.receiver.clone());
    let detail = if let Some(error) = state_error.as_ref().or(config_error.as_ref()) {
        format!("{error}; nothing is sent until it parses.")
    } else {
        let pristine = receiver.is_none()
            && delivered == Some(0)
            && pending == Some(0)
            && last_error.is_none();
        let mut detail = match (&receiver, pristine) {
            (None, true) => {
                "No telemetry receiver is configured; nothing leaves this machine.".to_owned()
            }
            // No standing configuration, but the durable account shows relay
            // activity: a receiver was named explicitly on an invocation.
            (None, false) => {
                let mut detail =
                    "No telemetry receiver is configured; nothing is sent without one.".to_owned();
                if let Some(delivered) = delivered.filter(|count| *count > 0) {
                    detail.push_str(&format!(
                        " {delivered} events were delivered to {} when a receiver was named \
                         explicitly.",
                        receipts.delivered_to.as_deref().map_or_else(
                            || "an earlier receiver".to_owned(),
                            receipt_receiver_text
                        )
                    ));
                }
                detail
            }
            (Some(_), _) => match &receipts.last_delivered_at {
                Some(at) => format!("Last delivery at {at}."),
                None => "Nothing has been delivered yet.".to_owned(),
            },
        };
        if let Some(pending) = pending.filter(|count| *count > 0) {
            detail.push_str(&format!(
                " {pending} events are queued or dead in the spool."
            ));
        }
        if let Some(reason) = queue.as_ref().ok().and_then(|q| q.hold_reason.as_deref()) {
            detail.push_str(&format!(" Policy hold: {reason}"));
        }
        if let Some(error) = &last_error {
            detail.push_str(&format!(" Last attempt failed: {error}"));
        }
        for session in skipped.iter().flatten() {
            detail.push_str(&format!(" {}", skipped_session_text(session)));
        }
        detail
    };

    // The enrolled key, so the panel names the identity this edge presents
    // beside where its evidence goes. Null for an edge that is not enrolled,
    // and for one whose record does not read: `enrolment_error` then says
    // why, since that edge's enrolment is unknown rather than absent.
    let (enrolment, enrolment_error) = match commonmeasure_harness::EnrolmentRecord::load(home) {
        Ok(record) => (record, None),
        Err(error) => (None, Some(error)),
    };

    let queue = queue.ok();
    json!({
        "queued": queue.as_ref().map(|q| q.queued),
        "dead": queue.as_ref().map(|q| q.dead),
        "delivered_batches": queue.as_ref().and_then(|q| q.delivered),
        "close_error": queue.as_ref().and_then(|q| q.close_error.as_deref()),
        "close_error_unreadable": queue
            .as_ref()
            .and_then(|q| q.close_error_unreadable.as_deref()),
        "oldest_queued_age_seconds": queue.as_ref().and_then(|q| q.oldest_queued_age_seconds),
        "next_attempt_at": queue.as_ref().and_then(|q| q.next_attempt_at),
        "held": queue.as_ref().map(|q| q.held),
        "due_now": queue.as_ref().map(|q| q.due_now),
        "hold_reason": queue.as_ref().and_then(|q| q.hold_reason.as_deref()),
        "last_error": last_error,
        // Null when the file did not read; `unavailable` says why.
        "skipped_sessions": skipped,
        "unavailable": state_error.or(config_error),
        // Null unless the spool is refused as one 0.3.4 or earlier wrote.
        // `incomplete` says why its counts, if so, are only lower bounds.
        "refused_spool": refused_spool,
        "receiver": receiver,
        "delivered": delivered,
        "pending": pending,
        "detail": detail,
        "key_id": enrolment.as_ref().map(|record| record.key_id.clone()),
        "key_standing": enrolment.as_ref().map(|record| record.standing()),
        // A stored hub URL nothing is sent to: the relay refuses before it
        // spools, so the queue counts above cannot show it.
        "hub_refused": enrolment.as_ref().and_then(|record| record.hub_refused()),
        "enrolment_error": enrolment_error,
    })
}

/// The `enrolment record unreadable:` line `status` and `doctor` print for an
/// egress report ([`egress_report`]), newline included, or `None` when the
/// record read or is absent. The console states it in the same words.
pub fn enrolment_error_line(report: &Value) -> Option<String> {
    report["enrolment_error"]
        .as_str()
        .map(|error| format!("enrolment record unreadable: {error}\n"))
}

/// The `refused spool:` line `status` and `doctor` print for an egress
/// report ([`egress_report`]), newline included, or `None` when the report
/// carries no refused spool. The console states it in the same words.
pub fn refused_spool_line(report: &Value) -> Option<String> {
    serde_json::from_value::<RefusedSpool>(report["refused_spool"].clone())
        .ok()
        .map(|refused| refused_spool_text(&refused))
}

/// What a refused spool still owes, as far as its current parts show. A
/// count claims nothing about batches it could not read, so only a complete
/// count of zero is stated as nothing outstanding.
fn refused_spool_text(refused: &RefusedSpool) -> String {
    let count = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let owed = refused.outstanding + refused.unknown + refused.unindexed;
    let mut text = "refused spool: ".to_owned();
    if let Some(reason) = &refused.incomplete {
        if owed == 0 {
            return format!(
                "{text}the count is incomplete ({reason}), so what the spool owes is unknown\n"
            );
        }
        text.push_str(&format!("the count is incomplete ({reason}); at least "));
    }
    text.push_str(&count(
        refused.outstanding,
        "batch is queued or dead",
        "batches are queued or dead",
    ));
    text.push_str(" in lines with an index");
    if refused.unknown > 0 {
        text.push_str(&format!(
            "; {} no recorded delivery state, and outbound.ack may record {} as accepted",
            count(refused.unknown, "more has", "more have"),
            if refused.unknown == 1 { "it" } else { "them" }
        ));
    }
    if refused.unindexed > 0 {
        text.push_str(&format!(
            "; {} without an index {} not read",
            count(refused.unindexed, "line", "lines"),
            if refused.unindexed == 1 { "is" } else { "are" }
        ));
    }
    if owed > 0 {
        text.push_str(
            "; each is projected again only if its session log remains, or its run is passed \
             again with --run\n",
        );
    } else {
        text.push_str("; no indexed batch is outstanding\n");
    }
    text
}

/// One skipped session as `detail`, `status` and `doctor` state it.
fn skipped_session_text(session: &SkippedSession) -> String {
    let mut text = format!(
        "The relay skipped session {} at {}: {} did not read: {}.",
        session.session, session.skipped_at, session.path, session.cause
    );
    if session.batches_held > 0 {
        let (plural, verb) = if session.batches_held == 1 {
            ("", "stays")
        } else {
            ("es", "stay")
        };
        text.push_str(&format!(
            " {} batch{plural} spooled from it {verb} queued until the log reads.",
            session.batches_held
        ));
    }
    text
}

struct QueueReport {
    queued: u64,
    held: u64,
    due_now: u64,
    hold_reason: Option<String>,
    dead: u64,
    /// Unknown when the count of pruned deliveries is.
    delivered: Option<u64>,
    close_error: Option<String>,
    close_error_unreadable: Option<String>,
    pending: u64,
    oldest_queued_age_seconds: Option<i64>,
    next_attempt_at: Option<chrono::DateTime<Utc>>,
    last_error: Option<String>,
}

fn queue_report(home: &Path, now: chrono::DateTime<Utc>) -> Result<QueueReport> {
    use crate::spool::DeliveryStatus;
    // One read: states and payloads taken separately could straddle a
    // compaction that prunes delivered batches.
    let snapshot = Spool::read_only(home).snapshot()?;
    let mut report = QueueReport {
        queued: 0,
        held: 0,
        due_now: 0,
        hold_reason: None,
        dead: 0,
        delivered: None,
        close_error: snapshot.close_error.clone(),
        close_error_unreadable: snapshot.close_error_unreadable.clone(),
        pending: 0,
        oldest_queued_age_seconds: None,
        next_attempt_at: None,
        last_error: None,
    };
    let mut unknown_age = false;
    let mut error_at = None;
    // Pruned deliveries stay in the delivered count, and an unknown pruned
    // count makes the total unknown rather than the retained part of it.
    report.delivered = snapshot.pruned_delivered.map(|pruned| {
        pruned
            + snapshot
                .states
                .values()
                .filter(|state| state.status == DeliveryStatus::Delivered)
                .count() as u64
    });
    for (index, entry) in &snapshot.undelivered {
        let state = &snapshot.states[index];
        match state.status {
            DeliveryStatus::Delivered => continue,
            DeliveryStatus::Dead => report.dead += 1,
            DeliveryStatus::Queued => {
                report.queued += 1;
                if let Some(queued_at) = entry.queued_at {
                    let age = (now - queued_at).num_seconds().max(0);
                    report.oldest_queued_age_seconds =
                        Some(report.oldest_queued_age_seconds.unwrap_or(0).max(age));
                } else {
                    unknown_age = true;
                }
                if let Some(reason) = &state.hold_reason {
                    report.held += 1;
                    report.hold_reason = Some(reason.clone());
                } else {
                    if state.next_attempt_at.is_none_or(|at| at <= now) {
                        report.due_now += 1;
                    }
                    if let Some(next) = state.next_attempt_at {
                        report.next_attempt_at =
                            Some(report.next_attempt_at.map_or(next, |old| old.min(next)));
                    }
                }
            }
        }
        report.pending += entry.document["events"]
            .as_array()
            .context("spooled batch has no events")?
            .len() as u64;
        if state.last_error.is_some()
            && (report.last_error.is_none() || state.last_attempt_at >= error_at)
        {
            error_at = state.last_attempt_at;
            report.last_error = state.last_error.clone();
        }
    }
    if unknown_age {
        report.oldest_queued_age_seconds = None;
    }
    Ok(report)
}

/// Human-readable delivery standing shared by status, doctor and relay errors.
pub fn egress_text(report: &Value) -> String {
    let count = |field: &str| {
        report[field]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into())
    };
    let mut text = enrolment_error_line(report).unwrap_or_default();
    if let Some(reason) = report["hub_refused"].as_str() {
        text.push_str(&format!("hub URL refused: {reason}\n"));
    }
    text.push_str(&format!(
        "relay batches: {} queued, {} dead, {} delivered; queued and dead are undelivered\n",
        count("queued"),
        count("dead"),
        count("delivered_batches")
    ));
    let age = report["oldest_queued_age_seconds"]
        .as_u64()
        .map(|n| format!("{n}s"))
        .unwrap_or_else(|| {
            if report["queued"] == 0 {
                "none".into()
            } else {
                "unknown".into()
            }
        });
    text.push_str(&format!(
        "oldest queued: {age}; next attempt: {}\n",
        report["next_attempt_at"]
            .as_str()
            .unwrap_or(if report["queued"].is_null() {
                "unknown"
            } else if report["due_now"].as_u64().is_some_and(|n| n > 0) {
                "due now"
            } else if report["held"].as_u64().is_some_and(|n| n > 0) {
                "held"
            } else {
                "none"
            })
    ));
    if let Some(reason) = report["hold_reason"].as_str() {
        text.push_str(&format!(
            "policy hold ({} batches): {reason}\n",
            count("held")
        ));
    }
    if let Some(error) = report["last_error"].as_str() {
        text.push_str(&format!("last relay error: {error}\n"));
    }
    for session in report["skipped_sessions"].as_array().into_iter().flatten() {
        if let Ok(session) = serde_json::from_value::<SkippedSession>(session.clone()) {
            text.push_str(&format!("{}\n", skipped_session_text(&session)));
        }
    }
    if let Some(error) = report["close_error"].as_str() {
        text.push_str(&format!(
            "last spool close did not fold the delivery journal: {error}; delivery state is \
             intact and the next relay run retries\n"
        ));
    }
    if let Some(error) = report["close_error_unreadable"].as_str() {
        text.push_str(&format!(
            "relay/spool/outbound.close-error did not read ({error}), so whether the last spool \
             close folded the delivery journal is unknown; delivery state is intact and the \
             file can be removed\n"
        ));
    }
    if let Some(error) = report["unavailable"].as_str() {
        text.push_str(&format!("relay state unavailable: {error}\n"));
    }
    if let Some(refused) = refused_spool_line(report) {
        text.push_str(&refused);
    }
    if report["dead"].as_u64().is_some_and(|n| n > 0) {
        text.push_str("requeue dead batches with `commonmeasure relay requeue`, then run `commonmeasure relay`\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// EGR-131. Catches: a state file staged through the fixed
    /// `<name>.json.tmp`, which a wider leftover of that name passes its mode
    /// to, and which two writers share; a state file written under the umask.
    #[cfg(unix)]
    #[test]
    fn state_files_are_owner_only_and_never_staged_through_a_leftover() {
        under_umask_022(
            "state::tests::state_files_are_owner_only_and_never_staged_through_a_leftover",
            state_files_are_owner_only_and_never_staged_through_a_leftover_body,
        );
    }

    /// Runs `body` in a child of this test binary whose umask is 022, set
    /// between fork and exec, and asserts that the child ran the one test
    /// `name` and passed. An owner-only assertion passes whatever mode the
    /// writer asks for under a umask that already masks 066, such as 077.
    #[cfg(unix)]
    fn under_umask_022(name: &str, body: impl FnOnce()) {
        use std::os::unix::process::CommandExt as _;
        const CHILD: &str = "COMMONMEASURE_TEST_UMASK_CHILD";
        if std::env::var_os(CHILD).is_some_and(|test| test == name) {
            body();
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", name, "--test-threads=1"])
            .env(CHILD, name);
        // SAFETY: `umask` is async-signal-safe and changes only the child's
        // own process state, between fork and exec.
        unsafe {
            child.pre_exec(|| {
                libc::umask(0o022);
                Ok(())
            });
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        assert!(
            stdout.contains("test result: ok. 1 passed"),
            "the child did not run exactly one test: {stdout}"
        );
    }

    #[cfg(unix)]
    fn state_files_are_owner_only_and_never_staged_through_a_leftover_body() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let state = RelayState::open(home.path()).unwrap();
        let names = [
            "session-agents.json",
            "refused-delivered.json",
            "skipped-sessions.json",
            "receipts.json",
        ];
        let relay = home.path().join("relay");
        for name in names {
            let leftover = relay.join(name).with_extension("json.tmp");
            std::fs::write(&leftover, b"left by an older writer").unwrap();
            std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let session = Uuid::new_v4();
        state.pin_session_agent(session, "agent").unwrap();
        state.record_refused_delivered(session, 1).unwrap();
        let skipped = crate::UnreadableSession {
            session: "s-1".to_owned(),
            path: relay.join("s-1.jsonl"),
            cause: anyhow::anyhow!("unreadable"),
            batches_held: 0,
        };
        state
            .record_skipped_sessions(None, &[skipped], Utc::now())
            .unwrap();
        state.record_success("http://127.0.0.1:9").unwrap();

        for name in names {
            let mode = std::fs::metadata(relay.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
            let leftover = relay.join(name).with_extension("json.tmp");
            assert_eq!(
                std::fs::read(&leftover).unwrap(),
                b"left by an older writer",
                "{name}: the leftover is neither reused nor removed"
            );
        }
    }

    /// An enrolment record that exists but does not read is carried as an
    /// error, not dropped as though the edge were unenrolled.
    #[test]
    fn an_unreadable_enrolment_record_is_reported_with_its_error() {
        let home = tempfile::tempdir().unwrap();
        let report = egress_report(home.path());
        assert!(report["enrolment_error"].is_null(), "{report}");
        assert!(!egress_text(&report).contains("enrolment record"));

        std::fs::write(home.path().join("enrolment.json"), "{not json").unwrap();
        let report = egress_report(home.path());
        assert!(report["key_id"].is_null(), "{report}");
        let error = report["enrolment_error"].as_str().unwrap();
        assert!(error.contains("is not a valid enrolment record"), "{error}");
        assert!(
            egress_text(&report).starts_with(&format!("enrolment record unreadable: {error}\n")),
            "{}",
            egress_text(&report)
        );
    }

    /// A hand-edited record's value never reaches `enrolment_error`: serde
    /// quotes a value of the wrong type, and a stored hub URL can carry
    /// credentials. The error states the category, line and column only.
    #[test]
    fn an_unreadable_enrolment_record_quotes_no_value() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("enrolment.json");
        for (content, category) in [
            (
                "{\n  \"hub\": \"https://hub.example\",\n  \"organization\": \"https://user:ak_PLANTED@hub.example\"\n}",
                "a data error at line 3, column 55",
            ),
            (
                "{\"hub\": \"https://user:ak_PLANTED@hub.example\" x}",
                "a syntax error at line 1, column 47",
            ),
            (
                "{\"hub\": \"https://user:ak_PLANTED@hub.",
                "an end-of-file error at line 1, column 37",
            ),
        ] {
            std::fs::write(&path, content).unwrap();
            let error = egress_report(home.path())["enrolment_error"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                error,
                format!(
                    "{} is not a valid enrolment record: {category}",
                    path.display()
                )
            );
            assert!(!error.contains("ak_PLANTED"), "{error}");
        }
    }

    #[test]
    fn the_last_delivery_is_stated_with_its_age_or_why_it_is_unknown() {
        let home = tempfile::tempdir().unwrap();
        let now = at("2026-09-22T12:00:00Z");
        assert_eq!(
            last_delivery_text(home.path(), now),
            "last delivery: none recorded"
        );
        std::fs::create_dir_all(home.path().join("relay")).unwrap();
        let receipts = home.path().join("relay/receipts.json");
        std::fs::write(&receipts, "{not json").unwrap();
        assert!(
            last_delivery_text(home.path(), now).starts_with("last delivery: unknown, "),
            "an unreadable file is not a delivery that never happened"
        );
        // Receipts 0.3.4 and earlier wrote name no receiver: `receiver`
        // there is the last attempt, which a failure may have moved to
        // another receiver after this delivery was accepted. They are not
        // read as a delivery to the configured receiver.
        std::fs::write(
            &receipts,
            r#"{"receiver":"https://hub.example/api/v1/telemetry","last_delivered_at":"2026-09-16T10:00:00.000Z","last_error":null}"#,
        )
        .unwrap();
        let text = last_delivery_text(home.path(), now);
        assert!(
            text.starts_with("last delivery: unknown, ")
                && text.contains("receipts.json was written by commonmeasure 0.3.4 or earlier"),
            "{text}"
        );
        // A delivery made under `--receiver` went somewhere the configured
        // lines do not name, so this line names it.
        std::fs::write(
            home.path().join("relay.json"),
            r#"{"receiver":"https://hub.example/api/v1/telemetry"}"#,
        )
        .unwrap();
        std::fs::write(
            &receipts,
            r#"{"receiver":"https://other.example/ingest","delivered_to":"https://other.example/ingest","last_delivered_at":"2026-09-16T10:00:00.000Z"}"#,
        )
        .unwrap();
        assert_eq!(
            last_delivery_text(home.path(), now),
            "last delivery: 2026-09-16T10:00:00.000Z to https://other.example, 6d 2h ago"
        );
        // A later failure to another receiver moves `receiver` and leaves
        // the accepted delivery and its receiver where they were.
        std::fs::write(
            &receipts,
            r#"{"receiver":"https://other.example/ingest","delivered_to":"https://hub.example/api/v1/telemetry","last_delivered_at":"2026-09-16T10:00:00.000Z","last_error":"refused"}"#,
        )
        .unwrap();
        assert_eq!(
            last_delivery_text(home.path(), now),
            "last delivery: 2026-09-16T10:00:00.000Z, 6d 2h ago"
        );
        std::fs::write(&receipts, r#"{"last_error":"refused"}"#).unwrap();
        assert_eq!(
            last_delivery_text(home.path(), now),
            "last delivery: none recorded"
        );
        assert_eq!(age_text(250), "4m 10s");
        assert_eq!(age_text(-5), "0s");
    }

    /// A receipt as 0.4.1 wrote it: the receiver as configured, which that
    /// release accepted with a key in its credentials or query.
    fn write_receipt(home: &Path, delivered_to: &str) {
        std::fs::create_dir_all(home.join("relay")).unwrap();
        std::fs::write(
            home.join("relay/receipts.json"),
            json!({
                "receiver": delivered_to,
                "delivered_to": delivered_to,
                "last_delivered_at": "2026-09-20T10:00:00.000Z",
                "last_error": null,
            })
            .to_string(),
        )
        .unwrap();
    }

    /// The relay now refuses the `relay.json` such a receipt came from, and
    /// the receipt stands until the next accepted delivery, which a refused
    /// or removed file never makes. Status names its receiver by origin
    /// whether the file is refused, corrected as the 0.4.2 Upgrading note
    /// says, or removed, and not at all where the corrected file reaches the
    /// same endpoint.
    #[test]
    fn a_receipt_names_its_receiver_by_its_origin_alone() {
        let now = at("2026-09-25T12:00:00Z");
        let corrected =
            json!({"receiver": "https://hub.example/api/v1/telemetry", "api_key": "ak_PLANTED"})
                .to_string();
        for delivered_to in [
            "https://ops:ak_PLANTED@hub.example/api/v1/telemetry",
            "https://hub.example/api/v1/telemetry?api_key=ak_PLANTED",
        ] {
            let refused = json!({"receiver": delivered_to}).to_string();
            for (case, relay_json) in [
                ("refused", Some(&refused)),
                ("corrected", Some(&corrected)),
                ("absent", None),
            ] {
                let home = tempfile::tempdir().unwrap();
                write_receipt(home.path(), delivered_to);
                RelayState::open(home.path())
                    .unwrap()
                    .record_delivered(&[Uuid::new_v4()])
                    .unwrap();
                if let Some(relay_json) = relay_json {
                    std::fs::write(home.path().join("relay.json"), relay_json).unwrap();
                }
                let line = last_delivery_text(home.path(), now);
                let report = egress_report(home.path());
                let shown = format!("{line}\n{report}\n{}", egress_text(&report));
                assert!(
                    !shown.contains("ak_PLANTED"),
                    "{delivered_to} {case}: {shown}"
                );
                // Credentials are never sent, so the corrected file reaches
                // the endpoint the receipt records; a query is part of it.
                if case == "corrected" && delivered_to.contains('@') {
                    assert_eq!(line, "last delivery: 2026-09-20T10:00:00.000Z, 5d 2h ago");
                } else {
                    assert_eq!(
                        line,
                        "last delivery: 2026-09-20T10:00:00.000Z to https://hub.example, 5d 2h ago",
                        "{delivered_to} {case}"
                    );
                }
                if case == "absent" {
                    assert!(
                        report["detail"].as_str().unwrap().contains(
                            "1 events were delivered to https://hub.example when a receiver was \
                             named explicitly"
                        ),
                        "{report}"
                    );
                }
            }
        }

        // A receiver with no origin is named by no part of itself.
        let home = tempfile::tempdir().unwrap();
        write_receipt(home.path(), "hub.example/t?api_key=ak_PLANTED");
        RelayState::open(home.path())
            .unwrap()
            .record_delivered(&[Uuid::new_v4()])
            .unwrap();
        assert_eq!(
            last_delivery_text(home.path(), now),
            "last delivery: 2026-09-20T10:00:00.000Z to an earlier receiver, 5d 2h ago"
        );
        assert!(
            egress_report(home.path())["detail"]
                .as_str()
                .unwrap()
                .contains("1 events were delivered to an earlier receiver when")
        );
    }

    /// 0.4.1 posted to a `relay.json` receiver that did not parse and
    /// recorded the transport's error, which quotes the URL.
    #[test]
    fn a_recorded_error_that_quotes_a_receiver_is_shown_without_it() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("relay")).unwrap();
        std::fs::write(
            home.path().join("relay/receipts.json"),
            json!({
                "receiver": "hub.example/t?api_key=ak_PLANTED",
                "last_error": "parse url hub.example/t?api_key=ak_PLANTED/events: relative URL \
                               without a base",
            })
            .to_string(),
        )
        .unwrap();
        let report = egress_report(home.path());
        assert_eq!(
            report["last_error"],
            "the receiver is not a URL: relative URL without a base"
        );
        assert!(!egress_text(&report).contains("ak_PLANTED"));
        assert_eq!(
            delivery_error_text("receiver answered 503: busy"),
            "receiver answered 503: busy"
        );
    }

    /// The error the relay records is the real transport's, so its form is
    /// pinned here, where it is consumed: a receiver that does not parse,
    /// including one with `: ` in it, is shown by the reason alone. None of
    /// these reaches the network; each fails at the parse.
    #[test]
    fn the_transports_error_for_a_receiver_that_does_not_parse_is_shown_without_it() {
        for receiver in [
            "hub.example/t?api_key=ak_PLANTED",
            "hub.example/t?note=a: ak_PLANTED",
            "hub.example/t?note=a: b: ak_PLANTED",
            "https://ops:ak_PLANTED@[zz]/t",
            "https://ops:ak_PLANTED@hub.example:99999/t",
            "ak_PLANTED:opaque",
        ] {
            let error = crate::client::deliver(receiver, Some("ak_PLANTED"), &json!({}))
                .err()
                .expect("a receiver that does not parse is not delivered to");
            let recorded = format!("{error:#}");
            assert!(recorded.contains("ak_PLANTED"), "{receiver}: {recorded}");
            let shown = delivery_error_text(&recorded);
            assert!(
                shown.starts_with("the receiver is not a URL: "),
                "{receiver}: {shown}"
            );
            assert!(!shown.contains("ak_PLANTED"), "{receiver}: {shown}");
        }
    }
}
