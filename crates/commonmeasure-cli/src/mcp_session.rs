//! One mediated session, opened the same way whichever transport carries it.
//!
//! `commonmeasure mcp` opens one per process and serves it on stdio; the
//! hosted edge opens one per HTTP session. What is per process stays with
//! the caller: the operator home, the provider credentials applied to the
//! process environment before any thread exists, and the streams the session
//! is served on. Everything a session needs before its first crossing is done
//! here, once per session, in one order.

use std::path::Path;

use commonmeasure_harness::SessionLog;
use commonmeasure_harness::mcp::{McpServer, Served};
use commonmeasure_harness::policy::{PolicyDocument, Principal};
use commonmeasure_supply::credentials::CredentialsStatus;

/// What the transport opening a session serves and holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transport {
    /// The tools advertised and dispatched and the revisions negotiated.
    pub(crate) served: Served,
    /// Refuse every private address whatever the policy says
    /// (`SessionPolicy::hold_private_floor`): the service's setting. The
    /// stdio server leaves the floor to the policy.
    pub(crate) hold_private_floor: bool,
    /// The directory comes from the operator's hosted-service configuration.
    pub(crate) declared_directory: bool,
    /// The session was started by a host process on this machine, so its
    /// log records that process for the join with the hook log
    /// (`docs/contracts/session-evidence.md` §Host process). A hosted
    /// session's client is in its vendor's cloud; the service's own parent
    /// is a supervisor and is not recorded as a host.
    pub(crate) local_host: bool,
    /// The process relays this home on an interval (the hosted service),
    /// so a licence's reporting demand can be met whatever the host word.
    pub(crate) interval_relay: bool,
}

impl Transport {
    /// The stdio server: everything served, the floor the policy's.
    pub(crate) const STDIO: Self = Self {
        served: Served::DEFAULT,
        hold_private_floor: false,
        declared_directory: false,
        local_host: true,
        interval_relay: false,
    };
}

/// Open session `session_id` for the host `host` and return the server that
/// answers it.
///
/// The host word is the registration's own and is recorded on every record
/// the session leaves. It is a property of the session, not of the process:
/// a hosted edge answers several hosts from one process, one session each.
/// `cwd` is the directory the session's policy scope is resolved against and
/// the one its crossings record; the stdio server inherits the harness's own,
/// and an unscoped transport passes none. A hosted service may pass its
/// explicitly declared directory and sets `declared_directory` so its basis
/// is recorded. `principal` is the
/// identity the policy is resolved for when a transport authenticated one
/// (the hosted edge's verified token); the stdio server passes none and the
/// policy reads the process's own user, as it always has. `transport` says
/// what the transport serves and whether it holds the private-address floor.
///
/// In order: the directory proof is renewed when the enrolment record says it
/// is due; managed policy is refreshed unless this session's log already
/// carries a session-start hook's refresh, so the session runs under the
/// revision the hub desires now; the policy is resolved once, and the log is
/// stamped with the identity every crossing will record; the refresh and the
/// enrolled edge's identity are recorded before any crossing can be, and on a
/// local transport the host process that started the server is recorded
/// beside them. A failed append there is a failed session start.
pub(crate) fn open(
    home: &Path,
    host: &str,
    session_id: &str,
    cwd: Option<String>,
    principal: Option<Principal>,
    credentials: CredentialsStatus,
    transport: Transport,
) -> Result<McpServer, String> {
    let started = std::time::Instant::now();
    let directory_proof = crate::start_directory_proof_refresh(home);
    let policy_sync = (!SessionLog::holds_event(home, session_id, "policy_sync"))
        .then(|| {
            crate::sync_managed_policy(home, commonmeasure_harness::managed::SESSION_START_BUDGET)
        })
        .flatten();
    crate::finish_directory_proof_refresh(directory_proof, started);
    let document = PolicyDocument::read(home)?;
    let policy = match principal {
        Some(principal) => document.with_principal(principal),
        None => document,
    }
    .resolve(cwd.as_deref());
    let policy = if transport.hold_private_floor {
        policy.hold_private_floor()
    } else {
        policy
    };
    let mut log = SessionLog::open(home, session_id).map_err(|error| error.to_string())?;
    log.set_policy_identity(&policy.identity());
    if let Some(sync) = policy_sync {
        log.record_policy_sync(host, "server_start", sync)
            .map_err(|error| {
                format!(
                    "could not record policy_sync to {}: {error}",
                    log.path().display()
                )
            })?;
    }
    if let Some(enrolment) = commonmeasure_harness::EnrolmentRecord::load(home)? {
        log.record_edge_identity(
            host,
            &enrolment,
            &enrolment.listing_at(home, chrono::Utc::now()),
        )
        .map_err(|error| {
            format!(
                "could not record edge_identity to {}: {error}",
                log.path().display()
            )
        })?;
    }
    // The join between this log and the hook's is the host process, so it
    // is recorded on every local session, enrolled or not, managed or not:
    // the two records above are absent on an edge without a hub, and this
    // one is needed whether or not there is one.
    if transport.local_host {
        log.record_host_process(host, "mcp", &commonmeasure_harness::host_process::find())
            .map_err(|error| {
                format!(
                    "could not record host_process to {}: {error}",
                    log.path().display()
                )
            })?;
    }
    if transport.declared_directory {
        let directory = cwd
            .as_deref()
            .ok_or("declared hosted directory is missing")?;
        log.record_hosted_scope(host, directory)
            .map_err(|error| format!("could not record hosted scope: {error}"))?;
    }
    // Where each credential came from is evidence — path, digest and names,
    // never a value — and the server records it before the first record a
    // tool call leaves, not here, so a server asked for nothing leaves its
    // start records and no credentials record
    // (`McpServer::record_session_start`).
    // Human-facing words go to stderr on every transport: on stdio the host
    // reads stdout as protocol, and in service mode stderr is the journal.
    eprintln!(
        "commonmeasure: mediating session {session_id}, recording to {}{}",
        log.path().display(),
        policy
            .scope()
            .map(|scope| format!(", under scope \"{scope}\""))
            .unwrap_or_default()
    );
    let server = McpServer::new(log, policy, host, cwd, credentials)
        .served(transport.served)
        // A hosted service answers several tenants from one home and one
        // edge identity, so they share one pace per host, it tells none of
        // them when another asked, and it keeps one call's waiting inside
        // what the HTTP transport allows a tool call.
        .pace(match transport.local_host {
            true => commonmeasure_harness::crawl_delay::Pace::Own,
            false => commonmeasure_harness::crawl_delay::Pace::Hosted,
        });
    Ok(if transport.interval_relay {
        server.interval_relay()
    } else {
        server
    })
}
