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
pub mod project;
pub mod spool;
pub mod state;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use config::RelayConfig;
use spool::{Spool, SpoolEntry};
use state::RelayState;

pub use enrolment::{
    ConnectReport, DisconnectReport, EnrolmentCheck, ManagedPin, PolicySigner, ProofAction,
    ProofRefresh, SIGNER_PATH, Standing, check_standing, connect, disconnect,
    refresh_directory_proof, refresh_directory_proof_if_due,
};
pub use state::egress_report;

#[derive(Debug, Default)]
pub struct RelayOptions {
    /// Forecast the same projection and pending delivery without network or writes.
    pub dry_run: bool,
    /// Draft policy to forecast; valid only with `dry_run`.
    pub policy: Option<PathBuf>,
    /// Receiver base URL; overrides `relay.json` for this invocation.
    pub receiver: Option<String>,
    /// API key; overrides `relay.json` for this invocation.
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
    /// crate cannot read and must not (`DECISIONS.md` §Session policy and egress).
    GoverningEngagement(String),
    /// A published run passed with `--run`. Runs are outside the
    /// session-engagement filter because the run contract carries no
    /// engagement, so nothing named cleared these events (`DECISIONS.md` §Session policy and egress).
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
    /// nothing eligible to project.
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
    /// (`docs/contracts/session-evidence.md` §The refused count on the wire).
    pub refused_reported: u64,
    pub events_enqueued: u64,
    pub batches_delivered: u64,
    pub events_delivered: u64,
    /// Events the receiver newly recorded. Lower than `events_delivered` on a
    /// redelivery, and that difference is the idempotency working. Unknown
    /// during a dry run because no receiver has acknowledged the events.
    pub events_new_at_receiver: Option<u64>,
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
        format!(
            "delivery to {} failed; undelivered batches remain spooled under {}: {:#}",
            self.receiver,
            self.spool.display(),
            self.cause
        )
    }
}

impl std::error::Error for DeliveryFailure {}

/// Project, spool and deliver. Everything before the receiver is named fails
/// without touching disk or network: no configured receiver means no egress
/// and no relay state.
pub fn relay(home: &Path, options: &RelayOptions) -> Result<RelayReport> {
    anyhow::ensure!(
        options.dry_run || options.policy.is_none(),
        "--policy requires --dry-run"
    );
    let configured = RelayConfig::load(home).map_err(|error| anyhow::anyhow!(error))?;
    let receiver = options
        .receiver
        .clone()
        .or_else(|| configured.as_ref().map(|config| config.receiver.clone()));
    let api_key = options.api_key.clone().or_else(|| {
        configured
            .as_ref()
            .and_then(|config| config.api_key.clone())
    });
    let Some(receiver) = receiver else {
        bail!(
            "no telemetry receiver is configured: pass --receiver or set one in {}; \
             nothing was projected and nothing was sent",
            home.join("relay.json").display()
        );
    };

    // The enrolled key's standing, asked before anything is delivered:
    // a revocation is learnt on the next relay run, which is this one. The
    // same answer keeps a standing key's directory proof current. A forecast
    // checks neither, because both can change local state.
    let (standing, directory_proof) = if options.dry_run {
        (None, None)
    } else {
        match enrolment::check_standing(home, api_key.as_deref())? {
            Some(check) => (Some(check.standing), check.directory_proof),
            None => (None, None),
        }
    };

    // The same policy.json the capture paths read, held for the whole
    // invocation: every egress decision below is taken against these bytes, so
    // a policy edited while the relay runs cannot change its later answers.
    // A policy file that cannot be read is an error here, not a silently
    // unfiltered projection.
    if !options.dry_run
        && let Err(error) = commonmeasure_harness::directory::sync(home)
    {
        eprintln!(
            "commonmeasure: directory grant refresh failed; cached grants retain their original expiry: {error}"
        );
    }
    let directory_selection = commonmeasure_harness::directory::Registry::read(home)
        .map_err(anyhow::Error::msg)?
        .is_some();
    let policy_document = match &options.policy {
        Some(path) => commonmeasure_harness::policy::PolicyDocument::read_file(path),
        None => commonmeasure_harness::policy::PolicyDocument::read(home),
    }
    .map_err(|error| anyhow::anyhow!("{error}; nothing was projected and nothing was sent"))?;
    // The operator's named internal prefixes. At capture they lower the
    // privacy floor; at egress they are an exclusion — internal crossings are
    // operator-record only.
    let internal_prefixes: Vec<String> = policy_document.resolve(None).internal_prefixes().to_vec();

    // What has already left, or is spooled to leave, is read before
    // projection: it decides which events are new, and whether a session's
    // refused count has moved since a receiver last accepted one.
    let (spool, state) = if options.dry_run {
        (Spool::read_only(home), RelayState::read_only(home))
    } else {
        (Spool::open(home)?, RelayState::open(home)?)
    };
    let mut agents = state.session_agents()?;
    // The spool retains acknowledged documents. Recover their first identity
    // when upgrading a home that has delivered sessions but has no pins yet.
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
    for path in &session_logs {
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("session log without a readable name")?;
        let records = commonmeasure_harness::SessionLog::read(path)
            .with_context(|| format!("read {}", path.display()))?;
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
        let projected = project::project_session(session_id, &records, &internal_prefixes, &|at| {
            cleared.contains_key(&at)
        });
        for (event, position) in &projected.event_positions {
            // Every projected event comes from a crossing this filter cleared,
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
            if has_new {
                refused_reported += projected.refused;
            } else if projected.refused > refused_known.get(&wire_session).copied().unwrap_or(0) {
                let last = projected.batches.last().expect("a non-empty projection");
                let mut carrier = last.clone();
                carrier.events = vec![last.events.last().expect("a batch has events").clone()];
                carrier.refused = Some(projected.refused);
                carriers.push((path.display().to_string(), carrier));
                refused_reported += projected.refused;
            }
        }
        batches.extend(
            projected
                .batches
                .into_iter()
                .map(|batch| (path.display().to_string(), batch)),
        );
    }
    let mut runs_projected = 0usize;
    for run_dir in &options.runs {
        let summary_path = run_dir.join("summary.json");
        let summary: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&summary_path)
                .with_context(|| format!("read {}", summary_path.display()))?,
        )
        .with_context(|| format!("{} is not valid JSON", summary_path.display()))?;
        let projected = project::project_run(&summary, &internal_prefixes)
            .with_context(|| format!("project {}", run_dir.display()))?;
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
            document: serde_json::to_value(&batch).context("serialise batch")?,
            directory_selection,
        };
        if options.dry_run {
            pending.push((0, entry));
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
            document: serde_json::to_value(&carrier).context("serialise batch")?,
            directory_selection,
        };
        if options.dry_run {
            pending.push((0, entry));
        } else {
            spool.enqueue(&entry)?;
        }
    }
    if !options.dry_run {
        pending = spool.pending()?;
    }

    // Deliver everything pending, oldest first. A failure stops here and the
    // spool keeps the rest: durable, inspectable, redeliverable.
    let mut batches_delivered = 0u64;
    let mut events_delivered = 0u64;
    let mut events_new_at_receiver = 0u64;
    let mut delivered_by_clearance: BTreeMap<Clearance, u64> = BTreeMap::new();
    let mut hosts = BTreeSet::new();
    for (index, mut entry) in pending {
        // Re-read consent, grant expiry and source policy before every delivery.
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
            recheck_directory_batch(home, &mut entry)?;
            if event_ids(&entry.document).is_empty() {
                // The immutable spool and original evidence remain on disk.
                // If later cleared again, projection can enqueue these ids anew.
                if !options.dry_run {
                    spool.ack_through(index)?;
                }
                continue;
            }
        }
        let session = wire_session(&entry.document).context("spooled batch has no session id")?;
        let agent = agents
            .get(&session)
            .context("spooled batch has no agent id")?;
        entry.document["agent_id"] = serde_json::Value::String(agent.clone());
        let ids = event_ids(&entry.document);
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
            let acceptance = match client::deliver(&receiver, api_key.as_deref(), &entry.document) {
                Ok(acceptance) => acceptance,
                Err(error) => {
                    state.record_failure(&receiver, &format!("{error:#}"))?;
                    return Err(anyhow::Error::new(DeliveryFailure {
                        standing,
                        directory_proof,
                        receiver,
                        spool: home.join("relay").join("spool"),
                        cause: error,
                    }));
                }
            };
            state.record_delivered(&ids)?;
            if let (Some(session), Some(refused)) = (
                wire_session(&entry.document),
                entry.document["refused"].as_u64(),
            ) {
                state.record_refused_delivered(session, refused)?;
            }
            spool.ack_through(index)?;
            events_new_at_receiver += acceptance.events_created;
            state.record_success(&receiver)?;
        }
        batches_delivered += 1;
        events_delivered += ids.len() as u64;
        for id in &ids {
            let clearance = clearance_of
                .get(id)
                .cloned()
                .unwrap_or(Clearance::SpooledBeforeThisRun);
            *delivered_by_clearance.entry(clearance).or_default() += 1;
        }
    }

    Ok(RelayReport {
        dry_run: options.dry_run,
        hosts,
        receiver,
        sessions_read: session_logs.len(),
        sessions_projected,
        sessions_withheld,
        sessions_withheld_access_context,
        runs_projected,
        refused_reported,
        events_enqueued,
        batches_delivered,
        events_delivered,
        events_new_at_receiver: (!options.dry_run).then_some(events_new_at_receiver),
        delivered_by_clearance: delivered_by_clearance
            .into_iter()
            .map(|(clearance, events)| DeliveredUnderClearance { clearance, events })
            .collect(),
        standing,
        directory_proof,
    })
}

/// Apply the declaration at the last boundary before projection. The governing
/// engagement remains a resolved property of the recorded cwd: it is never
/// copied into the private evidence merely to make egress convenient. Each
/// crossing is resolved separately because one host session can traverse
/// worktrees.
///
/// This reads the governing engagement — the one an enforcing seam may see. The
/// reported engagement the console counts this crossing under lives in the
/// sink's attribution rules, which this crate cannot read and must not
/// (`DECISIONS.md` §Session policy and egress); the console is where the two are compared.
///
/// Keys are the positions of the cleared crossings **in the log as read**,
/// witnessed and refused alike, not
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
        .filter(|(_, record)| project::is_witnessed(record) || project::is_refused(record))
        .filter_map(|(at, record)| {
            let resolved = policy.resolve(record["payload"]["cwd"].as_str());
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
/// Such terms require `access_context` on the session (`DECISIONS.md`
/// §Session policy and egress); `None` when no cleared crossing falls under
/// terms naming any.
fn access_context_required(
    policy: &commonmeasure_harness::policy::PolicyDocument,
    records: &[serde_json::Value],
    cleared: &HashMap<usize, String>,
) -> Option<String> {
    records
        .iter()
        .enumerate()
        .filter(|(at, _)| cleared.contains_key(at))
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
    document["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["id"].as_str())
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect()
}

/// Re-project a queued session against current permission, preserving stable event
/// ids. Missing source evidence cannot authorise a previously queued disclosure.
fn recheck_directory_batch(home: &Path, entry: &mut SpoolEntry) -> Result<()> {
    if commonmeasure_harness::directory::Registry::read(home)
        .map_err(anyhow::Error::msg)?
        .is_none()
    {
        // Provenance still requires consent even if both the registry and its
        // persistent mode marker have been lost. Legacy scopes cannot replace it.
        // Keep the batch pending as well: acknowledging it would allow its
        // historical events to be projected again under legacy scopes later.
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
        return Ok(());
    }
    let id = original
        .file_stem()
        .and_then(|s| s.to_str())
        .context("invalid session source")?;
    let records = commonmeasure_harness::SessionLog::read(&original)?;
    let policy =
        commonmeasure_harness::policy::PolicyDocument::read(home).map_err(anyhow::Error::msg)?;
    let cleared = egress_clearances(&policy, &records);
    let prefixes = policy.resolve(None).internal_prefixes().to_vec();
    let projected =
        project::project_session(id, &records, &prefixes, &|at| cleared.contains_key(&at));
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
    Ok(())
}
