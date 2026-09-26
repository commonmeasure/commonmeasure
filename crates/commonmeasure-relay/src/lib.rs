//! The Content Telemetry relay: the one place the operator record crosses the
//! machine boundary, and only ever as a purpose-limited projection.
//!
//! The private operator record and the projection are different artefacts and
//! different types (`commonmeasure_harness::Crossing` and friends on one side,
//! [`wire::WireBatch`] on the other); nothing here can append to an evidence log.
//! The pipeline is: project witnessed evidence (`project`), queue durably
//! (`spool`), deliver to the configured receiver (`client`), and account for
//! what was actually accepted (`state`). No configured receiver means the
//! pipeline never starts: there is no default egress destination.

pub mod client;
pub mod config;
pub mod enrolment;
pub mod instance_registration;
pub mod project;
pub mod spool;
pub mod state;
pub mod supplier_credentials;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use config::RelayConfig;
use spool::{Spool, SpoolEntry};
use state::RelayState;

pub use enrolment::{
    ConnectReport, DisconnectReport, EnrolmentCheck, ManagedPin, PolicySigner, ProofAction,
    ProofRefresh, SIGNER_PATH, Standing, check_standing, connect, disconnect,
    refresh_directory_proof, refresh_directory_proof_if_due,
};
pub use state::{egress_report, enrolment_error_line, refused_spool_line};

#[derive(Debug, Default)]
pub struct RelayOptions {
    /// Forecast the projection and pending delivery against the policy and
    /// approvals on disk, without network or writes. A real run syncs managed
    /// policy and reporting approvals first, so over a home where either moves
    /// the forecast can differ from the run.
    pub dry_run: bool,
    /// Draft policy to forecast; valid only with `dry_run`.
    pub policy: Option<PathBuf>,
    /// Receiver base URL; overrides `relay.json` for this invocation.
    pub receiver: Option<String>,
    /// API key for the receiver this invocation delivers to; overrides
    /// `relay.json` for it. The key in `relay.json` is sent only to the origin
    /// of the receiver in `relay.json`, so another `receiver` gets this key or
    /// none.
    pub api_key: Option<String>,
    /// Published run directories to project alongside the session logs.
    pub runs: Vec<PathBuf>,
    /// Session ids to project; every recorded session when empty.
    pub sessions: Vec<String>,
}

/// Under whose clearance a delivered event left the machine.
///
/// Only the first variant is a clearance an operator declared. The other two
/// are absences, and they are different absences: a published run has no
/// engagement to be cleared by, while an event spooled by an earlier
/// invocation was cleared by something this run never resolved. Neither is a
/// zero and neither is the other (`docs/FAIL-POLICY.md` §7).
///
/// Ordering is the declaration order below, so a report reads declared
/// clearances first, alphabetically, and the absences last.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Clearance {
    /// The governing engagement declared on the policy scope that matched the
    /// crossing's recorded working directory — the only engagement identity an
    /// enforcing seam reads, and the one under which these events were cleared
    /// to leave.
    ///
    /// Never the reported engagement the console counts this work under: that
    /// is resolved at read time from the sink's attribution rules, which this
    /// crate cannot read and must not, because attribution may not feed
    /// enforcement (`docs/contracts/session-evidence.md` §Importing history).
    GoverningEngagement(String),
    /// A published run passed with `--run`. Runs are outside the
    /// session-engagement filter because the run contract carries no
    /// engagement, so nothing named cleared these events.
    RunWithoutEngagement,
    /// Spooled by an earlier invocation and delivered by this one. The
    /// clearance it was projected under was resolved then, against a policy and
    /// a session log this run did not re-read: unknown here, not absent.
    SpooledBeforeThisRun,
}

impl fmt::Display for Clearance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GoverningEngagement(name) => write!(formatter, "governing engagement {name}"),
            Self::RunWithoutEngagement => {
                write!(formatter, "no engagement: a published run carries none")
            }
            Self::SpooledBeforeThisRun => write!(
                formatter,
                "a clearance this run did not resolve: spooled by an earlier run"
            ),
        }
    }
}

/// How many delivered events left under one clearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredUnderClearance {
    pub clearance: Clearance,
    pub events: u64,
}

#[derive(Debug)]
pub struct RelayReport {
    /// Counts describe projected delivery, not acknowledgements, when true.
    pub dry_run: bool,
    /// Distinct outgoing source hosts, held only in memory for a dry run.
    pub hosts: BTreeSet<String>,
    pub receiver: String,
    /// Session logs read this invocation. `sessions_projected` and
    /// `sessions_withheld` are disjoint subsets of it; the remainder held
    /// nothing eligible to project. A log that did not read is not counted
    /// here; [`UnreadableSessions`] names it.
    pub sessions_read: usize,
    pub sessions_projected: usize,
    /// Sessions holding witnessed crossings of which not one was cleared to
    /// leave. An absence to state rather than a row to omit: an operator whose
    /// work stayed home should read that it did.
    pub sessions_withheld: usize,
    /// Sessions cleared to leave whose crossings fall under operator terms
    /// that require the institution's identifiers on the session
    /// (`access_context`, standard section 5.1.3). The event-batch envelope
    /// this relay delivers has no place for that container, and a delivery
    /// whose required context cannot be established is withheld rather than
    /// sent without it. Counted apart from `sessions_withheld`, because the
    /// operator cleared these and the wire could not carry them.
    pub sessions_withheld_access_context: usize,
    pub runs_projected: usize,
    /// The refused counts put on the wire this run: the running total of
    /// each session for which a batch was enqueued, whether it carried new
    /// events or only a moved count. A session whose count the receiver
    /// already holds contributes nothing. The URLs and reasons stay home
    /// (`docs/contracts/telemetry-projection.md` §The refused count on the wire).
    pub refused_reported: u64,
    pub events_enqueued: u64,
    /// Undelivered batches retained after this invocation.
    pub batches_queued: u64,
    pub batches_dead: u64,
    pub batches_delivered: u64,
    pub events_delivered: u64,
    /// `events_delivered` split by the event's wire `type`
    /// (`content_retrieved`, `content_grounded`, `turn_started`,
    /// `turn_completed`). Counted from the documents as posted, or in a dry
    /// run as they would be posted, turn boundaries included (EGR-25). It
    /// covers batches spooled by earlier runs as well as those projected by
    /// this one.
    ///
    /// A forecast is taken against the policy and the reporting approvals on
    /// disk. It is not a promise about the next real run: that run syncs
    /// managed policy and refreshes reporting approvals before it projects
    /// ([`relay_with_clock`]), and either can change which crossings are
    /// cleared. The counts match a real run over an unchanged home.
    pub events_by_type: BTreeMap<String, u64>,
    /// Events the receiver newly recorded. Lower than `events_delivered` on a
    /// redelivery, and that difference is the idempotency working. Unknown
    /// during a dry run because no receiver has acknowledged the events, and
    /// when any batch was accepted without a stated `events_created` count
    /// ([`client::Acceptance`]).
    pub events_new_at_receiver: Option<u64>,
    /// Delivered events that were queued with an `instance` reference and
    /// posted without it, because this receiver did not issue the instance
    /// (`project::withhold_unissued_instances`). Delivery state is by event
    /// id whatever the receiver, so these events are recorded delivered and
    /// the issuer is not sent them by a later run: a reporting duty of that
    /// instance reads nothing for them (EGR-06). Counts only members a spooled
    /// batch held; a batch projected for this receiver never carried one.
    pub instance_references_withheld: u64,
    /// `events_delivered` split by the clearance each event left under,
    /// ordered by [`Clearance`]. A report of what happened: it is computed from
    /// the clearance decisions after they are taken and is never an input to
    /// them.
    pub delivered_by_clearance: Vec<DeliveredUnderClearance>,
    /// The enrolled key's standing as the hub answered it this run, or why
    /// it could not; `None` for an edge that is not enrolled. A revocation
    /// learnt here is recorded on the enrolment record before delivery, so
    /// the next session's evidence names it.
    pub standing: Option<enrolment::Standing>,
    /// What keeping the enrolled key listed in the key directory did this
    /// run; `None` for an edge that is not enrolled or whose standing the hub
    /// did not confirm.
    pub directory_proof: Option<enrolment::ProofRefresh>,
}

/// A delivery the receiver refused or that never reached it, with the key's
/// standing as the hub answered it this run. The two are one fact to the
/// operator: a receiver answering 401 to an edge whose key the hub has
/// revoked is the revocation, and the standing line says so before the
/// failure does. The spool keeps every undelivered batch.
#[derive(Debug)]
pub struct DeliveryFailure {
    pub standing: Option<enrolment::Standing>,
    pub directory_proof: Option<enrolment::ProofRefresh>,
    pub receiver: String,
    pub spool: PathBuf,
    pub cause: anyhow::Error,
    /// [`RelayReport::instance_references_withheld`] for the batches this
    /// run delivered before or after the one that failed. Their events are
    /// recorded delivered whatever happened to the rest of the run, so the
    /// failure says so as the report would have (EGR-07).
    pub instance_references_withheld: u64,
    /// Sessions this run skipped because their logs did not read. The run
    /// would have failed for them had every delivery succeeded, so the
    /// failure names them as well.
    pub unreadable_sessions: Vec<UnreadableSession>,
}

/// A session this run skipped because its log did not read. Nothing of it
/// was projected this run. A batch an earlier run spooled from it is
/// delivered like any other unless directory selection applies to the batch:
/// the recheck before delivery reads the log again, so that batch stays
/// queued and unsent and is counted in `batches_held`.
#[derive(Debug)]
pub struct UnreadableSession {
    /// The log's file stem, which is the session id the operator passes to
    /// `--session`.
    pub session: String,
    pub path: PathBuf,
    pub cause: anyhow::Error,
    /// Due batches spooled from this session that this run left queued,
    /// because permission to send them is rechecked against the log and the
    /// log did not read.
    pub batches_held: u64,
}

impl fmt::Display for UnreadableSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "session {} was skipped: {} did not read: {:#}",
            self.session,
            self.path.display(),
            self.cause
        )?;
        if self.batches_held > 0 {
            write!(
                formatter,
                "; {} batch{} spooled from it earlier stay{} queued and unsent, because directory \
                 consent is rechecked against the log before a batch leaves",
                self.batches_held,
                if self.batches_held == 1 { "" } else { "es" },
                if self.batches_held == 1 { "s" } else { "" },
            )?;
        }
        Ok(())
    }
}

/// The hold recorded on a queued batch whose origin log did not read at the
/// directory recheck. A hold consumes no attempt, and the next claim clears
/// it, so a repaired log lets the batch leave on the following run.
const UNREADABLE_ORIGIN_HOLD: &str = "Held: the session log this batch was projected from does \
                                      not read, so directory consent cannot be rechecked; \
                                      undelivered.";

/// A run that skipped one or more sessions whose logs did not read and
/// completed for every other session. One damaged log does not hold back
/// the rest of the home (NET-06): the other sessions were projected, and
/// every due batch was delivered, those spooled before this run included.
/// The exception is a batch spooled from a skipped session while directory
/// selection applies to it ([`UnreadableSession::batches_held`]).
/// The run still fails, so a caller that reads only the outcome learns that
/// a session's evidence is not leaving.
#[derive(Debug)]
pub struct UnreadableSessions {
    pub sessions: Vec<UnreadableSession>,
    /// What the run did for the sessions that read. `sessions_read` leaves
    /// the skipped sessions out.
    pub report: RelayReport,
}

impl fmt::Display for UnreadableSessions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&unreadable_sessions_text(&self.sessions))
    }
}

impl std::error::Error for UnreadableSessions {}

/// One line per skipped session, under a line that says the rest of the run
/// went ahead. Shared with [`DeliveryFailure`], so it claims nothing about
/// what was delivered.
fn unreadable_sessions_text(sessions: &[UnreadableSession]) -> String {
    let (plural, pronoun) = if sessions.len() == 1 {
        ("", "it")
    } else {
        ("s", "them")
    };
    let mut text = format!(
        "{} session log{plural} did not read; nothing of {pronoun} was projected and the run \
         went on without {pronoun}",
        sessions.len()
    );
    for session in sessions {
        text.push_str(&format!("\n{session}"));
    }
    text
}

impl fmt::Display for DeliveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(standing) = &self.standing {
            writeln!(formatter, "{standing}")?;
        }
        if let Some(directory_proof) = &self.directory_proof {
            writeln!(formatter, "{directory_proof}")?;
        }
        formatter.write_str(&self.delivery_text())
    }
}

impl DeliveryFailure {
    /// The failure alone, for a caller that has already printed the
    /// standing on its own line.
    pub fn delivery_text(&self) -> String {
        let failure = format!(
            "delivery to {} failed; undelivered batches remain spooled under {}: {:#}",
            self.receiver,
            self.spool.display(),
            self.cause
        );
        let mut text = failure;
        if self.instance_references_withheld > 0 {
            text.push('\n');
            // The failure line above names no delivered events, so the
            // warning says which events it counts.
            let count = self.instance_references_withheld;
            let subject = if count == 1 {
                "1 event this run delivered was".to_owned()
            } else {
                format!("{count} events this run delivered were")
            };
            text.push_str(&withheld_references_text(&subject, &self.receiver));
        }
        if !self.unreadable_sessions.is_empty() {
            text.push('\n');
            text.push_str(&unreadable_sessions_text(&self.unreadable_sessions));
        }
        text
    }
}

/// What the operator is told when delivered events lost their `instance`
/// reference because `receiver` did not issue the instance. A re-targeted
/// batch burns its event ids against the receiver that accepted it, so the
/// hub that issued the instance never reads them.
///
/// The text follows the line that counts the delivered events, which is what
/// "those events" refers to. A failed run prints no such line;
/// [`DeliveryFailure::delivery_text`] names the events itself.
pub fn withheld_references_warning(count: u64, receiver: &str) -> String {
    withheld_references_text(&format!("{count} of those events were"), receiver)
}

fn withheld_references_text(subject: &str, receiver: &str) -> String {
    format!(
        "warning: {subject} queued with an instance reference that \
         {receiver} did not issue. The reference was withheld and the events are recorded \
         delivered; the issuing hub is not sent them afterwards, so its reporting duties read \
         nothing for them"
    )
}

impl std::error::Error for DeliveryFailure {}

/// The API key that may accompany a request to `target`, if this run holds one.
///
/// A key belongs to the receiver it was given with: `--api-key` to the
/// receiver this run delivers to, the key in `relay.json` to the receiver in
/// `relay.json`. It is sent only to that receiver's origin, compared as the
/// instance issuer is compared (`project::receiver_origin`). `relay.json`
/// holds the enrolled hub's ingest key, so `--receiver <other>` without
/// `--api-key` delivers with no key rather than handing the hub's key to a
/// receiver that did not issue it (EGR-05). A URL without a host-based origin
/// matches nothing.
fn key_for<'a>(
    target: &str,
    delivering_to: &str,
    options: &'a RelayOptions,
    configured: Option<&'a RelayConfig>,
) -> Option<&'a str> {
    let origin = project::receiver_origin(Some(target));
    let same_origin =
        |other: &str| origin.is_some() && project::receiver_origin(Some(other)) == origin;
    if let Some(key) = options.api_key.as_deref()
        && (target == delivering_to || same_origin(delivering_to))
    {
        return Some(key);
    }
    configured
        .filter(|config| same_origin(&config.receiver))
        .and_then(|config| config.api_key.as_deref())
}

/// Project, spool and deliver. Everything before the receiver is named fails
/// without touching disk or network: no configured receiver means no egress
/// and no relay state.
///
/// Two failures arrive after the run has done its work, and each carries
/// what was done: [`DeliveryFailure`] when a batch did not reach the
/// receiver, and [`UnreadableSessions`] when every delivery succeeded and a
/// session log was skipped because it did not read.
pub fn relay(home: &Path, options: &RelayOptions) -> Result<RelayReport> {
    relay_with_clock(home, options, &Utc::now)
}

/// Run the production relay using an explicit UTC clock. The service and CLI
/// use [`relay`]; deterministic cadence tests advance this clock without sleeps.
pub fn relay_with_clock(
    home: &Path,
    options: &RelayOptions,
    now: &dyn Fn() -> DateTime<Utc>,
) -> Result<RelayReport> {
    anyhow::ensure!(
        options.dry_run || options.policy.is_none(),
        "--policy requires --dry-run"
    );
    if !options.dry_run
        && let Some(record) = commonmeasure_harness::EnrolmentRecord::load(home)
            .map_err(|reason| anyhow::anyhow!(reason))?
    {
        commonmeasure_harness::enrolment::hub_url_accepted(&record.hub)
            .map_err(|reason| anyhow::anyhow!(reason))?;
    }
    let configured = RelayConfig::load(home).map_err(|error| anyhow::anyhow!(error))?;
    let receiver = options
        .receiver
        .clone()
        .or_else(|| configured.as_ref().map(|config| config.receiver.clone()));
    let Some(receiver) = receiver else {
        bail!(
            "no telemetry receiver is configured: pass --receiver or set one in {}; \
             nothing was projected and nothing was sent",
            home.join("relay.json").display()
        );
    };
    let api_key = key_for(&receiver, &receiver, options, configured.as_ref());
    // A supplier scope belongs to the receiver relay.json names, however the
    // override spells it; a `--receiver` override to another endpoint is not
    // scoped by it.
    let supplier_scope: Option<Vec<String>> = configured
        .as_ref()
        .filter(|config| config::same_receiver(&config.receiver, &receiver))
        .and_then(|config| config.suppliers.clone());

    // The same policy.json the capture paths read, held for the whole
    // invocation: every egress decision below is taken against these bytes, so
    // a policy edited while the relay runs cannot change its later answers.
    // A policy file that cannot be read is an error here, not a silently
    // unfiltered projection, and it is read before the spool is opened so a
    // home with a broken policy gains no relay state at all.
    let mut policy_document = match &options.policy {
        Some(path) => commonmeasure_harness::policy::PolicyDocument::read_file(path),
        None => commonmeasure_harness::policy::PolicyDocument::read(home),
    }
    .map_err(|error| anyhow::anyhow!("{error}; nothing was projected and nothing was sent"))?;

    // The spool lock is taken before any other work, so a second relay — two
    // sessions ending at the same moment, or a session end beside a typed
    // `commonmeasure relay` — loses it here and exits before it has asked
    // the hub for a standing, refreshed a reporting approval or written any
    // state. What the loser would have projected stays in the session logs
    // and is projected by the next run.
    //
    // What has already left, or is spooled to leave, is read before
    // projection: it decides which events are new, and whether a session's
    // refused count has moved since a receiver last accepted one.
    let (spool, state) = if options.dry_run {
        (Spool::read_only(home), RelayState::read_only(home))
    } else {
        (Spool::open(home)?, RelayState::open(home)?)
    };

    // The enrolled key's standing, asked before anything is delivered:
    // a revocation is learnt on the next relay run, which is this one. The
    // same answer keeps a standing key's directory proof current. A forecast
    // checks neither, because both can change local state.
    let (standing, directory_proof) = if options.dry_run {
        (None, None)
    } else {
        // The standing is asked of the enrolled hub, which need not be the
        // receiver of this run, so the key is resolved for the hub's origin.
        let hub_key = commonmeasure_harness::EnrolmentRecord::load(home)
            .ok()
            .flatten()
            .and_then(|record| key_for(&record.hub, &receiver, options, configured.as_ref()));
        match enrolment::check_standing(home, hub_key)? {
            Some(check) => (Some(check.standing), check.directory_proof),
            None => (None, None),
        }
    };

    // Reporting approvals decide which sessions a directory-selected home may
    // project, so they are refreshed before projection, and the policy read
    // above takes them up: it holds the approvals as they were before this
    // refresh, and an approval the hub renewed after expiry would otherwise
    // clear nothing until the next run (EGR-49). A named policy file carries
    // no directory selection. A forecast refreshes nothing and reads the
    // approvals as they are cached.
    if !options.dry_run {
        if let Err(error) = commonmeasure_harness::directory::sync(home) {
            eprintln!(
                "commonmeasure: reporting approval refresh failed; cached approvals retain their original expiry: {error}"
            );
        }
        if options.policy.is_none() {
            policy_document
                .reread_selection(home)
                .map_err(anyhow::Error::msg)?;
        }
    }
    let directory_selection = commonmeasure_harness::directory::Registry::read(home)
        .map_err(anyhow::Error::msg)?
        .is_some();
    // The operator's named internal prefixes. At capture they lower the
    // privacy floor; at egress they are an exclusion — internal crossings are
    // operator-record only.
    let internal_prefixes: Vec<String> = policy_document.resolve(None).internal_prefixes().to_vec();

    let mut agents = state.session_agents()?;
    // A session is pinned at its first delivery, so a batch queued by an
    // earlier run and not yet sent carries the only copy of its agent id.
    for (_, entry) in spool.entries()? {
        if let (Some(session), Some(agent)) = (
            wire_session(&entry.document),
            entry.document["agent_id"].as_str(),
        ) {
            agents.entry(session).or_insert_with(|| agent.to_owned());
        }
    }
    let mut pending = spool.pending()?;
    let mut already: HashSet<Uuid> = state.delivered()?;
    let mut refused_known: HashMap<Uuid, u64> = state.refused_delivered()?;
    for (_, entry) in &pending {
        already.extend(event_ids(&entry.document));
        if let (Some(session), Some(refused)) = (
            wire_session(&entry.document),
            entry.document["refused"].as_u64(),
        ) {
            let known = refused_known.entry(session).or_default();
            *known = (*known).max(refused);
        }
    }

    // Project. `clearance_of` accumulates what the report says at the end: the
    // clearance each projected event left under, recorded as the decision is
    // taken and read only after delivery.
    let mut batches = Vec::new();
    // Batches enqueued for a moved refused count alone: one already-delivered
    // event under its own id, carrying the session's new count, which the
    // receiver max-merges. The count travels only on a batch, and a refusal
    // after a session's last admitted crossing would otherwise never leave.
    let mut carriers = Vec::new();
    let mut clearance_of: HashMap<Uuid, Clearance> = HashMap::new();
    let session_logs: Vec<PathBuf> = if options.sessions.is_empty() {
        commonmeasure_harness::SessionLog::list(home).context("list sessions")?
    } else {
        options
            .sessions
            .iter()
            .map(|id| {
                let path = home.join("sessions").join(format!("{id}.ndjson"));
                if path.exists() {
                    Ok(path)
                } else {
                    Err(anyhow::anyhow!("no session {id} at {}", path.display()))
                }
            })
            .collect::<Result<_>>()?
    };
    let mut sessions_projected = 0usize;
    let mut sessions_withheld = 0usize;
    let mut sessions_withheld_access_context = 0usize;
    let mut refused_reported = 0u64;
    // A log that does not read is skipped and named when the run ends. The
    // other sessions of the home, and the batches already spooled, do not
    // wait on it: a reporting duty that depends on them would otherwise stay
    // outstanding for as long as one log is damaged (NET-06).
    let mut unreadable_sessions = Vec::new();
    for path in &session_logs {
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("session log without a readable name")?;
        let records = match commonmeasure_harness::SessionLog::read(path) {
            Ok(records) => records,
            Err(cause) => {
                unreadable_sessions.push(UnreadableSession {
                    session: session_id.to_owned(),
                    path: path.clone(),
                    cause: cause.into(),
                    batches_held: 0,
                });
                continue;
            }
        };
        let cleared = egress_clearances(&policy_document, &records);
        if !cleared
            .keys()
            .any(|&position| project::is_witnessed(&records[position]))
        {
            // Nothing here may leave. A session that had crossings to clear and
            // cleared none is the case worth counting; one that never witnessed
            // a crossing is not withheld, it is empty. A cleared refusal alone
            // sends nothing either: its count travels only on a batch.
            if records.iter().any(project::is_witnessed) {
                sessions_withheld += 1;
            }
            continue;
        }
        if let Some(reference) = access_context_required(&policy_document, &records, &cleared) {
            // The terms the operator declared for a host in this session
            // require the institution's identifiers on the session document.
            // An event batch carries no session `data`, so the delivery
            // format cannot state them; the session stays home and the
            // report says which terms held it.
            eprintln!(
                "commonmeasure: session {session_id} withheld: the terms {reference} require \
                 access_context on the session, and the event-batch delivery format carries no \
                 session data; delivery as a session document waits on the receiver"
            );
            sessions_withheld_access_context += 1;
            continue;
        }
        let mut projected = project::project_session(
            Some(&receiver),
            session_id,
            &records,
            &internal_prefixes,
            &|at| cleared.contains_key(&at),
        );
        if let Some(suppliers) = &supplier_scope {
            project::scope_to_suppliers(&mut projected, suppliers);
        }
        for (event, position) in &projected.event_positions {
            // Every projected event comes from a record this filter cleared,
            // and a crossing is cleared only under a named governing
            // engagement, so the lookup cannot miss.
            if let Some(engagement) = cleared.get(position) {
                clearance_of.insert(
                    *event,
                    Clearance::GoverningEngagement(engagement.to_owned()),
                );
            }
        }
        if !projected.batches.is_empty() {
            sessions_projected += 1;
            let wire_session = projected.batches[0].session_id;
            let has_new = projected
                .batches
                .iter()
                .flat_map(|batch| &batch.events)
                .any(|event| !already.contains(&event.id));
            // A scoped projection has no count, so it reports none and
            // needs no carrier.
            if let Some(refused) = projected.refused {
                if has_new {
                    refused_reported += refused;
                } else if refused > refused_known.get(&wire_session).copied().unwrap_or(0) {
                    let last = projected.batches.last().expect("a non-empty projection");
                    let mut carrier = last.clone();
                    carrier.events = vec![last.events.last().expect("a batch has events").clone()];
                    carrier.refused = Some(refused);
                    carriers.push((path.display().to_string(), carrier));
                    refused_reported += refused;
                }
            }
        }
        batches.extend(
            projected
                .batches
                .into_iter()
                .map(|batch| (path.display().to_string(), batch)),
        );
    }
    // The delivery loop can add a session this loop never opened, so the
    // count of logs read is taken here.
    let sessions_read = session_logs.len() - unreadable_sessions.len();
    let mut runs_projected = 0usize;
    for run_dir in &options.runs {
        let summary_path = run_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&summary_path)
                .with_context(|| format!("read {}", summary_path.display()))?,
        )
        .with_context(|| format!("{} is not valid JSON", summary_path.display()))?;
        let mut projected = project::project_run(&summary, &internal_prefixes)
            .with_context(|| format!("project {}", run_dir.display()))?;
        if let Some(suppliers) = &supplier_scope {
            project::retain_supplied(&mut projected, suppliers);
        }
        if !projected.is_empty() {
            runs_projected += 1;
        }
        for event in projected.iter().flat_map(|batch| &batch.events) {
            clearance_of.insert(event.id, Clearance::RunWithoutEngagement);
        }
        batches.extend(
            projected
                .into_iter()
                .map(|batch| (run_dir.display().to_string(), batch)),
        );
    }

    // Queue what has not already been delivered or spooled. Identity does the
    // filtering, and it holds only because an event id is derived from its
    // record's position in the log as read: the same crossing keeps the same
    // id whatever the policy says about the crossings around it, so a re-run
    // enqueues what is genuinely new and nothing else — including a re-run
    // after the operator changed which engagements are cleared.
    let mut events_enqueued = 0u64;
    for (origin, mut batch) in batches {
        batch.events.retain(|event| !already.contains(&event.id));
        if batch.events.is_empty() {
            continue;
        }
        already.extend(batch.events.iter().map(|event| event.id));
        events_enqueued += batch.events.len() as u64;
        batch.agent_id = agents
            .entry(batch.session_id)
            .or_insert(batch.agent_id)
            .clone();
        let entry = SpoolEntry {
            origin,
            queued_at: Some(now()),
            document: serde_json::to_value(&batch).context("serialise batch")?,
            directory_selection,
        };
        if options.dry_run {
            pending.push((u64::MAX, entry));
        } else {
            spool.enqueue(&entry)?;
        }
    }
    // A carrier's one event is on the receiver's record by construction;
    // the batch is enqueued for the count it carries, and counts as no new
    // event.
    for (origin, mut carrier) in carriers {
        carrier.agent_id = agents
            .entry(carrier.session_id)
            .or_insert(carrier.agent_id)
            .clone();
        let entry = SpoolEntry {
            origin,
            queued_at: Some(now()),
            document: serde_json::to_value(&carrier).context("serialise batch")?,
            directory_selection,
        };
        if options.dry_run {
            pending.push((u64::MAX, entry));
        } else {
            spool.enqueue(&entry)?;
        }
    }
    if !options.dry_run {
        pending = spool.pending()?;
    }

    // A failed or dead batch must not prevent another due batch from leaving.
    let mut first_failure = None;
    let delivery_states = spool.delivery_states()?;
    let mut batches_delivered = 0u64;
    let mut events_delivered = 0u64;
    // One batch accepted without a stated count makes the run's total
    // unknown: a partial sum would read as the whole.
    let mut events_new_at_receiver = Some(0u64);
    let mut instance_references_withheld = 0u64;
    let mut delivered_by_clearance: BTreeMap<Clearance, u64> = BTreeMap::new();
    let mut events_by_type: BTreeMap<String, u64> = BTreeMap::new();
    let mut hosts = BTreeSet::new();
    for (index, mut entry) in pending {
        if let Some(delivery) = delivery_states.get(&index) {
            if delivery.status == spool::DeliveryStatus::Dead
                || delivery.next_attempt_at.is_some_and(|at| at > now())
            {
                continue;
            }
            if delivery.attempts >= spool::MAX_ATTEMPTS {
                if !options.dry_run {
                    spool.claim(index, now())?;
                }
                continue;
            }
        }
        // A batch queued by 0.3.4 or earlier may carry consent provenance a
        // later prune filled in, so it is held whatever the consent in force.
        if entry.queued_at.is_none() {
            if !options.dry_run {
                spool.hold(index, spool::PRE_0_3_5_HOLD)?;
            }
            continue;
        }
        // Re-read consent, approval expiry and source policy before every delivery.
        // Hold selection's lock across the send: opt-out completes after any
        // already-running delivery, and every subsequent send sees the opt-out.
        let _consent = if options.dry_run {
            // A forecast sends nothing and must not create even a lock file.
            None
        } else {
            Some(
                commonmeasure_harness::declaration::lock(&home.join("directories.lock"))
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?,
            )
        };
        let selection_active = commonmeasure_harness::directory::Registry::read(home)
            .map_err(anyhow::Error::msg)?
            .is_some();
        if selection_active || entry.directory_selection {
            if let Recheck::OriginUnreadable(unreadable) =
                recheck_directory_batch(home, &mut entry)?
            {
                // The second strict read of the run. The batch cannot be
                // sent without it, and one damaged log must not stop the
                // batches behind this one (NET-06), so the batch stays
                // queued and its session is named with the others.
                if !options.dry_run {
                    spool.hold(index, UNREADABLE_ORIGIN_HOLD)?;
                }
                match unreadable_sessions
                    .iter_mut()
                    .find(|known| known.session == unreadable.session)
                {
                    Some(known) => known.batches_held += 1,
                    None => unreadable_sessions.push(unreadable),
                }
                continue;
            }
            if event_ids(&entry.document).is_empty() {
                if !options.dry_run {
                    spool.hold(
                        index,
                        "Held by current directory consent or source policy; undelivered.",
                    )?;
                }
                continue;
            }
        }
        if let Some(suppliers) = &supplier_scope {
            // After the directory recheck, which rewrites the refused count
            // from an unscoped projection of the origin log.
            project::scope_document(&mut entry.document, suppliers);
            if event_ids(&entry.document).is_empty() {
                if !options.dry_run {
                    spool.hold(
                        index,
                        "Held by the receiver's supplier scope: no event in it is from a listed supplier; undelivered.",
                    )?;
                }
                continue;
            }
        }
        if !options.dry_run && !spool.claim(index, now())? {
            continue;
        }
        let session = wire_session(&entry.document).context("spooled batch has no session id")?;
        let agent = agents
            .get(&session)
            .context("spooled batch has no agent id")?;
        entry.document["agent_id"] = serde_json::Value::String(agent.clone());
        // The batch may have been queued for a receiver other than this one.
        let references_withheld =
            project::withhold_unissued_instances(&mut entry.document, &receiver);
        project::strip_licence_userinfo(&mut entry.document);
        let (ids, by_type) = events_of(&entry.document);
        if options.dry_run {
            for event in entry.document["events"].as_array().into_iter().flatten() {
                if let Some(url) = event["content_url"].as_str() {
                    let host = commonmeasure_harness::grounding::host_of(url);
                    if !host.is_empty() {
                        hosts.insert(host);
                    }
                }
            }
        } else {
            state.pin_session_agent(session, agent)?;
            let acceptance = match client::deliver(&receiver, api_key, &entry.document) {
                Ok(acceptance) => acceptance,
                Err(error) => {
                    spool.record_failure(index, &format!("{error:#}"))?;
                    state.record_failure(&receiver, &format!("{error:#}"))?;
                    if first_failure.is_none() {
                        first_failure = Some(error);
                    }
                    continue;
                }
            };
            state.record_delivered(&ids)?;
            if let (Some(session), Some(refused)) = (
                wire_session(&entry.document),
                entry.document["refused"].as_u64(),
            ) {
                state.record_refused_delivered(session, refused)?;
            }
            spool.record_acceptance(index, &ids, references_withheld)?;
            events_new_at_receiver = events_new_at_receiver
                .zip(acceptance.events_created)
                .map(|(total, created)| total + created);
            state.record_success(&receiver)?;
        }
        batches_delivered += 1;
        events_delivered += ids.len() as u64;
        for (kind, count) in by_type {
            *events_by_type.entry(kind).or_default() += count;
        }
        instance_references_withheld += references_withheld;
        for id in &ids {
            let clearance = clearance_of
                .get(id)
                .cloned()
                .unwrap_or(Clearance::SpooledBeforeThisRun);
            *delivered_by_clearance.entry(clearance).or_default() += 1;
        }
    }

    if !options.dry_run {
        // The durable account would otherwise read healthy after a run that
        // delivered and skipped: `record_success` clears the last error.
        let examined: Option<Vec<&str>> = (!options.sessions.is_empty())
            .then(|| options.sessions.iter().map(String::as_str).collect());
        state.record_skipped_sessions(examined.as_deref(), &unreadable_sessions, now())?;
    }
    if let Some(cause) = first_failure {
        return Err(anyhow::Error::new(DeliveryFailure {
            standing,
            directory_proof,
            receiver,
            spool: home.join("relay").join("spool"),
            cause,
            instance_references_withheld,
            unreadable_sessions,
        }));
    }
    let states = spool.delivery_states()?;
    let batches_queued = states
        .values()
        .filter(|s| s.status == spool::DeliveryStatus::Queued)
        .count() as u64;
    let batches_dead = states
        .values()
        .filter(|s| s.status == spool::DeliveryStatus::Dead)
        .count() as u64;
    let report = RelayReport {
        batches_queued,
        batches_dead,
        dry_run: options.dry_run,
        hosts,
        receiver,
        sessions_read,
        sessions_projected,
        sessions_withheld,
        sessions_withheld_access_context,
        runs_projected,
        refused_reported,
        events_enqueued,
        batches_delivered,
        events_delivered,
        events_by_type,
        events_new_at_receiver: events_new_at_receiver.filter(|_| !options.dry_run),
        instance_references_withheld,
        delivered_by_clearance: delivered_by_clearance
            .into_iter()
            .map(|(clearance, events)| DeliveredUnderClearance { clearance, events })
            .collect(),
        standing,
        directory_proof,
    };
    if unreadable_sessions.is_empty() {
        Ok(report)
    } else {
        Err(anyhow::Error::new(UnreadableSessions {
            sessions: unreadable_sessions,
            report,
        }))
    }
}

/// Apply the declaration at the last boundary before projection. The governing
/// engagement remains a resolved property of the recorded cwd: it is never
/// copied into the private evidence merely to make egress convenient. Each
/// crossing and turn boundary is resolved separately because one host session
/// can traverse worktrees.
///
/// This reads the governing engagement — the one an enforcing seam may see. The
/// reported engagement the console counts this crossing under lives in the
/// sink's attribution rules, which this crate cannot read and must not
/// (`docs/contracts/session-evidence.md` §Importing history); the console is
/// where the two are compared.
///
/// Keys are the positions of the cleared crossings **in the log as read**,
/// witnessed and refused alike, and turn boundaries, not
/// a shorter list of the crossings themselves. Projection derives each event's
/// id from that position and the relay treats those ids as delivery identity
/// across runs, so a filtered list would renumber every surviving crossing:
/// already-delivered events would redeliver under new ids, and a crossing
/// cleared later would inherit an id already burned and be silently dropped as
/// a duplicate. Excluding by position is what keeps a crossing's identity
/// independent of what the policy says about its neighbours.
///
/// Each value is the governing engagement that cleared that crossing, kept
/// rather than discarded so the report can name whose clearance let the events
/// leave. It names the decision; it never takes part in it.
fn egress_clearances(
    policy: &commonmeasure_harness::policy::PolicyDocument,
    records: &[serde_json::Value],
) -> HashMap<usize, String> {
    records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            project::is_witnessed(record)
                || project::is_refused(record)
                || project::is_turn_boundary(record)
        })
        .filter_map(|(at, record)| {
            let cwd = if project::is_turn_boundary(record) {
                record["payload"]["detail"]["cwd"].as_str()
            } else {
                record["payload"]["cwd"].as_str()
            };
            let resolved = policy.resolve(cwd);
            // Clearance implies a named engagement: `allows_telemetry_egress`
            // is false without one, and the loader refuses a policy that clears
            // egress under no name.
            resolved
                .allows_telemetry_egress()
                .then(|| {
                    resolved
                        .governing_engagement()
                        .map(|name| (at, name.to_owned()))
                })
                .flatten()
        })
        .collect()
}

/// The reference of the first operator terms, among the cleared crossings of
/// one session, that name institution identifiers for the crossing's host.
/// Such terms require `access_context` on the session
/// (`docs/contracts/session-evidence.md` §Source declarations); `None` when no
/// cleared crossing falls under terms naming any.
fn access_context_required(
    policy: &commonmeasure_harness::policy::PolicyDocument,
    records: &[serde_json::Value],
    cleared: &HashMap<usize, String>,
) -> Option<String> {
    records
        .iter()
        .enumerate()
        .filter(|(at, record)| cleared.contains_key(at) && !project::is_turn_boundary(record))
        .find_map(|(_, record)| {
            let payload = &record["payload"];
            let resolved = policy.resolve(payload["cwd"].as_str());
            resolved
                .terms_for(payload["host_name"].as_str().unwrap_or_default())
                .filter(|terms| !terms.access_context.is_empty())
                .map(|terms| terms.reference.clone())
        })
}

/// The wire session a spooled batch belongs to.
fn wire_session(document: &serde_json::Value) -> Option<Uuid> {
    document["session_id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
}

fn event_ids(document: &serde_json::Value) -> Vec<Uuid> {
    events_of(document).0
}

/// A batch's event ids and how many of its events carry each wire `type`,
/// in one pass. The two are read from the same events under the same
/// validity filter, so a report cannot count a type for an event whose id it
/// did not count.
fn events_of(document: &serde_json::Value) -> (Vec<Uuid>, BTreeMap<String, u64>) {
    let mut ids = Vec::new();
    let mut by_type: BTreeMap<String, u64> = BTreeMap::new();
    for event in document["events"].as_array().into_iter().flatten() {
        let Some(id) = event["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()) else {
            continue;
        };
        ids.push(id);
        *by_type
            .entry(event["type"].as_str().unwrap_or("untyped").to_owned())
            .or_default() += 1;
    }
    (ids, by_type)
}

/// What [`recheck_directory_batch`] established about one queued batch.
enum Recheck {
    /// The batch now holds the events current permission lets leave, which
    /// may be none.
    Rechecked,
    /// The origin log did not read, so permission could not be established
    /// and the batch was left as it was.
    OriginUnreadable(UnreadableSession),
}

/// Re-project a queued session against current permission, preserving stable event
/// ids. Missing source evidence cannot authorise a previously queued disclosure.
fn recheck_directory_batch(home: &Path, entry: &mut SpoolEntry) -> Result<Recheck> {
    if commonmeasure_harness::directory::Registry::read(home)
        .map_err(anyhow::Error::msg)?
        .is_none()
    {
        // Provenance still requires consent even if both the registry and its
        // persistent mode marker have been lost. Scope clearances cannot
        // replace it. Keep the batch pending as well: acknowledging it would
        // allow its events to be projected again under scope clearances later.
        anyhow::bail!(
            "queued directory reporting requires local consent state; re-enrol the directory before retrying"
        );
    }
    let source = Path::new(&entry.origin);
    let session_directory = home
        .join("sessions")
        .canonicalize()
        .context("session evidence directory unavailable for queued reporting")?;
    let original = source
        .canonicalize()
        .context("queued reporting source evidence unavailable")?;
    if original.parent() != Some(session_directory.as_path()) {
        // Batch runs have no directory binding. Selecting directories does not
        // implicitly approve old run batches or arbitrary spool origins.
        entry.document["events"] = serde_json::json!([]);
        return Ok(Recheck::Rechecked);
    }
    let id = original
        .file_stem()
        .and_then(|s| s.to_str())
        .context("invalid session source")?;
    let records = match commonmeasure_harness::SessionLog::read(&original) {
        Ok(records) => records,
        Err(cause) => {
            return Ok(Recheck::OriginUnreadable(UnreadableSession {
                session: id.to_owned(),
                path: source.to_path_buf(),
                cause: cause.into(),
                batches_held: 1,
            }));
        }
    };
    let policy =
        commonmeasure_harness::policy::PolicyDocument::read(home).map_err(anyhow::Error::msg)?;
    let cleared = egress_clearances(&policy, &records);
    let prefixes = policy.resolve(None).internal_prefixes().to_vec();
    // Only the event ids and the refused count are read from this projection,
    // and neither depends on the receiver.
    let projected = project::project_session(None, id, &records, &prefixes, &|at| {
        cleared.contains_key(&at)
    });
    let mut permitted: HashSet<Uuid> = projected
        .event_positions
        .iter()
        .map(|(id, _)| *id)
        .collect();
    if access_context_required(&policy, &records, &cleared).is_some() {
        permitted.clear();
    }
    if let Some(events) = entry.document["events"].as_array_mut() {
        events.retain(|event| {
            event["id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .is_some_and(|id| permitted.contains(&id))
        });
    }
    entry.document["refused"] = serde_json::json!(projected.refused);
    Ok(Recheck::Rechecked)
}
