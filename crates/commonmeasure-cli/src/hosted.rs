//! The hosted edge's transport: the mediated tools over Streamable HTTP, one
//! endpoint per host word, for the hosts that reach an MCP server only from
//! their vendor's cloud (`ARCHITECTURE.md` §Hosted integration).
//!
//! The same `McpServer` answers here as on stdio; what differs is how its
//! messages arrive and what it serves. Each POST body is one JSON-RPC
//! message. `initialize` opens a protocol session: the edge mints the session
//! id, binds it to the principal the request's token names, and returns the
//! id in `Mcp-Session-Id`. Nothing is written to disk yet: `initialize`,
//! `notifications/initialized`, `tools/list` and `ping` are answered from the
//! served set alone, and the session is opened through
//! `crate::mcp_session::open`, which records its identity, at its first other
//! request. A host that connects, lists the tools and calls none leaves no
//! file. Every later request names the session in the header and is answered
//! only when the same principal presents it on the same endpoint; anything
//! else is `404`, which the protocol defines as "start a new session".
//! `DELETE` ends a session, an idle session ends itself, and the process
//! ending ends them all.
//!
//! Responses are single JSON objects. This server initiates no requests and
//! sends no notifications, so it never opens an event stream, and `GET` on an
//! endpoint answers `405`.
//!
//! Two commands run this transport. `hosted serve` takes its origin from the
//! command line and serves what the policy allows; it is the form the tests
//! and a developer run. `hosted service` is the deployed form: it reads
//! [`SERVICE_CONFIG_FILE`], refuses to start unenrolled, unmanaged or while
//! another process holds the home, runs the relay, the policy refresh, the
//! key refresh and the idle sweep on an interval, and holds the
//! private-address floor whatever the policy says. Where its configuration
//! sets `supplier_custody`, it also fetches the supplier credentials its hub
//! releases to this edge, at start and on the same interval
//! (`docs/contracts/supplier-credentials.md` §Edge side).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commonmeasure_harness::EnrolmentRecord;
use commonmeasure_harness::mcp::{McpServer, Served};
use commonmeasure_http::{Request, Response, Server};
use commonmeasure_supply::credentials::{CredentialsStatus, ReleasedStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hosted_tokens::{Bearer, Verifier};

/// The host words an endpoint may be registered under, each the `host` every
/// record of a session through it carries. An unknown path is `404`, the
/// HTTP form of the stdio server refusing an unknown `--host`.
pub(crate) const HOST_WORDS: [&str; 4] = [
    "claude-connector",
    "chatgpt",
    "m365-copilot",
    "copilot-cloud-agent",
];

/// What a hosted endpoint serves: the three tools that read, and the two
/// revisions the hosts speak. `context_enrol` acts on the directory the
/// server runs in, which a hosted session has none of, and 2025-03-26 brings
/// JSON-RPC batching no hosted client asks for. The stdio server is
/// unchanged (`Served::default`).
pub(crate) const HOSTED_SERVED: Served = Served {
    tools: &["context_fetch", "context_search", "context_status"],
    protocols: &["2025-06-18", "2025-11-25"],
};

/// Endpoints are `<origin>/mcp/<host>`.
const ENDPOINT_PREFIX: &str = "/mcp/";

/// RFC 9728 places a resource's metadata at this path followed by the
/// resource's own path.
const RESOURCE_METADATA_PREFIX: &str = "/.well-known/oauth-protected-resource";

/// A session with no request for this long is ended; its next request is
/// `404` and the host starts a new one.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The service configuration under the operator home.
pub(crate) const SERVICE_CONFIG_FILE: &str = "hosted-service.json";

/// The lock one service process holds on the home for as long as it runs.
/// The kernel releases it when the process ends, however it ends.
pub(crate) use commonmeasure_harness::delivery::SERVICE_LOCK_FILE;

/// The default interval of the service's relay, policy refresh, key refresh
/// and idle sweep.
const DEFAULT_INTERVAL_SECS: u64 = 300;

const SESSION_HEADER: &str = "Mcp-Session-Id";
const PROTOCOL_HEADER: &str = "MCP-Protocol-Version";

pub(crate) struct Options {
    /// The address the process accepts connections on.
    pub(crate) listen: String,
    /// The origin hosts reach the edge on: scheme and authority, nothing
    /// else. Each endpoint's canonical URL, and so each token's audience,
    /// is built from it.
    pub(crate) origin: String,
    /// `Origin` header values accepted beside `origin` itself.
    pub(crate) allowed_origins: Vec<String>,
    /// The host words served, each one of [`HOST_WORDS`]. A path naming any
    /// other word is `404`.
    pub(crate) hosts: Vec<String>,
    /// Set for `hosted service`: the floor is held and the interval loop
    /// runs. `hosted serve` leaves it unset.
    pub(crate) service: Option<ServiceOptions>,
}

pub(crate) struct ServiceOptions {
    pub(crate) interval: Duration,
    pub(crate) session_directory: Option<String>,
    pub(crate) supplier_custody: bool,
}

/// `<home>/hosted-service.json`: what the deployed service needs beyond the
/// enrolment and deployment records, written by the operator once. Unknown
/// fields are refused so a misspelt field cannot silently take a default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceConfig {
    /// The address the process listens on, ordinarily loopback or the
    /// machine's private address behind the load balancer.
    #[serde(default = "default_listen")]
    pub(crate) listen: String,
    /// The public origin, scheme and authority only.
    pub(crate) origin: String,
    #[serde(default)]
    pub(crate) allowed_origins: Vec<String>,
    /// The host words served.
    pub(crate) hosts: Vec<String>,
    /// The interval of the relay, the policy refresh, the key refresh and
    /// the idle sweep.
    #[serde(default = "default_interval")]
    pub(crate) interval_seconds: u64,
    /// An operator-declared scope for this service's sessions. It is not a
    /// directory reported by the remote client, and grants no reporting by itself.
    #[serde(default)]
    pub(crate) session_directory: Option<String>,
    /// Fetch the supplier credentials the hub releases to this edge and use
    /// one where neither the environment nor `credentials.env` supplies the
    /// provider's variable. Unset, the service makes no release request and
    /// reads credentials as a local edge does.
    #[serde(default)]
    pub(crate) supplier_custody: bool,
}

fn default_listen() -> String {
    "127.0.0.1:8765".to_owned()
}

fn default_interval() -> u64 {
    DEFAULT_INTERVAL_SECS
}

impl ServiceConfig {
    pub(crate) fn path(home: &Path) -> PathBuf {
        home.join(SERVICE_CONFIG_FILE)
    }

    /// Read the configuration. `Ok(None)` is no file; a file that does not
    /// load is an error, never a default.
    pub(crate) fn read(home: &Path) -> Result<Option<Self>, String> {
        let source = Self::path(home);
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let mut config: Self = serde_json::from_slice(&encoded).map_err(|error| {
            format!(
                "{} is not a valid service configuration: {error}",
                source.display()
            )
        })?;
        if config.hosts.is_empty() {
            return Err(format!(
                "{}: hosts is empty; name at least one of {}",
                source.display(),
                HOST_WORDS.join(", ")
            ));
        }
        if config.interval_seconds == 0 {
            return Err(format!(
                "{}: interval_seconds must be at least 1",
                source.display()
            ));
        }
        if let Some(directory) = &config.session_directory {
            if !Path::new(directory).is_absolute() {
                return Err(
                    "session_directory must be an absolute, explicitly enrolled directory".into(),
                );
            }
            let root = commonmeasure_harness::directory::selected(Path::new(directory))?;
            let root_text = root.to_str().ok_or("session_directory must be UTF-8")?;
            let registry = commonmeasure_harness::directory::Registry::read(home)?
                .ok_or("enrol session_directory locally before configuring the hosted service")?;
            if !registry
                .matching(root_text)
                .is_some_and(|project| project.root == root)
            {
                return Err(
                    "session_directory must be an explicitly enrolled root with a current binding"
                        .into(),
                );
            }
            config.session_directory = Some(root_text.to_owned());
        }
        Ok(Some(config))
    }
}

/// `hosted service`: the deployed form. The order of refusals is the order
/// an operator fixes them in: the configuration, the enrolment, the
/// deployment mode, then the lock.
pub(crate) fn service(home: &Path, listen: Option<String>) -> Result<(), String> {
    let config = ServiceConfig::read(home)?.ok_or_else(|| {
        format!(
            "the hosted service reads its origin and host words from {}, which does not exist",
            ServiceConfig::path(home).display()
        )
    })?;
    require_enrolled(home)?;
    // A hosted edge that is not managed would take its policy from a file
    // nobody at the organisation can see.
    match commonmeasure_harness::managed::is_managed(home) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "the hosted service runs only under managed policy, because its policy must be \
                 the organisation's and not a file on this machine; {} is absent or local. Enrol \
                 with connect --managed.",
                commonmeasure_harness::managed::Deployment::path(home).display()
            ));
        }
        Err(reason) => return Err(reason),
    }
    let lock = HomeLock::take(home)?;
    let options = Options {
        listen: listen.unwrap_or(config.listen),
        origin: config.origin,
        allowed_origins: config.allowed_origins,
        hosts: config.hosts,
        service: Some(ServiceOptions {
            interval: Duration::from_secs(config.interval_seconds),
            session_directory: config.session_directory,
            supplier_custody: config.supplier_custody,
        }),
    };
    run(home, options, Some(lock))
}

/// `hosted serve`: the command-line form.
pub(crate) fn serve(home: &Path, options: Options) -> Result<(), String> {
    require_enrolled(home)?;
    run(home, options, None)
}

fn require_enrolled(home: &Path) -> Result<EnrolmentRecord, String> {
    EnrolmentRecord::load(home)?.ok_or_else(|| {
        format!(
            "the hosted edge serves only an enrolled home, because the hub named at enrolment is \
             the issuer whose tokens it accepts; there is no enrolment record at {}",
            EnrolmentRecord::path(home).display()
        )
    })
}

/// Bind, announce the bound address on stdout as `listening on <url>`, and
/// serve until the process ends. `lock` is held for as long as this runs.
fn run(home: &Path, options: Options, lock: Option<HomeLock>) -> Result<(), String> {
    // Provider credentials are applied once, at process start, before any
    // thread exists; each session records what was loaded.
    let credentials = commonmeasure_supply::credentials::apply(home)?;
    let edge = Arc::new(HostedEdge::open(home, &options, credentials)?);
    // Once at start, after `apply` and before a session can be opened, so
    // the first session's provenance record names what is held.
    let custody_started = Instant::now();
    edge.fetch_supplier_credentials();
    let handle = Server::bind(&options.listen)
        .map_err(|error| format!("cannot listen on {}: {error:#}", options.listen))?
        .spawn({
            let edge = Arc::clone(&edge);
            move |request| edge.handle(request)
        })
        .map_err(|error| format!("cannot serve: {error:#}"))?;
    eprintln!(
        "commonmeasure: hosted edge for {} at {}, endpoints {}, issuer {}{}",
        edge.organisation,
        edge.origin,
        edge.hosts
            .iter()
            .map(|host| edge.resource_url(host))
            .collect::<Vec<_>>()
            .join(" "),
        edge.verifier.issuer(),
        match &options.service {
            Some(service) => format!(
                ", service mode: private-address floor held, relay and refresh every {}s{}",
                service.interval.as_secs(),
                if service.supplier_custody {
                    ", supplier credentials released by the hub"
                } else {
                    ""
                }
            ),
            None => String::new(),
        }
    );
    if let Some(service) = &options.service {
        let interval = service.interval;
        std::thread::Builder::new()
            .name("hosted-service-interval".to_owned())
            .spawn({
                let edge = Arc::clone(&edge);
                move || {
                    loop {
                        std::thread::sleep(interval);
                        edge.tick();
                    }
                }
            })
            .map_err(|error| format!("cannot start the interval thread: {error}"))?;
        // A thread of its own on the same interval. Behind the policy
        // refresh and the relay in `tick`, a fetch would start an interval
        // plus however long those took after the last one, and the contract
        // bounds a revocation's arrival at the interval plus one request
        // budget (`docs/contracts/supplier-credentials.md` §Revocation and
        // rotation). For that bound each fetch starts one interval after the
        // last one started, not after it finished: a revocation the hub
        // takes just after it read its rows for one fetch is served by the
        // next, which starts an interval later and ends within one budget.
        // A fetch that overruns the interval is followed at once. If this
        // thread dies, the store stops supplying the set by its own age.
        if edge.released.is_some() {
            let edge = Arc::clone(&edge);
            std::thread::Builder::new()
                .name("hosted-service-custody".to_owned())
                .spawn(move || {
                    let mut next = custody_started + interval;
                    loop {
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                        edge.fetch_supplier_credentials();
                        next = (next + interval).max(Instant::now());
                    }
                })
                .map_err(|error| format!("cannot start the custody thread: {error}"))?;
        }
    }
    crate::write_stdout(&format!("listening on {}\n", handle.url()))?;
    handle.wait();
    drop(lock);
    Ok(())
}

/// The exclusive hold one service process has on the home. Two processes
/// would each hold half the sessions and answer `404` to the other half's
/// requests, and would both write the shared files; the second refuses to
/// start instead.
pub(crate) struct HomeLock {
    _file: std::fs::File,
}

impl HomeLock {
    pub(crate) fn path(home: &Path) -> PathBuf {
        home.join(SERVICE_LOCK_FILE)
    }

    fn take(home: &Path) -> Result<Self, String> {
        let path = Self::path(home);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("cannot open the lock {}: {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(format!(
                "another hosted service holds {}; one process serves a home, because the \
                 sessions live in its memory and the record needs one writer",
                path.display()
            )),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("cannot lock {}: {error}", path.display()))
            }
        }
    }

    /// Whether a service currently holds the home's lock. The licence
    /// ruling reads the same probe, since a running service relays every
    /// session in the home ([`commonmeasure_harness::delivery::service_running`]).
    pub(crate) fn held(home: &Path) -> bool {
        commonmeasure_harness::delivery::service_running(home)
    }
}

/// What `doctor` and `status` say about the service: whether the home is
/// configured for it, for which origin and endpoints, and whether a service
/// process holds the home now.
pub(crate) fn service_line(home: &Path) -> String {
    match ServiceConfig::read(home) {
        Ok(None) => "hosted service: not configured (no hosted-service.json)".to_owned(),
        Err(reason) => format!("hosted service: {reason}"),
        Ok(Some(config)) => format!(
            "hosted service: {} at {}, endpoints {}, every {}s{}",
            if HomeLock::held(home) {
                "running (lock held)"
            } else {
                "configured, not running"
            },
            config.origin,
            config
                .hosts
                .iter()
                .map(|host| format!("{ENDPOINT_PREFIX}{host}"))
                .collect::<Vec<_>>()
                .join(" "),
            config.interval_seconds,
            if config.supplier_custody {
                format!("; {}", custody_standing(home, config.interval_seconds))
            } else {
                String::new()
            }
        ),
    }
}

/// What the last supplier credential fetch did, by name, for a service
/// configured for custody. This reads the record and not the service's
/// memory, so it says "last fetch", and where the record's held set was last
/// confirmed longer ago than the store's maximum age it says a running
/// service has stopped using it: that is how a fetch that no longer runs
/// shows here.
fn custody_standing(home: &Path, interval_seconds: u64) -> String {
    use commonmeasure_relay::supplier_credentials::MAX_AGE_INTERVALS;
    match commonmeasure_relay::supplier_credentials::Fetch::read(home) {
        Ok(Some(fetch)) => {
            let max_age = interval_seconds.saturating_mul(u64::from(MAX_AGE_INTERVALS));
            let now = chrono::Utc::now();
            let lapsed = fetch.held.iter().any(|held| {
                chrono::DateTime::parse_from_rfc3339(&held.fetched_at).is_ok_and(|confirmed| {
                    now.signed_duration_since(confirmed).num_seconds()
                        >= i64::try_from(max_age).unwrap_or(i64::MAX)
                })
            });
            format!(
                "last fetch: {} at {}{}",
                fetch.journal_line(),
                fetch.at,
                if lapsed {
                    format!(
                        "; held set last confirmed more than {max_age}s ago, which a running \
                         service no longer uses"
                    )
                } else {
                    String::new()
                }
            )
        }
        Ok(None) => "supplier credentials not fetched yet".to_owned(),
        Err(reason) => format!("supplier credentials: {reason}"),
    }
}

/// One hosted edge: one enrolment, one origin, many sessions.
struct HostedEdge {
    home: PathBuf,
    origin: String,
    allowed_origins: Vec<String>,
    hosts: Vec<&'static str>,
    organisation: String,
    credentials: CredentialsStatus,
    /// The hub-released supplier credentials, where the service is
    /// configured for custody. In memory only; every session reads this one
    /// set at adapter construction.
    released: Option<ReleasedStore>,
    verifier: Verifier,
    /// Whether the private-address floor is held whatever the policy says.
    hold_private_floor: bool,
    /// Whether the interval loop relays this home (`hosted service`), which
    /// is automatic delivery for every session's reporting demands.
    interval_relay: bool,
    session_directory: Option<String>,
    sessions: Mutex<HashMap<String, Arc<Mutex<HostedSession>>>>,
}

/// One protocol session: the endpoint and bearer it was opened for, when it
/// was last used, and the server answering it once one exists.
struct HostedSession {
    state: SessionState,
    host: &'static str,
    bearer: Bearer,
    last_used: Instant,
}

enum SessionState {
    /// Minted and bound at `initialize`, nothing on disk. The `initialize`
    /// message is kept to be replayed into the server when it is opened, so
    /// the server holds the client identity and the negotiated revision the
    /// client was answered with.
    Pending { initialize: String },
    /// Boxed: a server is a few hundred bytes, a pending session a string.
    Open(Box<McpServer>),
}

impl HostedEdge {
    fn open(
        home: &Path,
        options: &Options,
        credentials: CredentialsStatus,
    ) -> Result<Self, String> {
        let enrolment = require_enrolled(home)?;
        let origin = canonical_origin(&options.origin)
            .map_err(|reason| format!("origin {:?}: {reason}", options.origin))?;
        let allowed_origins = options
            .allowed_origins
            .iter()
            .map(|allowed| {
                canonical_origin(allowed)
                    .map_err(|reason| format!("allowed origin {allowed:?}: {reason}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let hosts = options
            .hosts
            .iter()
            .map(|host| {
                HOST_WORDS
                    .into_iter()
                    .find(|word| word == host)
                    .ok_or_else(|| {
                        format!("host word {host:?} is not one of {}", HOST_WORDS.join(", "))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let verifier = Verifier::new(home, &enrolment.hub, &enrolment.organization.id);
        Ok(Self {
            home: home.to_owned(),
            origin,
            allowed_origins,
            hosts,
            organisation: enrolment.organization.id,
            credentials,
            released: options
                .service
                .as_ref()
                .filter(|service| service.supplier_custody)
                .map(|service| {
                    commonmeasure_relay::supplier_credentials::store_for(service.interval)
                }),
            verifier,
            hold_private_floor: options.service.is_some(),
            interval_relay: options.service.is_some(),
            session_directory: options
                .service
                .as_ref()
                .and_then(|service| service.session_directory.clone()),
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// The canonical URL of a host's endpoint: the token audience and the
    /// `resource` its metadata names.
    fn resource_url(&self, host: &str) -> String {
        format!("{}{ENDPOINT_PREFIX}{host}", self.origin)
    }

    fn metadata_url(&self, host: &str) -> String {
        format!(
            "{}{RESOURCE_METADATA_PREFIX}{ENDPOINT_PREFIX}{host}",
            self.origin
        )
    }

    /// Fetch the credentials the hub releases to this edge into the store,
    /// where custody is configured, and report the outcome to the journal
    /// by name. An unreachable hub keeps the last released set until the
    /// store's maximum age; a refusal or an empty release clears it.
    fn fetch_supplier_credentials(&self) {
        let Some(released) = &self.released else {
            return;
        };
        match commonmeasure_relay::supplier_credentials::fetch(
            &self.home,
            released,
            chrono::Utc::now(),
            commonmeasure_relay::supplier_credentials::BUDGET,
        ) {
            Ok(fetch) => eprintln!("commonmeasure: {}", fetch.journal_line()),
            Err(reason) => {
                // Nothing was recorded, so the journal is where an expiry
                // under a fetch that cannot be tried is said.
                let expired = released.expired();
                eprintln!(
                    "commonmeasure: supplier credentials not fetched: {reason}{}",
                    if expired.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "; no longer using {}, not confirmed by the hub for {}s",
                            expired
                                .iter()
                                .map(|name| format!("{} ({})", name.provider, name.connection_id))
                                .collect::<Vec<_>>()
                                .join(", "),
                            released.max_age().as_secs()
                        )
                    }
                );
            }
        }
    }

    /// The service's interval work: end idle sessions, refresh the managed
    /// policy, run the relay, refresh the issuer's keys. Each step reports
    /// to stderr, which is the service's journal, and a failure in one does
    /// not stop the next.
    fn tick(&self) {
        self.end_idle_sessions();
        if let Some(sync) =
            crate::sync_managed_policy(&self.home, commonmeasure_harness::managed::DEFAULT_BUDGET)
        {
            eprintln!(
                "commonmeasure: policy sync {}{}",
                sync["outcome"].as_str().unwrap_or("unknown"),
                sync["revision"]
                    .as_u64()
                    .map(|revision| format!(" (revision {revision})"))
                    .unwrap_or_default()
            );
        }
        // The operator who writes `relay/manual` reviews each run before it
        // leaves; the service honours that as the session-end relay does,
        // and the licence ruling already refuses reporting-demanding
        // sources while the marker is there.
        if let Some(reason) = commonmeasure_harness::delivery::withheld_reason(&self.home) {
            eprintln!("commonmeasure: relay skipped: {reason}");
        } else {
            self.relay();
        }
        match self.verifier.refresh_keys() {
            Ok(count) => eprintln!("commonmeasure: issuer keys refreshed, {count} held"),
            Err(reason) => eprintln!("commonmeasure: issuer keys not refreshed: {reason}"),
        }
    }

    /// One interval relay, reported to the journal.
    fn relay(&self) {
        let journal_report = |report: &commonmeasure_relay::RelayReport| {
            eprintln!(
                "commonmeasure: relay to {}: {} sessions read, {} projected, {} withheld, {} \
                 events delivered; {} queued batches and {} dead batches remain undelivered",
                report.receiver,
                report.sessions_read,
                report.sessions_projected,
                report.sessions_withheld,
                report.events_delivered,
                report.batches_queued,
                report.batches_dead
            );
            if report.instance_references_withheld > 0 {
                eprintln!(
                    "commonmeasure: warning: {} delivered events were queued with an \
                     instance reference that {} did not issue; the issuing hub is not sent \
                     them afterwards",
                    report.instance_references_withheld, report.receiver
                );
            }
        };
        match commonmeasure_relay::relay(
            &self.home,
            &commonmeasure_relay::RelayOptions {
                dry_run: false,
                policy: None,
                receiver: None,
                api_key: None,
                runs: Vec::new(),
                sessions: Vec::new(),
            },
        ) {
            Ok(report) => journal_report(&report),
            Err(error) => {
                // A run that skipped a damaged log ran for every other
                // session of the home; the journal says what it did and
                // names each skipped session on its own line.
                if let Some(skipped) =
                    error.downcast_ref::<commonmeasure_relay::UnreadableSessions>()
                {
                    journal_report(&skipped.report);
                    for session in &skipped.sessions {
                        eprintln!("commonmeasure: relay: {session}");
                    }
                } else {
                    eprintln!("commonmeasure: relay did not run: {error:#}");
                }
            }
        }
    }

    fn handle(&self, request: Request) -> Response {
        let path = request.target.split('?').next().unwrap_or_default();
        if let Some(resource) = path.strip_prefix(RESOURCE_METADATA_PREFIX) {
            return match self.host_word(resource) {
                Some(host) if request.method == "GET" => self.metadata(host),
                Some(_) => method_not_allowed("GET"),
                None => self.unknown_path(),
            };
        }
        let Some(host) = self.host_word(path) else {
            return self.unknown_path();
        };
        if request.method != "POST" && request.method != "DELETE" {
            return method_not_allowed("POST, DELETE");
        }
        if let Err(reason) = self.origin_allowed(&request) {
            return error_response(403, &reason);
        }
        let bearer = match self.authenticate(&request, host) {
            Ok(bearer) => bearer,
            Err(refusal) => return refusal,
        };
        if let Some(version) = request.headers.get(PROTOCOL_HEADER)
            && !HOSTED_SERVED.serves_protocol(version)
        {
            return error_response(
                400,
                &format!(
                    "{PROTOCOL_HEADER} {version} is not served; this edge serves {}",
                    HOSTED_SERVED.protocols.join(", ")
                ),
            );
        }
        self.end_idle_sessions();
        if request.method == "DELETE" {
            return self.end_session(&request, host, &bearer);
        }
        self.post(&request, host, bearer)
    }

    /// The host word of an endpoint path, when this edge serves it.
    fn host_word(&self, path: &str) -> Option<&'static str> {
        let word = path.strip_prefix(ENDPOINT_PREFIX)?;
        self.hosts.iter().copied().find(|host| *host == word)
    }

    fn unknown_path(&self) -> Response {
        error_response(
            404,
            &format!(
                "no endpoint at this path; the endpoints are {}",
                self.hosts
                    .iter()
                    .map(|host| format!("{ENDPOINT_PREFIX}{host}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    }

    /// RFC 9728 protected resource metadata, served without a token: it is
    /// what a host reads to find the authorisation server.
    fn metadata(&self, host: &'static str) -> Response {
        json_response(
            200,
            &json!({
                "resource": self.resource_url(host),
                "authorization_servers": [self.verifier.issuer()],
                "bearer_methods_supported": ["header"],
                "resource_name": "Common Measure mediated context",
            }),
        )
    }

    /// The protocol's guard against a page on another origin driving this
    /// server: an `Origin` that is present and not this edge's own, nor one
    /// the operator listed, is refused. A request with no `Origin` is a
    /// server-to-server client and passes.
    fn origin_allowed(&self, request: &Request) -> Result<(), String> {
        let Some(presented) = request.headers.get("Origin") else {
            return Ok(());
        };
        let refusal = || {
            format!(
                "Origin {presented:?} is not this edge's origin {} and is not an allowed origin",
                self.origin
            )
        };
        let canonical = canonical_origin(presented).map_err(|_| refusal())?;
        if canonical == self.origin || self.allowed_origins.contains(&canonical) {
            Ok(())
        } else {
            Err(refusal())
        }
    }

    /// The bearer token, verified for this endpoint. A refusal is `401`
    /// with the challenge RFC 9728 §5 and RFC 6750 §3 define, naming the
    /// resource metadata and the check that failed.
    fn authenticate(&self, request: &Request, host: &'static str) -> Result<Bearer, Response> {
        let Some(authorization) = request.headers.get("Authorization") else {
            return Err(self.challenge(host, None));
        };
        let token = authorization
            .get(..7)
            .filter(|scheme| scheme.eq_ignore_ascii_case("bearer "))
            .map(|_| authorization[7..].trim())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                self.challenge(
                    host,
                    Some("the Authorization header does not carry a Bearer token"),
                )
            })?;
        self.verifier
            .verify(token, &self.resource_url(host), host)
            .map_err(|reason| self.challenge(host, Some(&reason)))
    }

    fn challenge(&self, host: &str, reason: Option<&str>) -> Response {
        let metadata = self.metadata_url(host);
        let mut challenge = format!("Bearer resource_metadata=\"{metadata}\"");
        let body = match reason {
            Some(reason) => {
                challenge.push_str(&format!(
                    ", error=\"invalid_token\", error_description=\"{}\"",
                    quoted_string_safe(reason)
                ));
                json!({
                    "error": "invalid_token",
                    "error_description": reason,
                    "resource_metadata": metadata,
                })
            }
            None => json!({
                "error": "a bearer token is required",
                "resource_metadata": metadata,
            }),
        };
        let mut response = json_response(401, &body);
        response.headers.set("WWW-Authenticate", &challenge);
        response
    }

    /// One POST: `initialize` with no session header mints a session; any
    /// other message is routed to the session its header names.
    fn post(&self, request: &Request, host: &'static str, bearer: Bearer) -> Response {
        let Ok(body) = std::str::from_utf8(&request.body) else {
            return error_response(400, "the request body is not UTF-8");
        };
        let Some(session_id) = request.headers.get(SESSION_HEADER) else {
            return self.mint_session(body, host, bearer);
        };
        let Some(session) = self.session(session_id, host, &bearer) else {
            return unknown_session();
        };
        let mut session = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session.last_used = Instant::now();
        let message: Value = match serde_json::from_str(body) {
            Ok(message) => message,
            Err(error) => {
                return rpc_response(Some(json!({
                    "jsonrpc": "2.0", "id": Value::Null,
                    "error": {"code": -32700, "message": format!("parse error: {error}")},
                })));
            }
        };
        if let SessionState::Pending { .. } = &session.state
            && let Some(answer) = lifecycle_answer(&message)
        {
            return rpc_response(answer);
        }
        if let SessionState::Pending { initialize } = &session.state {
            let mut server = match crate::mcp_session::open(
                &self.home,
                host,
                session_id,
                self.session_directory.clone(),
                Some(session.bearer.principal()),
                self.credentials.clone(),
                crate::mcp_session::Transport {
                    served: HOSTED_SERVED,
                    hold_private_floor: self.hold_private_floor,
                    declared_directory: self.session_directory.is_some(),
                    local_host: false,
                    interval_relay: self.interval_relay,
                },
            ) {
                Ok(server) => server,
                Err(reason) => {
                    return error_response(
                        500,
                        &format!("the session could not be opened: {reason}"),
                    );
                }
            };
            if let Some(released) = &self.released {
                server = server.released(released.clone());
            }
            // The server is told what the client was told at initialize,
            // and answers nobody: that answer was already given.
            let _ = server.handle_message_text(initialize);
            session.state = SessionState::Open(Box::new(server));
        }
        let SessionState::Open(server) = &mut session.state else {
            unreachable!("a pending session was opened above");
        };
        rpc_response(server.handle_message_text(body))
    }

    /// Mint a session for an `initialize` request, bind it to the bearer and
    /// answer from the served set. Nothing is written: the session is opened
    /// at its first request that is not lifecycle.
    fn mint_session(&self, body: &str, host: &'static str, bearer: Bearer) -> Response {
        let message: Value = match serde_json::from_str(body) {
            Ok(message) => message,
            Err(error) => {
                return rpc_response(Some(json!({
                    "jsonrpc": "2.0", "id": Value::Null,
                    "error": {"code": -32700, "message": format!("parse error: {error}")},
                })));
            }
        };
        if message["method"] != "initialize" {
            return error_response(
                400,
                &format!(
                    "no {SESSION_HEADER}: a session is opened by initialize and every later \
                          request names it in that header"
                ),
            );
        }
        let session_id = match mint_session_id() {
            Ok(session_id) => session_id,
            Err(reason) => return error_response(500, &reason),
        };
        let answer = lifecycle_answer(&message).flatten();
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                session_id.clone(),
                Arc::new(Mutex::new(HostedSession {
                    state: SessionState::Pending {
                        initialize: body.to_owned(),
                    },
                    host,
                    bearer,
                    last_used: Instant::now(),
                })),
            );
        let mut response = rpc_response(answer);
        response.headers.set(SESSION_HEADER, &session_id);
        response
    }

    fn end_session(&self, request: &Request, host: &'static str, bearer: &Bearer) -> Response {
        let Some(session_id) = request.headers.get(SESSION_HEADER) else {
            return error_response(
                400,
                &format!("DELETE names the session in {SESSION_HEADER}"),
            );
        };
        if self.session(session_id, host, bearer).is_none() {
            return unknown_session();
        }
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
        Response::new(200, Vec::new())
    }

    /// The session `session_id` names, when it was opened on this endpoint by
    /// this bearer. A session id is not authentication: another bearer, or
    /// the same bearer on another endpoint, is told the session is unknown.
    fn session(
        &self,
        session_id: &str,
        host: &'static str,
        bearer: &Bearer,
    ) -> Option<Arc<Mutex<HostedSession>>> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .cloned()?;
        let matches = {
            let held = session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            held.host == host && held.bearer == *bearer
        };
        matches.then_some(session)
    }

    /// Drop sessions idle past [`IDLE_TIMEOUT`]. A session whose lock is held
    /// is in use and stays.
    fn end_idle_sessions(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, session| {
                session
                    .try_lock()
                    .map(|held| held.last_used.elapsed() < IDLE_TIMEOUT)
                    .unwrap_or(true)
            });
    }
}

/// The answer to a lifecycle message, given without a server: `None` when
/// the message is not lifecycle and the session must be opened for it. The
/// inner `None` is a notification, owed nothing. `initialize` is answered
/// from the served set exactly as the server would answer it, `tools/list`
/// with the served tools, `ping` with an empty result.
fn lifecycle_answer(message: &Value) -> Option<Option<Value>> {
    let object = message.as_object()?;
    let Some(id) = object.get("id").cloned() else {
        // A notification: JSON-RPC owes it nothing, and the protocol's
        // `notifications/initialized` is the one every host sends first.
        return Some(None);
    };
    let method = object.get("method").and_then(Value::as_str)?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => HOSTED_SERVED.initialize_result(&params).1,
        "ping" => json!({}),
        "tools/list" => json!({"tools": HOSTED_SERVED.tool_definitions()}),
        _ => return None,
    };
    Some(Some(json!({"jsonrpc": "2.0", "id": id, "result": result})))
}

/// `hosted-<milliseconds>-<128 random bits in hex>`: the local form with the
/// process id replaced by a value the protocol asks to be globally unique and
/// cryptographically secure. The same value is the header and the record's
/// `session_id`.
fn mint_session_id() -> Result<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| format!("draw a session id from the operating system: {error}"))?;
    let hex: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("hosted-{now}-{hex}"))
}

/// An origin in the form a host canonicalises: lower-case scheme and host,
/// default port dropped, no path, query or fragment.
fn canonical_origin(candidate: &str) -> Result<String, String> {
    let url = url::Url::parse(candidate).map_err(|error| format!("not a URL: {error}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!("scheme {} is not http or https", url.scheme()));
    }
    if url.host_str().is_none() {
        return Err("no host".to_owned());
    }
    if (url.path() != "/" && !url.path().is_empty())
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("an origin is scheme and authority only, with no path".to_owned());
    }
    Ok(url.origin().ascii_serialization())
}

/// The HTTP answer to what the server had to say: `202` with no body for a
/// notification, `400` for input the server could not accept (a JSON-RPC
/// error owed to no request), `200` otherwise.
fn rpc_response(answer: Option<Value>) -> Response {
    match answer {
        None => Response::new(202, Vec::new()),
        Some(answer) => {
            let status = if answer["error"].is_object() && answer["id"].is_null() {
                400
            } else {
                200
            };
            json_response(status, &answer)
        }
    }
}

fn json_response(status: u16, body: &Value) -> Response {
    Response::json(status, &body.to_string())
}

fn error_response(status: u16, reason: &str) -> Response {
    json_response(status, &json!({"error": reason}))
}

fn unknown_session() -> Response {
    error_response(
        404,
        "the session is unknown: it was never opened here, has ended, or was opened by another \
         principal or on another endpoint; start a new session with initialize",
    )
}

fn method_not_allowed(allow: &str) -> Response {
    let mut response = error_response(405, &format!("method not allowed; allowed: {allow}"));
    response.headers.set("Allow", allow);
    response
}

/// A refusal reason inside a quoted-string header value: RFC 9110 allows no
/// bare quote or backslash there, and no control characters.
fn quoted_string_safe(reason: &str) -> String {
    reason
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '"' => '\'',
            '\\' => '/',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_is_canonicalised_or_refused() {
        assert_eq!(
            canonical_origin("HTTPS://Edge.Example:443/").as_deref(),
            Ok("https://edge.example")
        );
        assert_eq!(
            canonical_origin("http://127.0.0.1:8765").as_deref(),
            Ok("http://127.0.0.1:8765")
        );
        assert!(canonical_origin("https://edge.example/mcp").is_err());
        assert!(canonical_origin("ftp://edge.example").is_err());
        assert!(canonical_origin("null").is_err());
    }

    #[test]
    fn a_minted_session_id_is_a_plain_identifier() {
        let id = mint_session_id().expect("minted");
        assert!(id.starts_with("hosted-"));
        assert_eq!(commonmeasure_harness::safe_session(&id), Some(id.as_str()));
        assert_ne!(id, mint_session_id().expect("minted"));
    }

    #[test]
    fn lifecycle_messages_are_answered_without_a_server_and_nothing_else_is() {
        let initialize = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26"}});
        let answer = lifecycle_answer(&initialize).flatten().expect("answered");
        assert_eq!(
            answer["result"]["protocolVersion"], "2025-11-25",
            "a revision the hosted set does not serve is answered with the latest it does"
        );
        let listed = lifecycle_answer(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .flatten()
            .expect("answered");
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, ["context_fetch", "context_search", "context_status"]);
        assert_eq!(
            lifecycle_answer(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
            Some(None)
        );
        assert!(
            lifecycle_answer(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "context_status"}}))
            .is_none()
        );
        assert!(
            lifecycle_answer(&json!([{"jsonrpc": "2.0", "id": 3, "method": "ping"}])).is_none()
        );
    }

    #[test]
    fn a_service_configuration_refuses_an_unknown_field_and_an_empty_host_list() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            ServiceConfig::read(home.path()).map(|c| c.is_none()),
            Ok(true)
        );
        std::fs::write(
            ServiceConfig::path(home.path()),
            r#"{"origin":"https://edge.test","hosts":["chatgpt"],"origins":[]}"#,
        )
        .expect("written");
        let refused = ServiceConfig::read(home.path()).expect_err("refused");
        assert!(refused.contains("origins"), "{refused}");
        std::fs::write(
            ServiceConfig::path(home.path()),
            r#"{"origin":"https://edge.test","hosts":[]}"#,
        )
        .expect("written");
        let refused = ServiceConfig::read(home.path()).expect_err("refused");
        assert!(refused.contains("hosts is empty"), "{refused}");
        std::fs::write(
            ServiceConfig::path(home.path()),
            r#"{"origin":"https://edge.test","hosts":["chatgpt"]}"#,
        )
        .expect("written");
        let config = ServiceConfig::read(home.path())
            .expect("reads")
            .expect("present");
        assert_eq!(config.interval_seconds, DEFAULT_INTERVAL_SECS);
        assert_eq!(config.listen, "127.0.0.1:8765");
    }

    #[test]
    fn the_home_lock_is_held_by_one_holder() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(!HomeLock::held(home.path()));
        let lock = HomeLock::take(home.path()).expect("taken");
        assert!(HomeLock::held(home.path()));
        drop(lock);
        assert!(!HomeLock::held(home.path()));
    }
}
