//! Keeping a host's `Crawl-delay` across every mediated fetch on one edge.
//!
//! The time of the last request to each host is kept under
//! `<home>/crawl-delay/`, one JSON file per host with a lock file beside it,
//! so every MCP server on the edge, and a server started after a restart,
//! measures from the same request. A request takes its turn under the lock:
//! it reads the last request time and either writes its own send time (now,
//! or now plus the wait) and releases the lock before waiting, or refuses
//! because the wait does not fit what the fetch may still wait. Two servers
//! asking at once are spaced by the delay: the second finds the first's turn
//! already written and waits for it to pass.
//!
//! A record that cannot be read, or that is dated beyond the horizon a turn
//! can legally reach, is not a turn to measure from and is not permission to
//! fetch either. The host is treated as though it had just been asked, the
//! file is rewritten with this request's send time, and the store heals
//! itself rather than refusing that host for the life of the home.
//!
//! The owner decided on 14 September 2026 that a `Crawl-delay` addressed to
//! `CommonMeasureBot` or `*` binds in every policy mode, so nothing here reads
//! the mode.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::declarations::MAX_CRAWL_DELAY;

mod backoff;
pub use backoff::{Backoff, BackoffEvent};

/// The most one `context_fetch` spends asleep for `Crawl-delay` on an edge a
/// person runs for themselves, over the page, every redirect hop and the
/// licence and manifest probes.
///
/// It is [`MAX_CRAWL_DELAY`], so one crossing can always wait out the longest
/// delay this edge keeps and a first crossing to a host is never refused by
/// the delay it has just read. Only sleeping is charged to it: name lookups,
/// connections, signing and responses are bounded by
/// [`commonmeasure_http::CLIENT_TIMEOUT`] and
/// [`crate::discovery::PROBE_TIMEOUT`] instead, so the waiting a publisher
/// asked for is not spent on the network before the wait begins.
pub const WAIT_BUDGET: Duration = MAX_CRAWL_DELAY;

/// The most one `context_fetch` spends asleep on a hosted edge, which serves
/// its sessions over streamable HTTP.
///
/// Claude Code 2.1.278 aborts a streamable-HTTP MCP request at 60000 ms
/// unless the server's configuration raises it, so a full [`WAIT_BUDGET`] of
/// sleeping would time the call out instead of answering it. Twenty seconds
/// leaves the rest of the call inside that limit, and a host whose delay does
/// not fit is refused with the transport named rather than waited out into a
/// timeout.
pub const HOSTED_WAIT_BUDGET: Duration = Duration::from_secs(20);

/// The whole-call wall clock one `context_fetch` may take, waiting and
/// working together.
///
/// The redirect chain, its probes and the delay's sleeping are each bounded
/// on their own, and those bounds add to more than a host allows one tool
/// call: with five hops at [`commonmeasure_http::CLIENT_TIMEOUT`], two probes
/// a hop at [`crate::discovery::PROBE_TIMEOUT`] and a full [`WAIT_BUDGET`],
/// the worst case is about 310 seconds against the 300000 ms Claude Code
/// allows a stdio call by default. This ceiling is checked before each turn,
/// probe and redirect hop, and no request is given longer than what is left
/// of it ([`Pacing::request_timeout`]), so the call ends inside it, give or
/// take a name lookup, which no timeout here bounds.
pub const CALL_CEILING: Duration = Duration::from_secs(240);

/// The whole-call time limit of one `context_fetch` on a hosted edge.
///
/// Claude Code aborts a streamable-HTTP request at 60000 ms by default, so a
/// hosted call keeps a limit under it, checked before every turn, probe and
/// redirect hop as [`CALL_CEILING`] is over stdio, and no request is given
/// longer than what is left of it (lead's decision, 22 September 2026). It
/// caps how far a hosted call follows a slow redirect chain: a chain that
/// has not answered inside it is refused naming the limit, which the caller
/// can read, rather than timed out by the host, which it cannot.
pub const HOSTED_CALL_CEILING: Duration = Duration::from_secs(50);

/// Whose edge a call is served by, which decides what it may spend asleep
/// and what a refusal may tell the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// An edge the caller runs for themselves, reached over stdio: the home,
    /// the pace and the store are theirs.
    Own,
    /// A hosted edge, reached over streamable HTTP, serving several tenants
    /// from one home and one enrolment.
    Hosted,
}

impl Pace {
    /// What one `context_fetch` may spend asleep here.
    pub fn wait_budget(self) -> Duration {
        match self {
            Pace::Own => WAIT_BUDGET,
            Pace::Hosted => HOSTED_WAIT_BUDGET,
        }
    }

    /// The whole-call time limit of one `context_fetch` here.
    pub fn call_ceiling(self) -> Duration {
        match self {
            Pace::Own => CALL_CEILING,
            Pace::Hosted => HOSTED_CALL_CEILING,
        }
    }

    /// Whether a refusal may name the edge's own file system and the times
    /// of the requests it made. A hosted tenant can act on neither: the
    /// store is the operator's, and the time of the last request to a host
    /// is another tenant's fetch.
    pub fn names_the_edge(self) -> bool {
        matches!(self, Pace::Own)
    }

    /// What a refusal says about why the budget is what it is.
    fn budget_note(self) -> &'static str {
        match self {
            Pace::Own => "",
            Pace::Hosted => {
                ", which is what one call may spend asleep on a hosted edge: the session is \
                 served over HTTP, where the host ends a tool call well before a whole delay \
                 could be waited out"
            }
        }
    }
}

/// The longest a host name may be in the file name of its record before it is
/// shortened and a digest of the whole name is appended. A DNS name may be
/// 253 characters, and most file systems stop at 255 bytes for one name, so
/// the full name plus an extension does not always fit.
const MAX_NAME: usize = 64;
/// How much of the host name a shortened file name keeps, for an operator
/// reading the directory.
const KEPT_NAME: usize = 40;

/// The directory of last request times, `<home>/crawl-delay/`.
#[derive(Debug, Clone)]
pub struct CrawlDelayStore {
    dir: PathBuf,
}

/// One host's file: when the last request to it was, or will be, sent.
#[derive(Debug, Serialize, Deserialize)]
struct LastRequest {
    host: String,
    at: DateTime<Utc>,
}

/// What a host's `Crawl-delay` did to one request, as the crossing records
/// it on the `robots` evaluation of the URL requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelayRuling {
    /// The host the delay is measured on: the name in the URL, so every
    /// scheme and path of one host shares one delay.
    pub host: String,
    /// The delay kept, in milliseconds: the reading's `honoured_ms`.
    pub delay_ms: u64,
    pub outcome: DelayOutcome,
    /// How long the request waited (`waited`), or would have had to wait
    /// (`refused`). Zero when the last request was long enough ago. Absent
    /// where the edge serves several tenants from one identity and this is a
    /// refusal: the interval would name another tenant's fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    /// What the fetch could still wait when this request's turn was taken.
    pub budget_ms: u64,
    /// When the next request to the host may be sent, for a refused request.
    /// Withheld on the same terms as `wait_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<DateTime<Utc>>,
    /// Why the last request time could not be kept at all, with the remedy.
    /// The request was not sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// The record that was discarded and rewritten because it could not be
    /// read or could not be used. This request owed a turn rather than
    /// finding one, so it waited a full delay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered: Option<String>,
    /// The licence document whose turn this request waits behind: a licence
    /// with no current reading is read before the page, so the page's wait
    /// is the licence's turn and then its own. Present on a refusal decided
    /// before either turn was taken, where `wait_ms` is the whole of that
    /// wait and `next_at` when the host may next be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_first: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayOutcome {
    /// The last request was at least the delay ago; sent without waiting.
    Clear,
    /// Sent after waiting `wait_ms`.
    Waited,
    /// The wait did not fit the budget; nothing was sent.
    Refused,
    /// The turn could not be kept, so the delay could not be kept either;
    /// nothing was sent.
    Unavailable,
}

impl DelayRuling {
    /// Whether the request may go, once the pacing has waited out its turn.
    pub fn sends(&self) -> bool {
        matches!(self.outcome, DelayOutcome::Clear | DelayOutcome::Waited)
    }

    /// How long this ruling's turn has still to be slept through.
    fn sleep(&self) -> Duration {
        if self.outcome == DelayOutcome::Waited {
            Duration::from_millis(self.wait_ms.unwrap_or_default())
        } else {
            Duration::ZERO
        }
    }

    /// Drop the times that would name another tenant's request.
    pub(crate) fn withhold_times(&mut self) {
        self.wait_ms = None;
        self.next_at = None;
    }
}

/// What one `context_fetch` has left to spend asleep. Charged with sleeping
/// alone, so the discovery a crossing does before the wait does not eat it.
#[derive(Debug)]
pub struct WaitBudget {
    left: Cell<Duration>,
}

impl WaitBudget {
    pub fn new(total: Duration) -> Self {
        Self {
            left: Cell::new(total),
        }
    }

    pub fn left(&self) -> Duration {
        self.left.get()
    }

    /// Sleep `wait` and charge it.
    fn sleep(&self, wait: Duration) {
        if wait.is_zero() {
            return;
        }
        self.left.set(self.left.get().saturating_sub(wait));
        std::thread::sleep(wait);
    }
}

/// The pacing of one `context_fetch`: the store it takes turns in, what the
/// call may still spend asleep, how long the whole call has left, and the
/// delay read for each host so far, so the licence and manifest probes are
/// paced by the same delay as the page.
///
/// A publisher's log cannot tell those probes from the page fetch, so they
/// take a turn too. `robots.txt` does not: the delay cannot be read without
/// it. A probe never refuses a crossing — where its wait does not fit what is
/// left of the budget it is not sent, and the record says so.
pub struct Pacing {
    store: CrawlDelayStore,
    budget: WaitBudget,
    /// Host to the delay kept for it, in milliseconds, as read this call.
    delays: RefCell<BTreeMap<String, u64>>,
    pace: Pace,
    /// When the call began, against which [`CALL_CEILING`] is measured. The
    /// ceiling bounds the whole call, not the sleeping alone, because the
    /// requests and the waiting share one host timeout.
    started: Instant,
    ceiling: Duration,
    backoff_events: RefCell<Vec<BackoffEvent>>,
    reservations: RefCell<BTreeMap<String, String>>,
    probe_reserve: std::cell::Cell<Duration>,
}

#[cfg(not(test))]
fn ceiling_for(pace: Pace) -> Duration {
    pace.call_ceiling()
}

/// The whole-call ceiling, which a unit test may shorten on its own thread
/// to reach the end of a call without waiting minutes for it. It shortens
/// the limit only; every check against it runs as in production.
#[cfg(test)]
fn ceiling_for(pace: Pace) -> Duration {
    TEST_CEILING
        .with(std::cell::Cell::get)
        .unwrap_or_else(|| pace.call_ceiling())
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CEILING: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

impl Pacing {
    /// The pacing of one call served at `pace`, with that pace's wait budget
    /// and the whole-call ceiling starting now.
    pub fn new(store: CrawlDelayStore, pace: Pace) -> Self {
        Self::with_budget(store, pace.wait_budget(), pace)
    }

    pub(crate) fn with_budget(store: CrawlDelayStore, budget: Duration, pace: Pace) -> Self {
        Self {
            store,
            budget: WaitBudget::new(budget),
            delays: RefCell::new(BTreeMap::new()),
            pace,
            started: Instant::now(),
            ceiling: ceiling_for(pace),
            backoff_events: RefCell::new(Vec::new()),
            reservations: RefCell::new(BTreeMap::new()),
            probe_reserve: std::cell::Cell::new(Duration::ZERO),
        }
    }

    /// Record the delay this call read for `host`, so the probes to it are
    /// paced by it too. Two readings in one call keep the longer.
    pub fn learn(&self, host: &str, honoured_ms: u64) {
        if host.is_empty() {
            return;
        }
        let mut delays = self.delays.borrow_mut();
        let kept = delays.entry(host.to_owned()).or_insert(honoured_ms);
        *kept = (*kept).max(honoured_ms);
    }

    /// The delay this call read for `host`, where it states one that paces.
    pub fn delay_for(&self, host: &str) -> Option<Duration> {
        self.delay_ms(host).map(Duration::from_millis)
    }

    fn delay_ms(&self, host: &str) -> Option<u64> {
        self.delays.borrow().get(host).copied().filter(|ms| *ms > 0)
    }

    /// What is left of the whole-call ceiling.
    pub fn left(&self) -> Duration {
        self.ceiling.saturating_sub(self.started.elapsed())
    }

    /// Whether the call has used its whole-call ceiling, so nothing further
    /// may be sent or waited for.
    pub fn over_ceiling(&self) -> bool {
        self.left().is_zero()
    }

    /// Why nothing more is sent once the ceiling is reached.
    pub fn ceiling_reason(&self) -> String {
        format!(
            "this call reached its time limit: one `context_fetch` may take {}s on this edge{}, \
             and nothing further is sent once that is used",
            self.pace.call_ceiling().as_secs(),
            match self.pace {
                Pace::Own => "",
                Pace::Hosted => ", which serves its sessions over HTTP",
            }
        )
    }

    /// The longest the next request may take: `limit`, or what is left of
    /// the whole-call time limit where that is less, so a request in flight
    /// never carries the call past it. `None` once nothing is left.
    pub fn request_timeout(&self, limit: Duration) -> Option<Duration> {
        let left = self.left();
        (!left.is_zero()).then(|| limit.min(left))
    }

    /// End a probe's reservation of wait budget before the page proceeds.
    pub fn end_probe(&self) {
        self.probe_reserve.set(Duration::ZERO);
    }

    /// What the call may still spend asleep: what is left of the wait budget,
    /// and never more than what is left before the whole-call ceiling.
    fn spendable(&self) -> Duration {
        self.budget.left().min(self.left())
    }

    /// Take the turn of a page or hop request to `host`, waiting it out where
    /// the wait fits what the call may still spend asleep. The request goes
    /// only where [`DelayRuling::sends`].
    pub fn turn(&self, host: &str, delay: Duration, now: DateTime<Utc>) -> DelayRuling {
        self.turn_within(host, delay, self.spendable(), now)
    }

    fn turn_within(
        &self,
        host: &str,
        delay: Duration,
        budget: Duration,
        now: DateTime<Utc>,
    ) -> DelayRuling {
        let mut ruling = self.store.take_turn(host, delay, budget, now, self.pace);
        self.budget.sleep(ruling.sleep());
        if !self.pace.names_the_edge() && ruling.outcome == DelayOutcome::Refused {
            ruling.withhold_times();
        }
        ruling
    }

    /// Take the turn of a probe to `url`, waiting it out. `Ok` carries the
    /// instant the probe is sent, which is `now` plus whatever it waited, so
    /// the record of the answer is dated when the request was made rather
    /// than when the wait began. `Err` says the probe was not sent, for the
    /// record that would have held its answer; no probe refuses a crossing.
    pub fn probe(&self, url: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
        self.probe_within(url, now, self.spendable())
    }

    /// Take the turn of a probe that must leave `reserve` of the budget for
    /// requests still to come after it: a licence read before the page keeps
    /// back the page's turn. Where the budget left is less than `reserve`
    /// the probe is not sent at all, and nothing is written.
    pub fn probe_reserving(
        &self,
        url: &str,
        now: DateTime<Utc>,
        reserve: Duration,
    ) -> Result<DateTime<Utc>, String> {
        let spendable = self.spendable();
        if spendable < reserve {
            return Err(format!(
                "not sent: the requests still to come after it need {}s of the {}s this fetch \
                 may still spend waiting{}",
                seconds(millis(reserve)),
                seconds(millis(spendable)),
                self.pace.budget_note()
            ));
        }
        self.probe_within(url, now, spendable - reserve)
    }

    /// How long a request to `host` would wait now, under the delay read for
    /// it this call, without taking a turn or writing anything. Zero where
    /// no delay paces the host. Read without the lock: it is an estimate for
    /// deciding whether a sequence of turns can fit before the first is
    /// taken, and each turn is still taken under the lock and refused where
    /// another server has moved the host's turn since.
    pub fn pending_wait(&self, host: &str, now: DateTime<Utc>) -> Duration {
        match self.delay_ms(host) {
            Some(delay_ms) => self
                .store
                .pending_wait(host, Duration::from_millis(delay_ms), now),
            None => Duration::ZERO,
        }
    }

    /// What this call may still spend asleep.
    pub fn spendable_now(&self) -> Duration {
        self.spendable()
    }

    /// The pace this call is served at.
    pub fn pace(&self) -> Pace {
        self.pace
    }

    /// Take the turn of a probe that may not wait: it is sent where the host
    /// is clear now and is otherwise not sent. The manifest probe of a paced
    /// host takes its turn this way before the page, so it never spends the
    /// wait the page needs.
    pub fn free_probe(&self, url: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
        self.probe_within(url, now, Duration::ZERO)
    }

    fn probe_within(
        &self,
        url: &str,
        now: DateTime<Utc>,
        budget: Duration,
    ) -> Result<DateTime<Utc>, String> {
        // The ceiling bounds the whole call, so it stops a probe to a host
        // that states no delay as much as one to a host that does.
        if self.over_ceiling() {
            return Err(format!("not sent: {}", self.ceiling_reason()));
        }
        self.probe_reserve
            .set(self.spendable().saturating_sub(budget));
        let host = crate::grounding::host_of(url);
        let before = self.budget.left();
        self.before_turn(&host, budget)?;
        let waited = before.saturating_sub(self.budget.left());
        let now = if waited.is_zero() { now } else { Utc::now() };
        let budget = budget.saturating_sub(waited);
        let Some(delay_ms) = self.delay_ms(&host) else {
            return Ok(now);
        };
        let ruling = self.turn_within(&host, Duration::from_millis(delay_ms), budget, now);
        if ruling.sends() {
            let waited = chrono::Duration::milliseconds(as_i64(Duration::from_millis(
                ruling.wait_ms.unwrap_or_default(),
            )));
            return Ok(now.checked_add_signed(waited).unwrap_or(now));
        }
        Err(format!(
            "not sent: {host} states `Crawl-delay: {}s`, which this edge keeps for a probe as \
             for a page, and {}",
            seconds(ruling.delay_ms),
            match &ruling.unavailable {
                Some(reason) => reason.clone(),
                None if budget.is_zero() => "the host is inside its delay, so the probe waits \
                                             for a crossing that finds it clear"
                    .to_owned(),
                None => format!(
                    "the wait is longer than the {}s this fetch may still spend waiting{}",
                    seconds(millis(budget)),
                    self.pace.budget_note()
                ),
            }
        ))
    }

    /// Whether a refusal this pacing produced may name absolute times and the
    /// edge's own file system.
    pub fn names_the_edge(&self) -> bool {
        self.pace.names_the_edge()
    }
}

impl CrawlDelayStore {
    pub fn open(home: &Path) -> Self {
        Self {
            dir: home.join("crawl-delay"),
        }
    }

    /// Where one host's record lives. An operator clearing a host's pace
    /// removes this file; removing the directory clears every host.
    pub fn path_of(&self, host: &str) -> PathBuf {
        self.path_for(host, "json")
    }

    /// The directory of records, named in a refusal so the remedy is at hand.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, host: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{}.{extension}", file_name(host)))
    }

    /// Take a request's turn at `host` under a delay of `delay`, with
    /// `budget` left to wait. The turn is written before the lock is
    /// released, so the caller must send only when [`DelayRuling::sends`],
    /// after sleeping `wait_ms`. Use [`Pacing`] rather than this directly
    /// where a call's budget is being kept.
    ///
    /// `pace` decides how much of the edge a refusal may describe: a hosted
    /// tenant is told what happened without the operator's file system or
    /// the instants of requests another tenant made.
    ///
    /// A delay longer than [`MAX_CRAWL_DELAY`] is kept at that bound, as the
    /// reader keeps it: this edge states one bound and holds to it.
    pub fn take_turn(
        &self,
        host: &str,
        delay: Duration,
        budget: Duration,
        now: DateTime<Utc>,
        pace: Pace,
    ) -> DelayRuling {
        let delay = delay.min(MAX_CRAWL_DELAY);
        let mut ruling = DelayRuling {
            host: host.to_owned(),
            delay_ms: millis(delay),
            outcome: DelayOutcome::Clear,
            wait_ms: Some(0),
            budget_ms: millis(budget),
            next_at: None,
            unavailable: None,
            recovered: None,
            licence_first: None,
        };
        if let Err(reason) = self.turn(&mut ruling, delay, budget, now, pace) {
            ruling.outcome = DelayOutcome::Unavailable;
            ruling.wait_ms = Some(0);
            ruling.next_at = None;
            ruling.unavailable = Some(reason);
        }
        ruling
    }

    /// How long a request to `host` would wait now under `delay`, read
    /// without the lock and without writing. A record that cannot be read,
    /// or one beyond the horizon, owes a full delay, as a turn would find.
    pub fn pending_wait(&self, host: &str, delay: Duration, now: DateTime<Utc>) -> Duration {
        let delay = delay.min(MAX_CRAWL_DELAY);
        let last = match std::fs::read(self.path_for(host, "json")) {
            Ok(bytes) => match serde_json::from_slice::<LastRequest>(&bytes) {
                Ok(record) => record.at,
                Err(_) => return delay,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Duration::ZERO;
            }
            Err(_) => return delay,
        };
        let horizon = chrono::Duration::milliseconds(as_i64(MAX_CRAWL_DELAY + WAIT_BUDGET));
        if now
            .checked_add_signed(horizon)
            .is_some_and(|edge| last > edge)
        {
            return delay;
        }
        last.checked_add_signed(chrono::Duration::milliseconds(as_i64(delay)))
            .filter(|next| *next > now)
            .and_then(|next| (next - now).to_std().ok())
            .unwrap_or_default()
    }

    /// How a message names one of the store's files. A hosted tenant is
    /// given the file's name alone: the path is the operator's, on a machine
    /// the tenant cannot reach.
    fn named(path: &Path, pace: Pace) -> String {
        if pace.names_the_edge() {
            path.display().to_string()
        } else {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "the host's record".to_owned())
        }
    }

    /// The remedy a refusal names when the turn cannot be kept. The store is
    /// fail-closed: with no turn there is no pace, and no page is fetched.
    fn remedy(&self, detail: &str, pace: Pace) -> String {
        if pace.names_the_edge() {
            return format!(
                "{detail}. The time of the last request to each host is kept under {}; make the \
                 operator home writable, or remove that directory to have it rebuilt, and the \
                 fetch goes through",
                self.dir.display()
            );
        }
        format!(
            "{detail}. The time of the last request to each host is kept on the edge that serves \
             this session; its operator can make that store writable or clear it, and the fetch \
             goes through"
        )
    }

    fn turn(
        &self,
        ruling: &mut DelayRuling,
        delay: Duration,
        budget: Duration,
        now: DateTime<Utc>,
        pace: Pace,
    ) -> Result<(), String> {
        if ruling.host.is_empty() {
            return Err(
                "the URL names no host, so there is no host to keep a delay for".to_owned(),
            );
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            self.remedy(
                &format!(
                    "{} could not be created: {error}",
                    Self::named(&self.dir, pace)
                ),
                pace,
            )
        })?;
        let lock_path = self.path_for(&ruling.host, "lock");
        // The editor's words ("no edit was made") are about a declaration
        // being edited; here the turn is what was not taken.
        let _lock = crate::declaration::lock(&lock_path).map_err(|refused| match refused {
            crate::declaration::LockRefused::Busy(_) => self.remedy(
                &format!(
                    "another server on this edge held {} while this request waited for its turn",
                    Self::named(&lock_path, pace)
                ),
                pace,
            ),
            crate::declaration::LockRefused::Failed(reason) => self.remedy(&reason, pace),
        })?;
        let path = self.path_for(&ruling.host, "json");
        // Leftovers of a write interrupted by a crash. Every writer of this
        // host's temporaries holds the lock now held, so nothing here is
        // being written.
        self.clear_temporaries(&ruling.host);
        // A record that cannot be read is not a turn to measure from and is
        // not permission to fetch either: the host is treated as though it
        // had just been asked, and the fresh record below replaces it.
        let (last, mut recovered) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<LastRequest>(&bytes) {
                Ok(record) => (Some(record.at), None),
                Err(error) => (
                    Some(now),
                    Some(format!(
                        "{} could not be read as a turn ({error}), so this request owed one",
                        Self::named(&path, pace)
                    )),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(error) => (
                Some(now),
                Some(format!(
                    "{} could not be opened ({error}), so this request owed a turn",
                    Self::named(&path, pace)
                )),
            ),
        };
        // Both are bounded by the cap above, so neither conversion can
        // overflow the millisecond count.
        let delay = chrono::Duration::milliseconds(as_i64(delay));
        // A turn is never written further ahead than the longest delay plus
        // the longest wait, so a time beyond that is a clock that moved
        // backwards; measuring from it would shut the host out until the
        // clock caught up, and taking it as clear would drop the delay.
        let horizon = chrono::Duration::milliseconds(as_i64(MAX_CRAWL_DELAY + WAIT_BUDGET));
        let last = match last {
            Some(at)
                if now
                    .checked_add_signed(horizon)
                    .is_some_and(|edge| at > edge) =>
            {
                // The instant itself is a request this edge made, so a
                // hosted tenant is told the record was unusable and not
                // when another tenant's fetch was recorded.
                recovered = Some(match pace.names_the_edge() {
                    true => format!(
                        "the turn recorded for {} was {}, beyond the {}s a turn can reach, so the \
                         clock has moved backwards and this request owed a turn",
                        ruling.host,
                        at.to_rfc3339(),
                        horizon.num_seconds()
                    ),
                    false => format!(
                        "the turn recorded for {} was beyond the {}s a turn can reach, so the \
                         clock has moved backwards and this request owed a turn",
                        ruling.host,
                        horizon.num_seconds()
                    ),
                });
                Some(now)
            }
            other => other,
        };
        ruling.recovered = recovered;
        // The horizon above bounds `at`, so this addition cannot overflow.
        let next = last
            .and_then(|at| at.checked_add_signed(delay))
            .filter(|next| *next > now);
        let send_at = match next {
            None => now,
            Some(next) => {
                let wait = (next - now).to_std().unwrap_or_default();
                ruling.wait_ms = Some(millis(wait));
                if wait > budget {
                    ruling.outcome = DelayOutcome::Refused;
                    ruling.next_at = Some(next);
                    // The corrupt record, where there was one, is left for a
                    // call with the budget to take its turn: writing a turn
                    // nobody took would push the next request out again.
                    return Ok(());
                }
                ruling.outcome = DelayOutcome::Waited;
                next
            }
        };
        let record = LastRequest {
            host: ruling.host.clone(),
            at: send_at,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(|error| error.to_string())?;
        crate::declaration::replace(&path, &bytes).map_err(|error| self.remedy(&error, pace))
    }

    /// Remove this host's interrupted writes. Called under the host's lock.
    ///
    /// The sweep reads the whole directory, which on an edge that has fetched
    /// many hosts is not work to repeat on every paced turn. A leftover is a
    /// crash's, so one sweep per host per process clears what was there when
    /// this process started; a leftover another process drops afterwards is
    /// cleared by whichever process next takes that host's turn.
    fn clear_temporaries(&self, host: &str) {
        let swept = SWEPT.get_or_init(|| Mutex::new(BTreeSet::new()));
        {
            let mut swept = swept.lock().unwrap_or_else(|held| held.into_inner());
            if !swept.insert(self.path_for(host, "json")) {
                return;
            }
        }
        // Prune only while this caller already holds the host lock. A
        // healthy response otherwise requires no write access to the store.
        if let Ok(Some(backoff)) = self.backoff(host)
            && backoff.failures == 0
            && backoff.until <= Utc::now()
        {
            std::fs::remove_file(self.path_for(host, "backoff.json")).ok();
        }
        let prefix = format!("{}.json.", file_name(host));
        let backoff_prefix = format!("{}.backoff.json.", file_name(host));
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with(&prefix) || name.starts_with(&backoff_prefix))
                && name.ends_with(".tmp")
            {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }
}

/// The records whose interrupted writes this process has already swept, by
/// the record's own path, so the directory is read once per host rather than
/// on every turn.
static SWEPT: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

/// The file name for a host. Brackets and colons of an IPv6 literal, and
/// anything else outside a DNS name, are folded to `_`. A name too long for
/// a file system keeps its first characters and carries a digest of the whole
/// host: a legal DNS name of 253 characters would otherwise be refused by the
/// file system, and every fetch to that host with it.
fn file_name(host: &str) -> String {
    let folded: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if folded.len() <= MAX_NAME {
        return folded;
    }
    // Folding is one ASCII character per character, so this index is a
    // character boundary.
    let digest = Sha256::digest(host.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{hex}", &folded[..KEPT_NAME])
}

pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Milliseconds for `chrono`, which counts them signed.
fn as_i64(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Milliseconds as seconds for a reader: `2000` is `2`, `1500` is `1.5`.
pub fn seconds(ms: u64) -> String {
    let whole = ms / 1000;
    let fraction = ms % 1000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "publisher.example";

    fn wait_ms(ruling: &DelayRuling) -> u64 {
        ruling.wait_ms.expect("a wait")
    }

    #[test]
    fn a_second_request_inside_the_delay_waits_for_the_first_turn_to_pass() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let start = Utc::now();
        let delay = Duration::from_secs(2);
        let first = store.take_turn(HOST, delay, WAIT_BUDGET, start, Pace::Own);
        assert_eq!(first.outcome, DelayOutcome::Clear);
        assert_eq!(wait_ms(&first), 0);

        let later = start + chrono::Duration::milliseconds(500);
        let second = store.take_turn(HOST, delay, WAIT_BUDGET, later, Pace::Own);
        assert_eq!(second.outcome, DelayOutcome::Waited);
        assert_eq!(wait_ms(&second), 1_500);
        // The second turn is written at its send time, so a third request
        // asked at the same moment waits for that one.
        let third = store.take_turn(HOST, delay, WAIT_BUDGET, later, Pace::Own);
        assert_eq!(wait_ms(&third), 3_500);

        let after = start + chrono::Duration::seconds(10);
        assert_eq!(
            store
                .take_turn(HOST, delay, WAIT_BUDGET, after, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
    }

    #[test]
    fn a_wait_beyond_the_budget_is_refused_with_the_next_time_and_takes_no_turn() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let start = Utc::now();
        let delay = Duration::from_secs(45);
        store.take_turn(HOST, delay, WAIT_BUDGET, start, Pace::Own);
        let budget = Duration::from_secs(30);
        let refused = store.take_turn(HOST, delay, budget, start, Pace::Own);
        assert_eq!(refused.outcome, DelayOutcome::Refused);
        assert!(!refused.sends());
        assert_eq!(wait_ms(&refused), 45_000);
        assert_eq!(refused.budget_ms, 30_000);
        assert_eq!(refused.next_at, Some(start + chrono::Duration::seconds(45)));
        // The refusal wrote nothing: the next request measures from the
        // first one still.
        let later = start + chrono::Duration::seconds(46);
        assert_eq!(
            store
                .take_turn(HOST, delay, WAIT_BUDGET, later, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
    }

    #[test]
    fn hosts_are_kept_apart_and_a_turn_beyond_the_horizon_is_discarded_for_a_fresh_one() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let delay = Duration::from_secs(5);
        store.take_turn(HOST, delay, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(
            store
                .take_turn("other.example", delay, WAIT_BUDGET, now, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
        let ahead = LastRequest {
            host: HOST.to_owned(),
            at: now + chrono::Duration::days(1),
        };
        std::fs::write(store.path_of(HOST), serde_json::to_vec(&ahead).unwrap()).unwrap();
        let ruling = store.take_turn(HOST, delay, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(ruling.outcome, DelayOutcome::Waited);
        assert_eq!(wait_ms(&ruling), 5_000);
        assert!(
            ruling
                .recovered
                .as_deref()
                .is_some_and(|reason| reason.contains("clock has moved backwards")),
            "{ruling:?}"
        );
        // The fresh record is readable, so the host is not shut out.
        let after = now + chrono::Duration::seconds(30);
        assert_eq!(
            store
                .take_turn(HOST, delay, WAIT_BUDGET, after, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
    }

    #[test]
    fn an_unreadable_record_owes_a_turn_and_is_rewritten() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(home.path().join("crawl-delay")).unwrap();
        std::fs::write(store.path_of(HOST), b"not json").unwrap();
        let now = Utc::now();
        let delay = Duration::from_secs(2);
        let ruling = store.take_turn(HOST, delay, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(ruling.outcome, DelayOutcome::Waited);
        assert_eq!(wait_ms(&ruling), 2_000);
        assert!(ruling.sends(), "the delay is kept, not the host shut out");
        assert!(
            ruling
                .recovered
                .as_deref()
                .is_some_and(|reason| reason.contains("could not be read as a turn")),
            "{ruling:?}"
        );
        // Healed: the next request reads the rewritten record.
        let later = now + chrono::Duration::seconds(10);
        let next = store.take_turn(HOST, delay, WAIT_BUDGET, later, Pace::Own);
        assert_eq!(next.outcome, DelayOutcome::Clear);
        assert!(next.recovered.is_none(), "{next:?}");
    }

    #[test]
    fn a_host_too_long_for_a_file_name_is_kept_under_a_digest() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        // The longest legal DNS name: 253 characters.
        let long: String = std::iter::repeat_n("label", 51).collect::<String>()[..253].to_owned();
        let long = long
            .char_indices()
            .map(|(index, c)| if index % 10 == 9 { '.' } else { c })
            .collect::<String>();
        assert_eq!(long.len(), 253);
        let name = store.path_of(&long);
        assert!(name.file_name().unwrap().len() <= 255, "{}", name.display());
        let now = Utc::now();
        let delay = Duration::from_secs(2);
        assert_eq!(
            store
                .take_turn(&long, delay, WAIT_BUDGET, now, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
        let second = store.take_turn(&long, delay, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(second.outcome, DelayOutcome::Waited);
        // Two long hosts differing only past the kept characters are still
        // two hosts.
        let other = format!("{}x", &long[..252]);
        assert_eq!(
            store
                .take_turn(&other, delay, WAIT_BUDGET, now, Pace::Own)
                .outcome,
            DelayOutcome::Clear
        );
    }

    #[test]
    fn a_home_that_cannot_hold_the_store_refuses_rather_than_dropping_the_delay() {
        let home = tempfile::tempdir().unwrap();
        // A file where the directory must be: nothing can be written.
        std::fs::write(home.path().join("crawl-delay"), b"in the way").unwrap();
        let store = CrawlDelayStore::open(home.path());
        let ruling = store.take_turn(
            HOST,
            Duration::from_secs(2),
            WAIT_BUDGET,
            Utc::now(),
            Pace::Own,
        );
        assert_eq!(ruling.outcome, DelayOutcome::Unavailable);
        assert!(!ruling.sends());
        let reason = ruling.unavailable.expect("a reason");
        assert!(reason.contains("crawl-delay"), "{reason}");
        assert!(reason.contains("remove that directory"), "{reason}");
    }

    #[test]
    fn a_url_with_no_host_is_refused_rather_than_sharing_one_record() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let ruling = store.take_turn(
            "",
            Duration::from_secs(2),
            WAIT_BUDGET,
            Utc::now(),
            Pace::Own,
        );
        assert_eq!(ruling.outcome, DelayOutcome::Unavailable);
        assert!(
            ruling.unavailable.expect("a reason").contains("no host"),
            "an empty host is not a host"
        );
    }

    #[test]
    fn an_interrupted_write_is_cleared_when_the_host_next_takes_its_turn() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(home.path().join("crawl-delay")).unwrap();
        let leftover = home
            .path()
            .join("crawl-delay")
            .join(format!("{HOST}.json.999.abc.tmp"));
        std::fs::write(&leftover, b"half a record").unwrap();
        store.take_turn(
            HOST,
            Duration::from_secs(2),
            WAIT_BUDGET,
            Utc::now(),
            Pace::Own,
        );
        assert!(!leftover.exists(), "the leftover was left behind");
    }

    #[test]
    fn a_delay_longer_than_the_bound_is_kept_at_the_bound() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let hour = Duration::from_secs(3_600);
        let first = store.take_turn(HOST, hour, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(first.delay_ms, 60_000);
        let second = store.take_turn(HOST, hour, WAIT_BUDGET, now, Pace::Own);
        assert_eq!(wait_ms(&second), 60_000);
    }

    #[test]
    fn the_budget_is_charged_with_sleeping_alone() {
        let budget = WaitBudget::new(Duration::from_secs(30));
        assert_eq!(budget.left(), Duration::from_secs(30));
        budget.sleep(Duration::from_millis(20));
        assert!(budget.left() <= Duration::from_secs(30) - Duration::from_millis(20));
        assert!(budget.left() >= Duration::from_secs(29));
    }

    #[test]
    fn a_refusal_over_a_shared_identity_names_no_time() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let delay = Duration::from_secs(60);
        let pacing = Pacing::with_budget(store.clone(), Duration::from_secs(1), Pace::Hosted);
        store.take_turn(HOST, delay, WAIT_BUDGET, now, Pace::Own);
        let refused = pacing.turn(HOST, delay, now);
        assert_eq!(refused.outcome, DelayOutcome::Refused);
        assert!(refused.next_at.is_none(), "{refused:?}");
        assert!(refused.wait_ms.is_none(), "{refused:?}");

        let telling = Pacing::with_budget(store.clone(), Duration::from_secs(1), Pace::Own);
        let refused = telling.turn(HOST, delay, now);
        assert!(refused.next_at.is_some(), "{refused:?}");
    }

    #[test]
    fn a_hosted_call_waits_less_and_says_which_transport_holds_it_to_that() {
        assert_eq!(Pace::Own.wait_budget(), WAIT_BUDGET);
        assert_eq!(Pace::Hosted.wait_budget(), HOSTED_WAIT_BUDGET);
        assert!(HOSTED_WAIT_BUDGET < Duration::from_secs(60));

        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let delay = Duration::from_secs(45);
        let pacing = Pacing::new(store.clone(), Pace::Hosted);
        pacing.learn("publisher.example", millis(delay));
        // The turn a page took; a 45-second wait is inside what this edge
        // keeps but beyond what a hosted call may spend asleep.
        store.take_turn("publisher.example", delay, WAIT_BUDGET, now, Pace::Own);
        let refused = pacing
            .probe("https://publisher.example/licence.xml", now)
            .expect_err("the wait does not fit a hosted call");
        assert!(refused.contains("served over HTTP"), "{refused}");
        assert!(refused.contains("20s"), "{refused}");
    }

    #[test]
    fn a_hosted_refusal_names_no_path_and_no_recorded_instant() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("crawl-delay"), b"in the way").unwrap();
        let store = CrawlDelayStore::open(home.path());
        let ruling = store.take_turn(
            HOST,
            Duration::from_secs(2),
            WAIT_BUDGET,
            Utc::now(),
            Pace::Hosted,
        );
        let reason = ruling.unavailable.expect("a reason");
        assert!(
            !reason.contains(&home.path().display().to_string()),
            "{reason}"
        );
        assert!(
            reason.contains("the edge that serves this session"),
            "{reason}"
        );

        // A record from a clock that moved backwards: the tenant is told the
        // record was unusable, not when another tenant's fetch was recorded.
        let clean = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(clean.path());
        let now = Utc::now();
        std::fs::create_dir_all(clean.path().join("crawl-delay")).unwrap();
        let ahead = LastRequest {
            host: HOST.to_owned(),
            at: now + chrono::Duration::days(1),
        };
        std::fs::write(store.path_of(HOST), serde_json::to_vec(&ahead).unwrap()).unwrap();
        let ruling = store.take_turn(HOST, Duration::from_secs(2), WAIT_BUDGET, now, Pace::Hosted);
        let recovered = ruling.recovered.expect("a recovered record");
        assert!(
            recovered.contains("clock has moved backwards"),
            "{recovered}"
        );
        assert!(!recovered.contains("T"), "an instant is named: {recovered}");
    }

    #[test]
    fn the_whole_call_ceiling_is_under_the_hosts_own_tool_call_timeout() {
        // The stdio host measured for this ends a call at 300 seconds, and a
        // streamable-HTTP one at 60 seconds by default.
        assert!(CALL_CEILING < Duration::from_secs(300));
        assert!(HOSTED_WAIT_BUDGET < Duration::from_secs(60));
        let home = tempfile::tempdir().unwrap();
        let pacing = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Own);
        assert!(!pacing.over_ceiling());
        assert!(pacing.left() <= CALL_CEILING);
        assert!(pacing.left() > CALL_CEILING - Duration::from_secs(5));

        // A call past its ceiling sends no further probe, to a host with a
        // delay or without one, and has nothing left to sleep.
        let mut spent = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Own);
        spent.ceiling = Duration::ZERO;
        assert!(spent.over_ceiling());
        assert_eq!(spent.spendable(), Duration::ZERO);
        let refused = spent
            .probe("https://unpaced.example/licence.xml", Utc::now())
            .expect_err("nothing is sent past the ceiling");
        assert!(
            refused.contains("one `context_fetch` may take"),
            "{refused}"
        );
        assert!(refused.contains("time limit"), "{refused}");
        assert_eq!(spent.request_timeout(Duration::from_secs(30)), None);
    }

    #[test]
    fn a_hosted_call_is_limited_under_the_http_timeout_and_no_request_outlasts_it() {
        // The streamable-HTTP host measured aborts a request at 60 seconds.
        assert_eq!(Pace::Hosted.call_ceiling(), HOSTED_CALL_CEILING);
        assert!(HOSTED_CALL_CEILING < Duration::from_secs(60));
        assert!(HOSTED_WAIT_BUDGET < HOSTED_CALL_CEILING);
        assert_eq!(Pace::Own.call_ceiling(), CALL_CEILING);
        let home = tempfile::tempdir().unwrap();
        let hosted = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Hosted);
        assert!(hosted.left() <= HOSTED_CALL_CEILING);
        // A request is given its own timeout while the call has longer left,
        // and only what is left once the call has less.
        assert_eq!(
            hosted.request_timeout(Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        );
        let timeout = hosted
            .request_timeout(Duration::from_secs(300))
            .expect("time left");
        assert!(timeout <= HOSTED_CALL_CEILING, "{timeout:?}");

        let mut spent = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Hosted);
        spent.ceiling = Duration::ZERO;
        let refused = spent
            .probe("https://unpaced.example/licence.xml", Utc::now())
            .expect_err("nothing is sent past the limit");
        assert!(refused.contains("reached its time limit"), "{refused}");
        assert!(refused.contains("50s"), "{refused}");
        assert!(refused.contains("over HTTP"), "{refused}");
        assert!(!refused.contains("timeout"), "{refused}");
    }

    #[test]
    fn a_probe_that_must_leave_the_pages_turn_is_not_sent_where_that_does_not_fit() {
        let home = tempfile::tempdir().unwrap();
        let pacing = Pacing::with_budget(
            CrawlDelayStore::open(home.path()),
            Duration::from_secs(10),
            Pace::Own,
        );
        pacing.learn(HOST, 30_000);
        let now = Utc::now();
        assert_eq!(pacing.pending_wait(HOST, now), Duration::ZERO);
        let refused = pacing
            .probe_reserving(
                "https://publisher.example/licence.xml",
                now,
                Duration::from_secs(30),
            )
            .expect_err("the page's turn does not fit");
        assert!(refused.contains("still to come"), "{refused}");
        assert!(
            !pacing.store.path_of(HOST).exists(),
            "no turn was written for a probe that was not sent"
        );
        // With the reserve inside the budget, the probe takes the clear turn
        // and the host is then owed a whole delay.
        assert_eq!(
            pacing.probe_reserving(
                "https://publisher.example/licence.xml",
                now,
                Duration::from_secs(5)
            ),
            Ok(now)
        );
        assert_eq!(pacing.pending_wait(HOST, now), Duration::from_secs(30));
    }

    #[test]
    fn a_probe_is_paced_by_the_delay_read_for_its_host_and_says_when_it_was_sent() {
        let home = tempfile::tempdir().unwrap();
        let pacing = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Own);
        let now = Utc::now();
        // No delay has been read for this host, so nothing is paced and the
        // probe is sent at once.
        assert_eq!(
            pacing.probe("https://publisher.example/a.json", now),
            Ok(now)
        );
        pacing.learn("publisher.example", 60_000);
        // The first turn is clear, so the probe is still sent at `now`.
        assert_eq!(
            pacing.probe("https://publisher.example/a.json", now),
            Ok(now)
        );
        let spent = Pacing::with_budget(
            CrawlDelayStore::open(home.path()),
            Duration::ZERO,
            Pace::Own,
        );
        spent.learn("publisher.example", 60_000);
        let refused = spent
            .probe("https://publisher.example/b.json", now)
            .expect_err("the wait does not fit");
        assert!(refused.starts_with("not sent: "), "{refused}");
        assert!(refused.contains("Crawl-delay: 60"), "{refused}");
    }

    #[test]
    fn a_free_probe_takes_only_a_turn_that_costs_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let pacing = Pacing::new(store.clone(), Pace::Own);
        pacing.learn("publisher.example", 45_000);
        // Nothing has asked this host, so the free turn is there to take.
        assert_eq!(
            pacing.free_probe("https://publisher.example/m.json", now),
            Ok(now)
        );
        // The turn it took is now the host's, so the next free probe is not
        // sent although the wait would have fitted the budget.
        let refused = pacing
            .free_probe("https://publisher.example/m.json", now)
            .expect_err("the host is inside its delay");
        assert!(refused.contains("finds it clear"), "{refused}");
        // The budget was not spent on it.
        assert_eq!(pacing.spendable(), pacing.left().min(WAIT_BUDGET));
    }

    #[test]
    fn seconds_are_written_without_trailing_zeros() {
        assert_eq!(seconds(2_000), "2");
        assert_eq!(seconds(1_500), "1.5");
        assert_eq!(seconds(250), "0.25");
        assert_eq!(seconds(60_000), "60");
    }
}
