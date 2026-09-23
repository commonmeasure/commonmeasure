//! The mediated surface: MCP over stdio.
//!
//! An agent that calls these tools instead of its own gives Common Measure the
//! crossing *before* it happens, which is the only point at which policy can
//! refuse anything. Every call is recorded to the same session log the observed
//! path writes to, tagged `mediated` so the two grades of evidence never blur.
//!
//! The framing is JSON-RPC 2.0 over stdio, the protocol Claude Code speaks to
//! an MCP server. Behind the tools sit the Common Measure supply adapters and
//! the same `commonmeasure_runtime::policy` the batch runner uses.

use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use commonmeasure_http::{Request, Response};
use commonmeasure_runtime::allowance::{AllowanceContext, GateDecision, Reservation};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_runtime::processor::pii::SourceClass;
use commonmeasure_supply::{Acquisition, INTERNAL_PROVIDER, SupplyError, supplier_with_released};
use commonmeasure_types::{
    AcquisitionCharge, ContextEnvelope, ContextJob, Gap, GapReason, LicenceState, PolicyMode,
    ProviderCapability,
};
use serde_json::{Value, json};

use crate::crawl_delay::CrawlDelayStore;
use crate::declarations::{Effective, TELEMETRY_PROFILE};
use crate::discovery::{self, DeclarationCache, Declarations, Governing, ManifestCache};
use crate::grounding;
use crate::identity::{Identity, PresentedIdentity, SigningIdentity};
use crate::policy::SessionPolicy;
use crate::session::{Crossing, CrossingMode, SessionLog};

/// The protocol revisions this server serves, oldest first. A tools-only
/// stdio server behaves the same under each, with two exceptions this file
/// handles: 2025-03-26 requires a server to accept JSON-RPC batches, which
/// 2025-06-18 removed, and its `Implementation` has no `title`. 2024-11-05
/// is not served: no client probed asks for it, and it has no tool
/// annotations (`readOnlyHint`), which the hosted edge's design adds to these
/// tools. A client asking for it is answered with the latest revision.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-03-26", "2025-06-18", "2025-11-25"];

/// The revision that required servers to accept JSON-RPC batches.
const BATCHING_PROTOCOL_VERSION: &str = "2025-03-26";

/// The revision to answer an `initialize` request with. The protocol's
/// version negotiation (`basic/lifecycle`, "Version Negotiation", in each
/// revision since 2025-03-26) states that a server supporting the requested
/// version "MUST respond with the same version", and otherwise "MUST respond
/// with another protocol version it supports", which "SHOULD be the latest".
/// A client that sends no version, or one this server does not serve, is
/// answered with the latest.
pub fn negotiate_protocol(requested: Option<&str>) -> &'static str {
    Served::default().negotiate(requested)
}

/// The tools this server can dispatch, in the order `tools/list` presents
/// them.
pub const TOOLS: [&str; 4] = [
    "context_fetch",
    "context_search",
    "context_status",
    "context_enrol",
];

/// What one transport serves of the whole: which tools it advertises and
/// dispatches, and which protocol revisions it negotiates. The stdio server
/// serves everything. The hosted edge serves the three tools that read and
/// the two revisions its hosts speak: `context_enrol` acts on the directory
/// the server runs in, which a hosted session does not have, and 2025-03-26
/// brings JSON-RPC batching no hosted client asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Served {
    /// Tool names, a subset of [`TOOLS`]. A call to a tool outside it is
    /// answered as an unknown tool.
    pub tools: &'static [&'static str],
    /// Protocol revisions, a subset of [`PROTOCOL_VERSIONS`], oldest first.
    pub protocols: &'static [&'static str],
}

impl Default for Served {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Served {
    /// Everything: every tool and every revision, which is what the stdio
    /// server serves.
    pub const DEFAULT: Self = Self {
        tools: &TOOLS,
        protocols: &PROTOCOL_VERSIONS,
    };

    /// The revision to answer `initialize` with: the requested one where it
    /// is served, else the latest served (see [`negotiate_protocol`]).
    pub fn negotiate(&self, requested: Option<&str>) -> &'static str {
        self.protocols
            .iter()
            .copied()
            .find(|served| Some(*served) == requested)
            .unwrap_or(self.protocols[self.protocols.len() - 1])
    }

    pub fn serves_protocol(&self, revision: &str) -> bool {
        self.protocols.contains(&revision)
    }

    /// The `tools/list` result: the definitions of the served tools only.
    pub fn tool_definitions(&self) -> Value {
        let all = tool_definitions();
        let served: Vec<Value> = all
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tool| {
                tool["name"]
                    .as_str()
                    .is_some_and(|name| self.tools.contains(&name))
            })
            .cloned()
            .collect();
        Value::Array(served)
    }

    /// The `initialize` result for `params`, and the revision it negotiated.
    /// Shared with a transport that answers `initialize` before it opens a
    /// session, so what the client is told never differs from what the
    /// server later holds.
    pub fn initialize_result(&self, params: &Value) -> (&'static str, Value) {
        let negotiated = self.negotiate(params["protocolVersion"].as_str());
        let mut server_info = json!({
            "name": "commonmeasure",
            "title": "Common Measure mediated context",
            "version": env!("CARGO_PKG_VERSION"),
        });
        // `title` on an implementation arrived in 2025-06-18; a client held
        // to 2025-03-26's schema is sent only the fields that revision
        // defines.
        if negotiated == BATCHING_PROTOCOL_VERSION
            && let Some(info) = server_info.as_object_mut()
        {
            info.remove("title");
        }
        (
            negotiated,
            json!({
                "protocolVersion": negotiated,
                "capabilities": {"tools": {}, "prompts": {}},
                "serverInfo": server_info,
            }),
        )
    }
}

const MAX_REDIRECTS: usize = 5;
/// The request header carrying the retrieval correlation id (Content
/// Telemetry section 7.2).
pub const CONTENT_TELEMETRY_ID: &str = "Content-Telemetry-ID";

/// The provider capabilities this surface can dispatch: `context_search`
/// calls `search` on a search provider and `query` on the internal corpus —
/// retrieval from a bounded corpus the operator owns, which is not a web
/// search and is not pretended to be one. No mediated tool calls anything
/// else, and the status list, this set and the tool schema's sealed
/// provider enum move together.
const MEDIATED_CAPABILITIES: [ProviderCapability; 2] =
    [ProviderCapability::Search, ProviderCapability::Query];

pub struct McpServer {
    session: SessionLog,
    policy: SessionPolicy,
    host: String,
    /// What the client said about itself in `initialize`, once it has. The
    /// `--host` value is the registration's word; this is the program's
    /// own, stamped on every crossing this server records. Held here and
    /// written to the log only before the first crossing, so a server that
    /// is started and never asked for anything leaves no client record.
    client: Option<crate::session::ClientIdentity>,
    /// The protocol version the client asked for in `initialize`.
    client_protocol: Option<String>,
    /// The protocol version this server answered `initialize` with, once it
    /// has; it decides whether a JSON-RPC batch is accepted.
    negotiated_protocol: Option<&'static str>,
    /// Whether the records a session's first tool call writes before its own
    /// (`credentials_loaded`, `client_identified`) have been written.
    start_recorded: bool,
    /// The directory this server was started in. Stdio carries no cwd, but
    /// the server inherits the harness's, and it is the same directory the
    /// session's policy scope was resolved against.
    cwd: Option<String>,
    /// Where provider credentials came from at start: the operator file's
    /// path and digest plus variable names, never a value. What an
    /// unavailable message points at, and what `context_status` reports.
    credentials: commonmeasure_supply::credentials::CredentialsStatus,
    /// The hub-released credentials this process holds, where the transport
    /// that opened this session fetches them: the hosted service with
    /// custody configured (`docs/contracts/supplier-credentials.md` §Edge
    /// side). Read at each adapter construction, after the environment.
    released: Option<commonmeasure_supply::credentials::ReleasedStore>,
    /// The released credentials the last `credentials_loaded` record named,
    /// in use and shadowed. The held set changes while a session runs, so the
    /// record is written again before a search that would use another set.
    released_recorded: Option<commonmeasure_supply::credentials::ReleasedStanding>,
    /// The tools and revisions the transport that opened this session
    /// serves.
    served: Served,
    /// The per-host cache of `robots.txt` and licence documents
    /// (`crate::discovery`), under the operator home beside the policy.
    declarations: DeclarationCache,
    /// The per-host cache of Content Telemetry discovery manifests.
    manifests: ManifestCache,
    /// The per-host record of the last request this edge made, which keeps a
    /// source's `Crawl-delay` across sessions and across servers
    /// (`crate::crawl_delay`).
    crawl_delay: CrawlDelayStore,
    /// Whose edge this server is. One identity fetches at one pace per host,
    /// which is what a publisher sees, so a hosted server's sessions share
    /// the edge's pace; a refusal there names neither when the host was last
    /// asked, because that request was another tenant's, nor the operator's
    /// own file system. A hosted call also waits less, because the transport
    /// that carries it ends a tool call sooner
    /// (`crate::crawl_delay::HOSTED_WAIT_BUDGET`).
    pace: crate::crawl_delay::Pace,
    /// The prompt records read so far from this session's log, advanced at
    /// each fetch rather than re-read from the start.
    prompts: crate::session::PromptCursor,
    /// The receiver named in `<home>/relay.json` when the server started, or
    /// none. A reporting demand is met only where reports can leave, and the
    /// relay reads the same file.
    receiver: Option<String>,
    /// The transport runs an interval relay over this home (the hosted
    /// service), so reports leave without a person whatever the host word
    /// ([`crate::delivery::SessionDelivery`]).
    interval_relay: bool,
    /// What every request this server makes presents to a publisher: the
    /// enrolled key it signs with, or why it signs nothing. Read once at
    /// start, because it is the identity of the session, and a session that
    /// changed identity halfway through would leave a record no publisher
    /// could reconcile with its own logs.
    identity: Identity,
    /// The authorities at which the enrolled hub takes this edge's signature
    /// ([`hub_authorities`]), empty for an edge with no enrolment. No
    /// mediated request is made to one of them.
    hub_authorities: Vec<HubAuthority>,
    evidence_error: Option<String>,
}

impl McpServer {
    pub fn new(
        session: SessionLog,
        policy: SessionPolicy,
        host: &str,
        cwd: Option<String>,
        credentials: commonmeasure_supply::credentials::CredentialsStatus,
    ) -> Self {
        // The policy's source is `<home>/policy.json`; the cache lives in
        // the same home.
        let home = policy
            .source()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        // An identity this runtime cannot read is not an unenrolled edge: it
        // is a broken one, and it must not fetch as though it had no key. The
        // reason is what every record then carries.
        // The record is read once, for the identity and for the hub it
        // names, so the key that signs and the origins it stays away from
        // come from one write of the file.
        let record = crate::enrolment::EnrolmentRecord::load(&home);
        let mut identity = record
            .clone()
            .and_then(|record| Identity::from_record(&home, record))
            .unwrap_or_else(|reason| Identity::Unsigned { reason });
        // A key signs only while the origins it must stay away from are
        // known: a guard that knew fewer of them would sign for the rest.
        let hub_authorities = match record {
            Ok(Some(record)) => {
                match hub_authorities(&home, &record, identity.signer().is_some()) {
                    Ok(authorities) => authorities,
                    Err(reason) => {
                        identity = Identity::Unsigned { reason };
                        Vec::new()
                    }
                }
            }
            Ok(None) | Err(_) => Vec::new(),
        };
        Self {
            session,
            policy,
            host: host.to_owned(),
            client: None,
            client_protocol: None,
            negotiated_protocol: None,
            start_recorded: false,
            cwd,
            credentials,
            released: None,
            released_recorded: None,
            served: Served::default(),
            declarations: DeclarationCache::open(&home),
            manifests: ManifestCache::open(&home),
            crawl_delay: CrawlDelayStore::open(&home),
            pace: crate::crawl_delay::Pace::Own,
            prompts: crate::session::PromptCursor::default(),
            receiver: configured_receiver(&home),
            interval_relay: false,
            identity,
            hub_authorities,
            evidence_error: None,
        }
    }

    /// The operator home this session runs against: the directory holding
    /// the policy file, which is also where `relay.json` and the relay's
    /// state live.
    fn home(&self) -> &std::path::Path {
        self.policy
            .source()
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
    }

    /// Restrict what this server advertises, dispatches and negotiates to
    /// `served`. The default serves everything.
    pub fn served(mut self, served: Served) -> Self {
        self.served = served;
        self
    }

    /// Serve sessions at `pace`: how much of one tool call may be spent
    /// waiting out a `Crawl-delay`, and how much of the edge a refusal may
    /// describe. The default is [`crate::crawl_delay::Pace::Own`], the stdio
    /// server on an edge its caller runs for themselves.
    pub fn pace(mut self, pace: crate::crawl_delay::Pace) -> Self {
        self.pace = pace;
        self
    }

    /// Count this session's reports as delivered without a person because
    /// the transport relays on an interval (the hosted service). The
    /// default reads delivery from the host word and the client's name.
    pub fn interval_relay(mut self) -> Self {
        self.interval_relay = true;
        self
    }

    /// Read remote providers' credentials from `released` where the
    /// environment holds none. The default reads the environment alone.
    pub fn released(mut self, released: commonmeasure_supply::credentials::ReleasedStore) -> Self {
        self.released = Some(released);
        self
    }

    /// Serve until stdin closes.
    ///
    /// Malformed input gets a JSON-RPC error, never a crash: Common Measure
    /// outliving a confused host is the point, and a mediator that dies takes
    /// the agent's tools with it.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Some(response) = self.handle_message_text(&line) else {
                continue; // A notification: no response is owed.
            };
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(())
    }

    /// Answer one JSON-RPC message as text: a request, a notification or,
    /// under 2025-03-26, a batch. `None` when nothing is owed, which is the
    /// case for a notification. This is the unit both transports feed the
    /// server: the stdio loop reads one per line, and the hosted edge's HTTP
    /// transport passes each POST body here. Malformed text gets a JSON-RPC
    /// error, never a panic.
    pub fn handle_message_text(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {error}"),
                ));
            }
        };
        let Value::Array(batch) = message else {
            return self.handle_message(&message, false);
        };
        // A batch is accepted only under the revision that requires it; under
        // any other it is one invalid request, answered rather than dropped,
        // because a client waiting on its members would hang.
        if self.negotiated_protocol != Some(BATCHING_PROTOCOL_VERSION) {
            return Some(error_response(
                Value::Null,
                -32600,
                &format!(
                    "a JSON-RPC batch is accepted only under protocol version \
                     {BATCHING_PROTOCOL_VERSION}; this session negotiated {}",
                    self.negotiated_protocol.unwrap_or("none")
                ),
            ));
        }
        if batch.is_empty() {
            return Some(error_response(Value::Null, -32600, "empty batch"));
        }
        // JSON-RPC 2.0 answers a batch with an array of the responses its
        // requests are owed, and with nothing when it held only
        // notifications.
        let responses: Vec<Value> = batch
            .iter()
            .filter_map(|member| self.handle_message(member, true))
            .collect();
        (!responses.is_empty()).then(|| Value::Array(responses))
    }

    fn handle_message(&mut self, message: &Value, in_batch: bool) -> Option<Value> {
        if !message.is_object() {
            return Some(error_response(
                Value::Null,
                -32600,
                "a message is a JSON object",
            ));
        }
        // Requests carry an id and are owed a response — even a request too
        // malformed to name a method, or a host waiting on it hangs.
        // Notifications carry no id and get nothing.
        let id = message.get("id").cloned()?;
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(error_response(id, -32600, "request has no method"));
        };
        if in_batch && method == "initialize" {
            // 2025-03-26 lifecycle: "The initialize request MUST NOT be part
            // of a JSON-RPC batch".
            return Some(error_response(
                id,
                -32600,
                "initialize cannot be part of a batch",
            ));
        }
        let method = method.to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(self.handle_request(id, &method, &params))
    }

    fn handle_request(&mut self, id: Value, method: &str, params: &Value) -> Value {
        match method {
            "initialize" => {
                self.identify_client(params);
                let (negotiated, result) = self.served.initialize_result(params);
                self.negotiated_protocol = Some(negotiated);
                ok_response(id, result)
            }
            "ping" => ok_response(id, json!({})),
            "prompts/list" => ok_response(
                id,
                json!({"prompts": [{"name": "commonmeasure_enrol", "description": "Enrol the current agent project directory. The client must expose MCP prompts explicitly."}]}),
            ),
            "prompts/get" if params["name"] == "commonmeasure_enrol" => ok_response(
                id,
                json!({"messages": [{"role": "user", "content": {"type": "text", "text": include_str!("../../../plugin/commands/enrol.md")}}]}),
            ),
            "tools/list" => ok_response(id, json!({"tools": self.served.tool_definitions()})),
            "tools/call" => self.handle_tool_call(id, params),
            "commonmeasure/observe" => {
                if self.negotiated_protocol.is_none() {
                    return error_response(id, -32600, "initialise before submitting observations");
                }
                if let Err(error) = self.record_session_start() {
                    return error_response(id, -32603, &error);
                }
                match self.session.record_host_observation(
                    params.clone(),
                    &self.host,
                    self.cwd.as_deref(),
                ) {
                    Ok(seq) => ok_response(id, json!({"recorded": seq})),
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                        error_response(id, -32602, &error.to_string())
                    }
                    Err(error) => error_response(
                        id,
                        -32603,
                        &format!("unavailable: host observation was not durably recorded: {error}"),
                    ),
                }
            }
            _ => error_response(id, -32601, &format!("method {method} not found")),
        }
    }

    /// Keep what the client said about itself. A client that sends no
    /// `clientInfo` leaves no record and no name on its crossings: the
    /// absence is the fact, and no default is put in its place. Nothing is
    /// written here: hosts start a server per window or per launch, and a
    /// server that is never asked for anything leaves its start records
    /// only (`docs/contracts/session-evidence.md` §Host process).
    fn identify_client(&mut self, params: &Value) {
        let info = &params["clientInfo"];
        let Some(name) = info["name"].as_str() else {
            return;
        };
        self.client = Some(crate::session::ClientIdentity {
            name: name.to_owned(),
            version: info["version"].as_str().unwrap_or_default().to_owned(),
            title: info["title"].as_str().map(str::to_owned),
        });
        self.client_protocol = params["protocolVersion"].as_str().map(str::to_owned);
    }

    /// Write the records that precede every crossing, once, before the first
    /// record a tool call leaves: `credentials_loaded` where the operator file
    /// was loaded at start, then `client_identified`. Written here rather than
    /// at start so a server that is started and never asked for anything
    /// leaves neither record.
    ///
    /// `credentials_loaded` is what lets a reader conclude, from its absence,
    /// that a session ran on the launching environment alone, so a failed
    /// append of it fails the tool call before anything is fetched, naming
    /// the log, and the next call tries again. A failed `client_identified`
    /// append does not fail the call; the log owes a gap.
    fn record_session_start(&mut self) -> Result<(), String> {
        if self.start_recorded {
            return self.record_released_change();
        }
        let standing = self.released_standing();
        if self.credentials.loaded.is_some() || standing.as_ref().is_some_and(names_any) {
            self.record_credentials(standing.as_ref())?;
        }
        self.released_recorded = standing;
        if let Some(client) = &self.client {
            let _ = self.session.record_client_identified(
                &self.host,
                client,
                self.client_protocol.as_deref(),
                self.negotiated_protocol,
            );
        }
        self.start_recorded = true;
        Ok(())
    }

    fn released_standing(&self) -> Option<commonmeasure_supply::credentials::ReleasedStanding> {
        self.released
            .as_ref()
            .map(commonmeasure_supply::credentials::ReleasedStore::standing)
    }

    /// Write `credentials_loaded` again where the released credentials in
    /// use, shadowed or expired are not the ones the last record named: a
    /// release, rotation to another connection, withdrawal or revocation
    /// reached this process since, or the set passed its maximum age with no
    /// fetch confirming it. Fetch times alone are not a change.
    fn record_released_change(&mut self) -> Result<(), String> {
        let standing = self.released_standing();
        let connections =
            |standing: &Option<commonmeasure_supply::credentials::ReleasedStanding>| {
                standing.as_ref().map(|standing| {
                    let ids = |names: &[commonmeasure_supply::credentials::ReleasedName]| {
                        names
                            .iter()
                            .map(|name| (name.connection_id.clone(), name.variable.clone()))
                            .collect::<Vec<_>>()
                    };
                    (
                        ids(&standing.in_use),
                        ids(&standing.shadowed),
                        ids(&standing.expired),
                    )
                })
            };
        if connections(&standing) == connections(&self.released_recorded) {
            return Ok(());
        }
        self.record_credentials(standing.as_ref())?;
        self.released_recorded = standing;
        Ok(())
    }

    /// Append `credentials_loaded`: what the operator file contributed and,
    /// under `hub`, the released credentials in use. A released credential
    /// the environment shadows is listed under `hub_shadowed`, and one past
    /// its maximum age under `hub_expired`. Names only.
    fn record_credentials(
        &mut self,
        standing: Option<&commonmeasure_supply::credentials::ReleasedStanding>,
    ) -> Result<(), String> {
        let payload = credentials_payload(&self.credentials, standing);
        self.session
            .record_credentials(&self.host, payload)
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "unavailable: could not record credentials_loaded to {}: {error}. \
                     Nothing was fetched.",
                    self.session.path().display()
                )
            })
    }

    fn handle_tool_call(&mut self, id: Value, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = match name.as_str() {
            tool if !self.served.tools.contains(&tool) => Err(format!("unknown tool {tool}")),
            "context_fetch" => self.tool_fetch(&arguments),
            "context_search" => self.tool_search(&arguments),
            "context_status" => Ok(self.tool_status()),
            "context_enrol" => self.tool_enrol(&arguments),
            other => Err(format!("unknown tool {other}")),
        };
        // A tool failure is a tool result, not a protocol error: the host's
        // model needs to read it and decide what to do.
        match result {
            Ok(value) => tool_result(id, &value, false),
            Err(detail) => tool_result(id, &json!({"error": detail}), true),
        }
    }

    /// Fetch one URL under the operator's standing policy, record it, and hand
    /// back the bytes.
    ///
    /// This is the whole mediated proposition in one call: the fetch does not
    /// happen unless policy allows it, and whether it happened or not is on the
    /// record either way. Before the request, `robots.txt` and the licence it
    /// names are read for the host and ruled on; after it, the response's own
    /// `Content-Usage` and `Link` headers are read and ruled on again, so a
    /// statement the page carries can still keep its bytes out of context.
    fn tool_fetch(&mut self, arguments: &Value) -> Result<Value, String> {
        self.record_session_start()?;
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .ok_or("url is required")?;
        if !self.policy.mediates_address(url) {
            if self.policy.holds_private_floor() {
                return Err(
                    "Common Measure does not mediate local or private addresses, and this edge \
                     holds that floor in service mode: no policy setting lifts it here."
                        .to_owned(),
                );
            }
            return Err(format!(
                "Common Measure does not mediate local or private addresses. Name this one's prefix \
                 in \"record_internal_prefixes\", or set \"allow_private_hosts\": true, in {} if \
                 it should be governed.",
                self.policy.source().display()
            ));
        }

        // Who named this URL, decided once against the prompts recorded so
        // far and stamped on whatever this fetch records.
        let named_by = Some(self.named_by(url));
        // Before the host's admission and before anything is read from the
        // origin: the declaration probes are signed requests too.
        if let Err(reason) = self.keeps_off_hub(url) {
            let mut facts = FetchFacts::refused(url, reason.clone());
            facts.named_by = named_by;
            self.record(facts);
            return Err(format!("refused before the crossing: {reason}"));
        }
        let ruling = self.policy.admit_host(url);
        if let Ruling::Refused { reason, .. } = &ruling {
            let mut facts = FetchFacts::refused(url, reason.clone());
            facts.named_by = named_by;
            self.record(facts);
            return Err(format!(
                "refused before the crossing: {reason} \
                 (operator policy in {})",
                self.policy.source().display()
            ));
        }

        self.instance_permits("context_fetch", Some(url))?;

        // What the host declares, read before the request: robots.txt for
        // the product token's group and the licence it names. The probe goes
        // through the same host policy and address floor as the page.
        let terms = self.policy.terms_for(&grounding::host_of(url)).cloned();
        // The pace this tool call keeps: the delay each host states, the
        // turns taken for the page, every redirect hop and the licence and
        // manifest probes, and what the call may still spend asleep.
        let pacing = crate::crawl_delay::Pacing::new(self.crawl_delay.clone(), self.pace);
        // `robots.txt` takes no turn, so the access rule is ruled before
        // anything is owed to the host: a crossing refused on a `Disallow`,
        // in any mode, costs the host only the file it would have read anyway.
        let read = discovery::read_robots(
            &self.declarations,
            url,
            Utc::now(),
            &|probe_url| self.probe(probe_url, &pacing),
            Some(&pacing),
            self.policy.mode(),
        );
        // A host that states a delay cannot have its manifest resolved after
        // the page: the page's turn takes the free one, and a probe needing a
        // whole delay from what the page left would never be sent for a host
        // stating more than half the budget. Where a delay was read, the
        // manifest is asked at this crossing's first free turn instead and the
        // page waits behind it; where the host is inside its delay the probe
        // is not sent and waits for a crossing that finds it clear. A licence
        // with no current reading is read before the page and carries terms
        // the page is admitted under, so it has the first turn and the
        // manifest, which carries no demand, waits for a later crossing.
        let early_manifest = (!read.refused()
            && pacing.delay_for(read.host()).is_some()
            && !read.reads_a_licence_first(url, Utc::now()))
        .then(|| self.resolve_manifest(url, &pacing, discovery::ManifestTurn::Free))
        .flatten();
        // A licence with no current reading is read here, before the page, and
        // the page's turn follows it: a page is never admitted under a licence
        // this edge has not read. Whether both turns fit is decided before
        // either is taken. The delay the source states is kept across every
        // session and every server on this edge.
        let mut declarations = discovery::read_declared(
            &self.declarations,
            read,
            url,
            terms.as_ref(),
            Utc::now(),
            &|probe_url| self.probe(probe_url, &pacing),
            Some(&pacing),
        );
        // A crossing refused on its licence's turn (licence-then-page did not
        // fit, or the licence was not sent) is refused here, before any
        // allowance is consulted. The page's own turn is still to come.
        if !declarations.robots.sends() {
            let reason = declarations
                .robots
                .delay_refusal()
                .unwrap_or_else(|| "the source's Crawl-delay could not be kept".to_owned());
            // The refusal keeps every breach known so far: the host's, and
            // what the declarations read so far carry (a `Content-Signal`
            // observe carries), as every other refusal path does.
            let known = self.rule_on_declarations(&mut declarations);
            let mut facts = FetchFacts::refused(url, reason.clone());
            facts.breach = merge_breaches(
                ruling.reason().map(str::to_owned),
                breaches_of(&known).as_deref(),
            );
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            self.record(facts);
            return Err(format!(
                "refused before the crossing: {reason} The refusal is recorded in {}.",
                self.session.path().display()
            ));
        }
        // Pre-authorisation, before any other ruling on the licence. A
        // licence a redirect's destination names is read before that hop
        // is requested and ruled on the same way, but it is not
        // pre-authorised: the quote known here is the requested URL's. Where
        // the licence read before the request quotes a price for the use this
        // fetch makes, the principal's allowance is consulted as it is before
        // a run's dispatch: the quoted price is reserved, strict refuses an
        // exhausted or incomparable allowance before the request, observe and
        // prefer carry the fetch with the breach recorded, and the
        // reservation is settled against the receipt afterwards. No rail pays
        // a quoted price here yet, so the receipt carries no charge and the
        // reservation is released on it; the record shows the price the
        // licence asked and that nothing was paid. A refusal that follows for
        // another reason releases the reservation with that reason.
        let mut authorised: Option<(AllowanceContext, GateDecision)> = None;
        let mut allowance_breach = None;
        if let Some(price) = quoted_price(&declarations)
            && !self.policy.allowances().is_empty()
        {
            let Some(home) = self
                .policy
                .source()
                .parent()
                .map(std::path::Path::to_path_buf)
            else {
                return Err(format!(
                    "unavailable: the policy source {} has no parent directory, so the \
                     allowance ledger cannot be located and the declared allowance cannot be \
                     enforced. No fetch was attempted.",
                    self.policy.source().display()
                ));
            };
            let context = AllowanceContext::new(
                &home,
                self.policy.principal().to_owned(),
                self.policy.allowances().to_vec(),
            );
            let mut decision = context.pre_dispatch(
                Some(&price),
                self.policy.mode(),
                Utc::now(),
                &format!("session {} fetch {url}", self.session.session_id()),
            );
            decision.record["quoted_by"] = json!("the licence read before the request");
            if let Some(ruling) = &decision.ruling {
                if ruling.is_refusal() {
                    let reason = ruling.reason().unwrap_or_default().to_owned();
                    let mut facts = FetchFacts::refused(url, reason.clone());
                    facts.declarations = Some(declarations);
                    facts.allowance = Some(decision.record);
                    facts.named_by = named_by;
                    self.record(facts);
                    return Err(format!(
                        "refused before the crossing: {reason} (operator policy in {})",
                        self.policy.source().display()
                    ));
                }
                allowance_breach = ruling.reason().map(str::to_owned);
            }
            authorised = Some((context, decision));
        }

        let before = self.rule_on_declarations(&mut declarations);
        if let Some(reason) = first_refusal(&before) {
            let mut facts = FetchFacts::refused(url, reason.clone());
            // A refusal keeps the other breaches the same declarations
            // carried, such as an unmet payment term beside the reporting
            // demand that refused, and the allowance ruling observe carried.
            facts.breach = merge_breaches(
                ruling.reason().map(str::to_owned),
                breaches_of(&before).as_deref(),
            );
            facts.breach = merge_breaches(facts.breach, allowance_breach.as_deref());
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.allowance =
                self.release_authorisation(authorised, "the fetch was refused before the request");
            let refused_by = self.refused_by(facts.declarations.as_ref(), url);
            self.record(facts);
            return Err(format!(
                "refused before the crossing: {reason} ({refused_by}; the source's declarations \
                 are recorded in {})",
                self.session.path().display()
            ));
        }

        // The page's own turn, taken once the licences it waits behind have
        // been read and what they say has been ruled on, so a crossing its
        // terms refuse waits for nothing and leaves the host's turn to the
        // next request. A turn that does not fit releases what the allowance
        // reserved, as any refusal before the request does.
        if !discovery::take_page_turn(&mut declarations, &pacing, Utc::now()) {
            let reason = declarations
                .robots
                .delay_refusal()
                .unwrap_or_else(|| "the source's Crawl-delay could not be kept".to_owned());
            let mut facts = FetchFacts::refused(url, reason.clone());
            facts.breach = merge_breaches(
                ruling.reason().map(str::to_owned),
                breaches_of(&before).as_deref(),
            );
            facts.breach = merge_breaches(facts.breach, allowance_breach.as_deref());
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.allowance = self.release_authorisation(
                authorised,
                "the source's Crawl-delay refused the fetch before the request",
            );
            self.record(facts);
            return Err(format!(
                "refused before the crossing: {reason} The refusal is recorded in {}.",
                self.session.path().display()
            ));
        }

        let mut request = Request::get("/");
        request
            .headers
            .set("User-Agent", &self.identity.user_agent());
        request.headers.set("Accept", "*/*");
        // The correlation id of the standard's section 7.2, sent where the
        // host is known to take part: a manifest verified in the cache, or a
        // licence read before this request. The same id goes on the
        // crossing and from there onto the retrieval event, so the owner can
        // match the report to its own log line. A host that has declared
        // nothing is not told an id it has no way to use.
        let content_telemetry_id = (self
            .manifests
            .verified(&grounding::host_of(url), Utc::now())
            || declarations.licence_terms().is_some())
        .then(uuid::Uuid::new_v4);
        if let Some(id) = &content_telemetry_id {
            request.headers.set(CONTENT_TELEMETRY_ID, &id.to_string());
        }
        let policy = &self.policy;
        let allowed = |candidate: &str| -> Result<(), String> {
            if !policy.mediates_address(candidate) {
                return Err(format!(
                    "{candidate} is a local or private address, which Common Measure does not mediate."
                ));
            }
            self.keeps_off_hub(candidate)?;
            match policy.admit_host(candidate) {
                Ruling::Refused { reason, .. } => Err(reason.clone()),
                _ => Ok(()),
            }
        };
        let reaches = |candidate: &str, addresses: &[SocketAddr]| {
            reaches_allowed_addresses(policy, candidate, addresses)
        };
        let presented = self.identity.presented();
        // Each redirect hop is read and ruled on as the first URL was, against
        // the robots.txt and licence at its own origin, before the hop is
        // requested: a shortener's rule governs the shortener and the
        // destination's rule governs the destination, and every mode stops
        // before a disallowed hop is asked for. Being a redirect is no
        // exemption.
        let hops: RefCell<Vec<(Declarations, Vec<Ruling>)>> = RefCell::new(Vec::new());
        let on_hop = |hop_url: &str| -> Result<(), String> {
            // A hop is a further request, and the redirect chain is the part
            // of the call whose bounds add up past the host's own timeout.
            if pacing.over_ceiling() {
                return Err(format!(
                    "{hop_url} was not requested: {}",
                    pacing.ceiling_reason()
                ));
            }
            let terms = self.policy.terms_for(&grounding::host_of(hop_url)).cloned();
            // The hop's own origin states its own delay, and a hop to the
            // host already fetched waits for that host's turn. The hop's
            // access rule is ruled and a licence with no current reading is
            // read first; the hop's own turn is taken once its terms have
            // been ruled on and have not refused it.
            let mut hop = discovery::before_fetch(
                &self.declarations,
                hop_url,
                terms.as_ref(),
                Utc::now(),
                &|probe_url| self.probe(probe_url, &pacing),
                Some(&pacing),
                self.policy.mode(),
            );
            let rulings = self.rule_on_declarations(&mut hop);
            let mut refusal = first_refusal(&rulings).cloned();
            if refusal.is_none() && !discovery::take_page_turn(&mut hop, &pacing, Utc::now()) {
                refusal = hop.robots.delay_refusal().or_else(|| {
                    Some(format!(
                        "the {} `Crawl-delay` turn for this hop could not be kept",
                        grounding::host_of(hop_url)
                    ))
                });
            }
            hops.borrow_mut().push((hop, rulings));
            refusal.map_or(Ok(()), Err)
        };
        // No request of the chain is given longer than what is left of the
        // call's time limit, so the call ends inside it rather than at the
        // host's own timeout, which the caller cannot read.
        let followed = follow(
            url,
            request,
            &|| {
                pacing
                    .request_timeout(commonmeasure_http::CLIENT_TIMEOUT)
                    .ok_or_else(|| pacing.ceiling_reason())
            },
            self.identity.signer(),
            &allowed,
            &reaches,
            &on_hop,
        );
        // The record names the last hop that was evaluated and keeps every
        // earlier hop's evaluation beside it; a breach carried on any hop
        // stays on the record.
        let (mut declarations, earlier, before) =
            fold_hops((declarations, before), hops.into_inner());
        let (final_url, response) = match followed {
            Ok(reached) => reached,
            // A refused hop is enforcement, and enforcement is on the record:
            // the same `crossing_refused` a directly named URL earns, naming
            // the hop that was refused rather than the one asked for.
            Err(FetchFailure::Refused {
                url: refused,
                reason,
            }) => {
                let mut facts = FetchFacts::refused(&refused, reason.clone());
                // Every hop evaluated keeps its carried breaches on the
                // refusal: the asked-for URL's host breach (strict has
                // already returned on it, so it is observe's and counts
                // once), the allowance ruling observe carried, the hops
                // before the last, and the last hop's own (`before`).
                facts.breach = merge_breaches(
                    ruling.reason().map(str::to_owned),
                    allowance_breach.as_deref(),
                );
                facts.breach = merge_breaches(facts.breach, breaches_of(&earlier).as_deref());
                facts.breach = merge_breaches(facts.breach, breaches_of(&before).as_deref());
                facts.declarations = Some(declarations);
                facts.named_by = named_by;
                facts.content_telemetry_id = content_telemetry_id;
                facts.identity = Some(presented.clone());
                facts.allowance =
                    self.release_authorisation(authorised, "a redirect hop was refused");
                let refused_by = self.refused_by(facts.declarations.as_ref(), &refused);
                self.record(facts);
                let hop = if refused == url {
                    String::new()
                } else {
                    format!("a redirect to {refused}; ")
                };
                // The hub's origin is closed by the runtime, so the policy
                // file is not where an operator would look to change it.
                if reason == HUB_ORIGIN_REFUSAL {
                    let hop = hop.trim_end_matches("; ");
                    return Err(if hop.is_empty() {
                        format!("refused before the crossing: {reason}")
                    } else {
                        format!("refused before the crossing: {reason} ({hop})")
                    });
                }
                return Err(format!(
                    "refused before the crossing: {reason} ({hop}{refused_by})"
                ));
            }
            // Nothing answered, but the request left the machine, or this
            // edge stopped short of sending it. An unrecorded attempt would
            // make the log claim the agent never reached for this.
            Err(
                FetchFailure::Failed {
                    url: attempted,
                    detail,
                }
                | FetchFailure::NotSent {
                    url: attempted,
                    detail,
                },
            ) => {
                let mut facts = FetchFacts::carried(&attempted);
                facts.breach = merge_breaches(
                    self.breach_at(url, &attempted, &ruling),
                    breaches_of(&earlier).as_deref(),
                );
                facts.breach = merge_breaches(facts.breach, breaches_of(&before).as_deref());
                facts.breach = merge_breaches(facts.breach, allowance_breach.as_deref());
                facts.failure = Some(detail.clone());
                facts.declarations = Some(declarations);
                facts.named_by = named_by;
                facts.content_telemetry_id = content_telemetry_id;
                facts.identity = Some(presented.clone());
                facts.allowance =
                    self.release_authorisation(authorised, "the fetch failed before any receipt");
                self.record(facts);
                return Err(detail);
            }
        };

        let host_breach = merge_breaches(
            self.breach_at(url, &final_url, &ruling),
            allowance_breach.as_deref(),
        );
        let host_breach = merge_breaches(host_breach, breaches_of(&earlier).as_deref());
        // The receipt: the origin answered, and no rail paid anything, so the
        // charge is none and a held reservation is released on it.
        let allowance_record = self.settle_authorisation(authorised);
        if !(200..300).contains(&response.status) {
            let challenge = challenge_of(&response);
            let mut facts = FetchFacts::carried(&final_url);
            facts.http_status = Some(response.status);
            facts.breach = merge_breaches(host_breach, breaches_of(&before).as_deref());
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.content_telemetry_id = content_telemetry_id;
            facts.identity = Some(presented.clone());
            facts.challenge = challenge.clone();
            facts.allowance = allowance_record;
            self.record(facts);
            return Err(match challenge {
                // The message says exactly what the record says, and no more.
                // The agent repeats this to a person (`crate::nudge`), so an
                // inference drawn here — that a bare 403 was about who asked —
                // would reach the operator as a claim about a third party that
                // nothing establishes.
                Some(challenge) => format!(
                    "{final_url} refused the request: {challenge} Operator policy allowed the \
                     crossing and no content was retrieved; the attempt and the identity \
                     presented are recorded in {}.",
                    self.session.path().display()
                ),
                None => format!("{final_url} answered {}", response.status),
            });
        }

        // What the page itself declared, read from the response it came in.
        discovery::after_fetch(
            &self.declarations,
            &mut declarations,
            &final_url,
            &response,
            Utc::now(),
            &|probe_url| self.probe(probe_url, &pacing),
            Some(&pacing),
        );
        let after = self.rule_on_declarations(&mut declarations);
        let licence = licence_of(&declarations);

        // Embedded credentials are verified against the received body before
        // transformation removes them. The resulting text is what the screens
        // rule on and the agent reads. The extraction record
        // is written before the crossing and carries the hash of the bytes
        // received beside the hash of the text delivered, so the crossing's
        // two hashes are tied by a record a reader can re-derive. A body the
        // origin served under gzip arrives decoded, with its coded bytes kept,
        // and the retrieved hash is over those.
        let (extraction_invocation, extraction) = commonmeasure_runtime::processor::extract::invoke(
            &final_url,
            &response.body,
            response
                .coded
                .as_ref()
                .map(|coded| (coded.coding, coded.bytes.as_slice())),
            response.headers.get("Content-Type"),
        );
        let _ = self
            .session
            .record_processor(extraction_invocation.to_value());
        let basis = extraction.basis(&final_url);
        let text = extraction.text;
        let hash = extraction.content_hash;
        let retrieved_hash = extraction.retrieved_hash;
        let tokens = grounding::estimate_tokens(&text);

        // A statement the response carried can disallow the use the fetch
        // was for, and a reporting demand this session cannot meet refuses
        // in every mode. A refusal withholds the fetched bytes from context,
        // hash recorded, grounded false, the same shape as a PII refusal, and
        // keeps every other breach the declarations carried. Other
        // statements are refused under strict and carried with the breach
        // under observe and prefer.
        if let Some(reason) = first_refusal(&after) {
            let mut facts = FetchFacts::refused(&final_url, reason.clone());
            facts.http_status = Some(response.status);
            facts.content_hash = Some(hash);
            facts.retrieved_hash = Some(retrieved_hash);
            facts.estimated_tokens = Some(tokens);
            facts.breach = merge_breaches(host_breach, breaches_of(&after).as_deref());
            facts.licence = licence;
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.content_telemetry_id = content_telemetry_id;
            facts.identity = Some(presented.clone());
            facts.allowance = allowance_record.clone();
            self.record(facts);
            return Err(format!(
                "withheld from context: {reason} The bytes were fetched and are not returned; \
                 the statement and its source are recorded in {}.",
                self.session.path().display()
            ));
        }

        // The admit-stage screens run after the bytes exist and before
        // anything is returned or recorded as grounded: the one point where a
        // finding can still keep the text out of the agent's context. Each
        // verdict follows the same mode discipline as host policy — strict
        // refuses the crossing, observe and prefer carry it with the finding
        // recorded either way. The PII detector runs first, then the injection
        // screen; either refusing keeps the bytes out.
        let (pii_invocation, pii) = commonmeasure_runtime::processor::pii::invoke(
            self.policy.mode(),
            source_class(&self.policy, &final_url, None),
            self.policy.refuse_on_pii(),
            &final_url,
            &basis,
            &text,
            Some(&hash),
        );
        let _ = self.session.record_processor(pii_invocation.to_value());
        let (injection_invocation, injection) = commonmeasure_runtime::processor::injection::invoke(
            self.policy.mode(),
            &final_url,
            &basis,
            &text,
            Some(&hash),
        );
        let _ = self
            .session
            .record_processor(injection_invocation.to_value());
        let declared_breach = merge_breaches(host_breach, breaches_of(&after).as_deref());
        if let Some(reason) = [&pii, &injection]
            .into_iter()
            .find_map(|ruling| match ruling {
                Ruling::Refused { reason, .. } => Some(reason.clone()),
                _ => None,
            })
        {
            let mut facts = FetchFacts::refused(&final_url, reason.clone());
            facts.http_status = Some(response.status);
            facts.content_hash = Some(hash);
            facts.retrieved_hash = Some(retrieved_hash);
            facts.estimated_tokens = Some(tokens);
            facts.breach = declared_breach;
            facts.licence = licence;
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.content_telemetry_id = content_telemetry_id;
            facts.identity = Some(presented.clone());
            facts.allowance = allowance_record.clone();
            self.record(facts);
            // The page was fetched: the request left the machine and the
            // bytes are hashed on the refused crossing. What policy stopped
            // is the text entering the context, and the wording says that
            // rather than claiming no request was made.
            return Err(format!(
                "refused before the content entered the context: {reason} The page was \
                 fetched and its hash is on the record; the finding is recorded in {}.",
                self.session.path().display()
            ));
        }
        let breach = merge_breaches(declared_breach, pii.reason());
        let breach = merge_breaches(breach, injection.reason());
        // Discovery runs after the page bytes are in hand and never delays
        // the crossing's ruling: the manifest identifies the owner and its
        // telemetry endpoint and carries no demand. Its record is written
        // first so the crossing can reference it by sequence. A paced host is
        // the exception: its manifest was asked at the crossing's first free
        // turn above, because after the page it would never get one. Where a
        // redirect left that host, the host actually reached is resolved here
        // as an unpaced one is.
        let manifest_record = match &early_manifest {
            Some(record) if grounding::host_of(&final_url) == grounding::host_of(url) => {
                Some(*record)
            }
            _ => self.resolve_manifest(&final_url, &pacing, discovery::ManifestTurn::Paced),
        };
        let mut facts = FetchFacts::carried(&final_url);
        facts.manifest_record = manifest_record;
        facts.http_status = Some(response.status);
        facts.content_hash = Some(hash.clone());
        facts.retrieved_hash = Some(retrieved_hash.clone());
        facts.estimated_tokens = Some(tokens);
        facts.grounded = true;
        facts.breach = breach.clone();
        facts.licence = licence.clone();
        let summary = declarations_summary(&declarations);
        facts.declarations = Some(declarations);
        facts.named_by = named_by;
        facts.content_telemetry_id = content_telemetry_id;
        facts.identity = Some(presented.clone());
        facts.allowance = allowance_record.clone();
        self.record(facts);

        let acquisition_id = self.session.acquisition_handle()?;
        let mut result = json!({
            "url": final_url,
            "content_hash": hash,
            "retrieved_hash": retrieved_hash,
            "estimated_tokens": tokens,
            "token_basis": grounding::TOKEN_BASIS,
            "http_status": response.status,
            // Declared where the source published a machine-readable licence
            // or the operator holds terms for the host; unknown otherwise.
            // Reaching a page is not permission.
            "licence": licence,
            "declarations": summary,
            "policy": ruling.reason().unwrap_or("Admitted; no constraint excluded it."),
            "breach": breach,
            "named_by": named_by,
            "content_telemetry_id": content_telemetry_id,
            "allowance": allowance_record,
            "recorded_in": self.session.path().display().to_string(),
            "content": text,
        });
        if let Some(handle) = acquisition_id {
            result["acquisition_id"] = json!(handle);
        }
        Ok(result)
    }

    /// Settle a fetch's pre-authorisation against its receipt. No rail paid
    /// a quoted price here, so the receipt reports no charge in currency and
    /// the reservation is released on it, as the ledger does for a supplier
    /// receipt without money; the record says so. Returns the gate's record
    /// for the crossing.
    fn settle_authorisation(
        &mut self,
        authorised: Option<(AllowanceContext, GateDecision)>,
    ) -> Option<Value> {
        let (context, mut decision) = authorised?;
        let charge = AcquisitionCharge::default();
        self.settle_mediated(
            &context,
            &mut decision.record,
            decision.reservation.as_ref(),
            &charge,
        );
        decision.record["paid"] = json!(
            "nothing: no settlement rail is configured, so the quoted price was reserved and \
             released on the receipt, not paid"
        );
        Some(decision.record)
    }

    /// End a fetch's pre-authorisation when no receipt will come, as a failed
    /// search releases its reservation.
    fn release_authorisation(
        &mut self,
        authorised: Option<(AllowanceContext, GateDecision)>,
        reason: &str,
    ) -> Option<Value> {
        let (context, mut decision) = authorised?;
        if let Some(reservation) = decision.reservation.take() {
            self.release_mediated(&context, &mut decision.record, &reservation, reason);
        }
        Some(decision.record)
    }

    /// Resolve the host's discovery manifest and record it. Best-effort in
    /// the same sense as every record: a failure to write costs the
    /// reference, and the log owes a gap.
    fn resolve_manifest(
        &mut self,
        page_url: &str,
        pacing: &crate::crawl_delay::Pacing,
        turn: discovery::ManifestTurn,
    ) -> Option<u64> {
        let resolution = discovery::resolve_manifest(
            &self.manifests,
            page_url,
            Utc::now(),
            &|probe_url| self.probe(probe_url, pacing),
            Some(pacing),
            turn,
        )?;
        let host = grounding::host_of(page_url);
        self.session.record_manifest(&host, &resolution).ok()
    }

    /// Fetch one URL for a declaration or manifest probe, under the same host
    /// policy and address floor as a page and a shorter budget. A refused
    /// host is an error naming the refusal, so the record says why the
    /// document could not be read.
    ///
    /// A URL refused before it is sent is [`NotSent`](discovery::ProbeFailure::NotSent).
    /// A redirect to a URL this edge will not request is
    /// [`RedirectDeclined`](discovery::ProbeFailure::RedirectDeclined): the
    /// host answered, and pointed at something this edge does not fetch, so
    /// for `robots.txt` the file is unreachable (RFC 9309 §2.3.1.2), not
    /// absent. Past [`MAX_REDIRECTS`] the chain is a failure, and so
    /// unreachable too.
    ///
    /// No probe is given longer than what is left of the call's time limit,
    /// so a probe in flight never carries the call past it. A probe that
    /// limit ended early, or left no time for, or that could not be signed,
    /// is [`CutShort`](discovery::ProbeFailure::CutShort): the failure is
    /// this edge's, and the host is not held to have failed.
    fn probe(
        &self,
        url: &str,
        pacing: &crate::crawl_delay::Pacing,
    ) -> Result<(String, Response), discovery::ProbeFailure> {
        use discovery::ProbeFailure::{CutShort, NotSent, RedirectDeclined, Unreachable};
        if !self.policy.mediates_address(url) {
            return Err(NotSent(format!(
                "{url} is a local or private address, which Common Measure does not mediate"
            )));
        }
        if let Err(reason) = self.keeps_off_hub(url) {
            return Err(NotSent(format!("{url} is refused: {reason}")));
        }
        if let Ruling::Refused { reason, .. } = self.policy.admit_host(url) {
            return Err(NotSent(format!("{url} is refused by policy: {reason}")));
        }
        let mut request = Request::get("/");
        request
            .headers
            .set("User-Agent", &self.identity.user_agent());
        request.headers.set("Accept", "*/*");
        let policy = &self.policy;
        let allowed = |candidate: &str| -> Result<(), String> {
            if !policy.mediates_address(candidate) {
                return Err(format!(
                    "{candidate} is a local or private address, which Common Measure does not mediate"
                ));
            }
            self.keeps_off_hub(candidate)?;
            match policy.admit_host(candidate) {
                Ruling::Refused { reason, .. } => Err(reason.clone()),
                _ => Ok(()),
            }
        };
        let reaches = |candidate: &str, addresses: &[SocketAddr]| {
            reaches_allowed_addresses(policy, candidate, addresses)
        };
        // Whether the last request was given less than a whole probe's
        // budget because the call's time limit was nearer.
        let shortened = std::cell::Cell::new(false);
        follow(
            url,
            request,
            &|| {
                let timeout = pacing
                    .request_timeout(discovery::PROBE_TIMEOUT)
                    .ok_or_else(|| pacing.ceiling_reason())?;
                shortened.set(timeout < discovery::PROBE_TIMEOUT);
                Ok(timeout)
            },
            self.identity.signer(),
            &allowed,
            &reaches,
            &|_| Ok(()),
        )
        .map_err(|failure| match failure {
            FetchFailure::Refused {
                url: target,
                reason,
            } if target != url => RedirectDeclined {
                reason: format!(
                    "{url} redirected to {target}, which this edge does not follow: {reason}"
                ),
                target,
            },
            FetchFailure::Refused { url, reason } if reason == HUB_ORIGIN_REFUSAL => {
                NotSent(format!("{url} is refused: {reason}"))
            }
            FetchFailure::Refused { url, reason } => {
                NotSent(format!("{url} is refused by policy: {reason}"))
            }
            FetchFailure::NotSent { detail, .. } => CutShort(detail),
            // A request given less than a whole probe's budget that fails
            // with the call's time used up was ended by this call, not by
            // the host: a host that would have answered within
            // `PROBE_TIMEOUT` is not recorded as unreachable.
            FetchFailure::Failed { url, detail } if shortened.get() && pacing.over_ceiling() => {
                CutShort(format!(
                    "{url} was given only what was left of this call's time limit and did not \
                     answer within it ({detail}): {}",
                    pacing.ceiling_reason()
                ))
            }
            FetchFailure::Failed { url, detail } => Unreachable(format!("{url}: {detail}")),
        })
    }

    /// Where the rule that refused `refused_url` before the crossing is
    /// written, for the reader to look: the source's own `robots.txt` where
    /// its access rule refused that URL, else the operator's policy file.
    /// A `robots.txt` that is unreachable because its redirect went to a
    /// host the operator's policy refuses, or to a private address the
    /// policy could admit, is the policy's to change, so that refusal names
    /// the policy file. A file this edge cut short is nobody's rule.
    fn refused_by(&self, declarations: Option<&Declarations>, refused_url: &str) -> String {
        let policy = format!("operator policy in {}", self.policy.source().display());
        match declarations.map(|declarations| &declarations.robots) {
            Some(robots) if robots.refuses() && robots.requested_url == refused_url => {
                match &robots.declined_redirect {
                    Some(target)
                        if robots.outcome == Some(discovery::RobotsRuling::Unreachable)
                            && (self.policy.admit_host(target).is_refusal()
                                || (!self.policy.mediates_address(target)
                                    && !self.policy.holds_private_floor())) =>
                    {
                        format!(
                            "{policy}, which does not admit {target}, where the source's \
                             robots.txt redirected"
                        )
                    }
                    // Neither the source nor the policy refused: this edge
                    // did not read the file, and asks again next time.
                    _ if robots.outcome == Some(discovery::RobotsRuling::CutShort) => format!(
                        "this edge did not read {}, and the next crossing asks for it again",
                        robots.url
                    ),
                    _ => format!("the source's robots.txt, {}", robots.url),
                }
            }
            _ => policy,
        }
    }

    /// Refuse a URL whose authority is one of the enrolled hub's. Every
    /// mediated request is signed with the enrolled key, the key the hub's
    /// own routes authenticate this edge by, and the URL is one an agent or a
    /// publisher chose: a direct fetch, a redirect, or a licence link a page
    /// names. Refused rather than sent unsigned, because the agent has no
    /// business at the hub's API either way.
    fn keeps_off_hub(&self, url: &str) -> Result<(), String> {
        match HubAuthority::of(url) {
            Some(asked) if self.hub_authorities.iter().any(|hub| hub.covers(&asked)) => {
                Err(HUB_ORIGIN_REFUSAL.to_owned())
            }
            _ => Ok(()),
        }
    }

    /// Rule on what the source declared, under the session's mode: the
    /// robots.txt access rule for the product token, a disallowed AI-input
    /// statement, a licence whose AI-input permission is conditional on a
    /// payment this edge cannot make, and a reporting demand this session
    /// cannot meet. An applicable operator assessment within its content
    /// and use scope governs source preferences and licence terms; its
    /// reporting duty still applies.
    ///
    /// Every ruling here follows the session's mode except the access rule
    /// and a licence's reporting demand, which refuse the crossing in every
    /// mode ([`binding_breach`]).
    fn rule_on_declarations(&self, declarations: &mut Declarations) -> Vec<Ruling> {
        let mode = self.policy.mode();
        let mut rulings = Vec::new();
        let mut reporting = None;
        if let Some(attribution) = declarations.robots.rule(mode) {
            rulings.push(match declarations.robots.outcome {
                Some(discovery::RobotsRuling::Unreachable) => binding_breach(
                    format!(
                        "{attribution}. An unreachable robots.txt is a complete disallow in \
                         every policy mode: refused before the request."
                    ),
                    "The source's robots.txt could not be reached and no copy of it is held.",
                ),
                Some(discovery::RobotsRuling::CutShort) => binding_breach(
                    format!("{attribution}: refused before the request, in every policy mode."),
                    "The source's robots.txt was not read, because this edge's own request for \
                     it failed, and no copy of it is held.",
                ),
                _ => binding_breach(
                    format!(
                        "{attribution}. A `Disallow` binds in every policy mode: refused before \
                         the request."
                    ),
                    "The source's robots.txt disallows this fetcher at this path.",
                ),
            });
        }
        if let Some(breach) = self.robots_redirect_breach(&declarations.robots) {
            rulings.push(breach);
        }
        match declarations.governing {
            Governing::OperatorTerms => {
                let terms = declarations.terms.as_ref();
                if terms.is_some_and(|terms| terms.requires_reporting) {
                    let ruling = self.reporting_ruling_for(None, None);
                    if let Some(reason) = &ruling.reason {
                        rulings.push(breach(
                            mode,
                            format!(
                                "The operator's terms {} require usage reporting, and the duty \
                                 cannot be met: {reason}.",
                                terms.map(|t| t.reference.as_str()).unwrap_or_default()
                            ),
                            "A reporting duty the operator declared cannot be met by this session.",
                        ));
                    }
                    reporting = Some(ruling);
                }
            }
            Governing::Statements => {
                // A licence the source names that could not be read has terms
                // nobody knows, not no terms. It may carry a reporting demand,
                // which binds in every policy mode, so the page is not admitted
                // under it in any mode (owner decision, 22 September 2026:
                // content whose reporting and licence requirements are not
                // respected is not had). A failed read is remembered for the
                // failure age and asked again after it. A licence that answered
                // 404 or 410 is missing, not unread: it has no terms, the
                // crossing proceeds, and its gap is on the record.
                for licence in declarations
                    .licences
                    .iter()
                    .filter(|licence| licence.unread)
                {
                    rulings.push(Ruling::Refused {
                        reason: format!(
                            "The source names the licence {}, which could not be read ({}), so \
                             its terms, including any reporting demand, are unknown and the page \
                             is not admitted under them in any policy mode.",
                            licence.url,
                            licence
                                .unavailable
                                .as_deref()
                                .unwrap_or("no reason recorded")
                        ),
                        gap: Gap::new(
                            GapReason::PolicyRefused,
                            "A licence the source names could not be read, so its terms could not \
                             be kept.",
                        ),
                    });
                }
                if declarations.ai_input() == Effective::Disallow {
                    let sources: Vec<String> = declarations
                        .ai_input_disallowed_by()
                        .iter()
                        .map(|statement| statement.detail.clone())
                        .collect();
                    rulings.push(breach(
                        mode,
                        format!(
                            "The source disallows AI input ({}); no operator terms override it.",
                            sources.join("; ")
                        ),
                        "The source's declared preference disallows the use this crossing makes.",
                    ));
                }
                if let Some((licence, terms)) = declarations.licence_terms()
                    && declarations.ai_input() == Effective::Allow
                {
                    if let Some(payment) = terms.payment.as_ref().filter(|p| p.is_monetary()) {
                        rulings.push(breach(
                            mode,
                            format!(
                                "The licence {} permits AI input under payment type {}{}, and \
                                 this edge holds no settlement rail, so the payment term is unmet.",
                                licence.url,
                                payment.kind.as_deref().unwrap_or("unstated"),
                                payment
                                    .amount
                                    .as_ref()
                                    .map(|amount| format!(
                                        " ({} {})",
                                        amount.decimal, amount.currency
                                    ))
                                    .unwrap_or_default()
                            ),
                            "A payment the licence requires cannot be made: no settlement rail is \
                             configured (unavailable dependency).",
                        ));
                    } else if terms.server.is_some() {
                        rulings.push(breach(
                            mode,
                            format!(
                                "The licence {} names a licence server ({}) a client must obtain a \
                                 licence from before access, and this edge holds no rail to do so.",
                                licence.url,
                                terms.server.as_deref().unwrap_or_default()
                            ),
                            "A licence token the licence requires cannot be obtained: no rail is \
                             configured (unavailable dependency).",
                        ));
                    }
                }
                // A licence's reporting demands are ruled on whatever the
                // combined AI-use preference: a Disallow from robots.txt or a
                // `Content-Usage` header beside a licence that demands
                // reporting is carried as a breach outside `strict`, and the
                // bytes it lets through are still bound by the demand (owner
                // decision, 22 September 2026). Which demands apply is
                // settled in `declarations::licence_terms`.
                if let Some((licence, terms)) = declarations.licence_terms() {
                    for demand in &terms.reporting {
                        // RSL 1.0 §3.12: a client satisfies every applicable
                        // demand or treats the activity as unlicensed. This
                        // runtime reports Content Telemetry events and
                        // nothing else, so a demand of any other type,
                        // stated or not, is unmet whatever the session
                        // clears. `declarations.reporting` stays the
                        // telemetry ruling; the breach names this one.
                        if demand.kind != "telemetry" {
                            let unstated = |value: &str| {
                                if value.is_empty() {
                                    "unstated".to_owned()
                                } else {
                                    value.to_owned()
                                }
                            };
                            rulings.push(binding_breach(
                                format!(
                                    "The licence {} requires reporting of type {} (profile {}) and \
                                     the demand cannot be met: this runtime reports telemetry \
                                     only.",
                                    licence.url,
                                    unstated(&demand.kind),
                                    unstated(&demand.profile)
                                ),
                                "A reporting demand the licence carries cannot be met by this session.",
                            ));
                            continue;
                        }
                        let ruling = self.reporting_ruling(demand);
                        if let Some(reason) = &ruling.reason {
                            rulings.push(binding_breach(
                                format!(
                                    "The licence {} requires telemetry reporting (profile {}) and the \
                                     demand cannot be met: {reason}.",
                                    licence.url, demand.profile
                                ),
                                "A reporting demand the licence carries cannot be met by this session.",
                            ));
                        }
                        reporting = Some(ruling);
                    }
                }
            }
        }
        if let Some(decision) = &mut declarations.assessment_decision {
            decision.mode = Some(mode);
            decision.outcome = Some(
                if first_refusal(&rulings).is_some() {
                    "refused"
                } else if rulings
                    .iter()
                    .any(|r| matches!(r, Ruling::AllowedWithBreach { .. }))
                {
                    "allowed_with_breach"
                } else {
                    "allowed"
                }
                .to_owned(),
            );
        }
        declarations.reporting = reporting;
        rulings
    }

    /// Rule on a licence's telemetry reporting demand: the profile must be
    /// the Content Telemetry binding this relay speaks, the demanded
    /// conformance level one it emits (retrieval or grounding), and the
    /// session must be able to deliver ([`Self::reporting_ruling_for`]), or
    /// nothing would ever be reported. An unrecognised profile is a demand
    /// this client cannot comply with, which RSL says leaves the activity
    /// unlicensed.
    fn reporting_ruling(
        &self,
        demand: &crate::declarations::RslReporting,
    ) -> crate::discovery::ReportingRuling {
        let level = demand
            .config
            .as_ref()
            .and_then(|config| config["conformance_level"].as_str())
            .map(str::to_owned);
        let mut ruling = self.reporting_ruling_for(Some(demand.profile.clone()), level.clone());
        if demand.profile != TELEMETRY_PROFILE {
            ruling.reason = Some(format!(
                "this runtime reports under {TELEMETRY_PROFILE} only and does not recognise the \
                 profile"
            ));
        } else {
            match level.as_deref() {
                Some("retrieval" | "grounding") => {}
                Some(other) => {
                    ruling.reason = Some(format!(
                        "the licence demands {other} conformance and this runtime emits retrieval \
                         and grounding events only"
                    ));
                }
                None => {
                    ruling.reason = Some(
                        "the reporting configuration names no conformance_level this runtime can \
                         read"
                            .to_owned(),
                    );
                }
            }
        }
        ruling.met = ruling.reason.is_none();
        ruling
    }

    /// The session's half of any reporting ruling: whether the scope clears
    /// telemetry egress, a receiver is configured and delivery happens
    /// without a person, with the receiver named in the record.
    ///
    /// The third check is the owner decision of 22 September 2026: a demand
    /// is met only where the events will actually leave, so an operator who
    /// switched automatic delivery off has an unmet demand until they switch
    /// it back on, and the reason names the marker that caused it.
    fn reporting_ruling_for(
        &self,
        profile: Option<String>,
        conformance_level: Option<String>,
    ) -> crate::discovery::ReportingRuling {
        let cleared = self.policy.allows_telemetry_egress();
        let reason = if !cleared {
            Some(
                "this session's policy scope clears no telemetry egress, so nothing would be \
                 reported"
                    .to_owned(),
            )
        } else if self.receiver.is_none() {
            Some(format!(
                "no telemetry receiver is configured in {}, so nothing would be reported",
                self.policy.source().with_file_name("relay.json").display()
            ))
        } else {
            crate::delivery::SessionDelivery {
                host: &self.host,
                client: self.client.as_ref().map(|client| client.name.as_str()),
                interval_relay: self.interval_relay,
            }
            .withheld_reason(self.home())
        };
        crate::discovery::ReportingRuling {
            profile,
            conformance_level,
            receiver: self.receiver.clone(),
            telemetry_egress_cleared: cleared,
            met: reason.is_none(),
            reason,
        }
    }

    /// The host-policy breach of a `robots.txt` request this crossing sent
    /// that a redirect took to another host, judged against the URL that
    /// answered as a page hop's is ([`McpServer::breach_at`]). Under
    /// `observe` and `prefer` the operator's host rules record a breach
    /// rather than refuse, so the redirect was followed; the request left
    /// this edge for that host, and the crossing carries the breach. A copy
    /// reused from the cache sent nothing this time and carries none.
    fn robots_redirect_breach(&self, robots: &discovery::RobotsOutcome) -> Option<Ruling> {
        if robots.cache != discovery::CacheDecision::Fetched {
            return None;
        }
        let answered = robots.final_url.as_deref()?;
        if grounding::host_of(answered) == grounding::host_of(&robots.url) {
            return None;
        }
        match self.policy.admit_host(answered) {
            Ruling::AllowedWithBreach { reason, gap } => Some(Ruling::AllowedWithBreach {
                reason: format!(
                    "{} redirected to {answered}, which was requested for its rules: {reason}",
                    robots.url
                ),
                gap,
            }),
            _ => None,
        }
    }

    /// The breach a crossing carries, if the mode carried it rather than
    /// refusing. Judged against the URL that actually answered, since a
    /// redirect can leave the hosts the operator allowed: the mode changed
    /// what happened to the crossing, not whether it is on record.
    fn breach_at(&self, asked: &str, reached: &str, ruling: &Ruling) -> Option<String> {
        if reached == asked {
            ruling.reason().map(str::to_owned)
        } else {
            self.policy.admit_host(reached).reason().map(str::to_owned)
        }
    }

    fn tool_search(&mut self, arguments: &Value) -> Result<Value, String> {
        self.governed_search(arguments, false)
    }

    pub(crate) fn governed_search(
        &mut self,
        arguments: &Value,
        redact_supplier_errors: bool,
    ) -> Result<Value, String> {
        self.record_session_start()?;
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .ok_or("query is required")?;
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 20) as u32;
        let provider = arguments
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("exa");

        self.instance_permits("context_search", None)?;

        let provider_ruling = self.policy.permits_provider(provider);
        if provider_ruling.reason().is_some() {
            self.session
                .record_provider_ruling(&self.host, provider, &provider_ruling)
                .map_err(|error| format!("could not record provider policy: {error}"))?;
        }
        if provider_ruling.is_refusal() {
            return Err(format!(
                "refused before the crossing: {}",
                provider_ruling.reason().unwrap_or_default()
            ));
        }

        let supplier = supplier_with_released(provider, self.released.as_ref()).map_err(
            |error| match error {
                SupplyError::CredentialMissing { variable } => {
                    unavailable_credential(provider, &variable, self.credentials.path.as_path())
                }
                _ if redact_supplier_errors => {
                    "unavailable: provider configuration could not be loaded".to_owned()
                }
                other => other.to_string(),
            },
        )?;
        // Dispatch on the capability the adapter declares, and only within
        // what this surface mediates. The internal corpus declares `query`
        // alone; a skill declares `invoke` alone and is refused here by name.
        // A mediated tool has no purchase decision and no admission budget of
        // the run's kind, so it must neither spend the operator's balance nor
        // execute anything — and a fall-through that called `query` on
        // whatever arrived would rest that guarantee on the adapter happening
        // to refuse rather than on this surface deciding.
        let mediated = supplier
            .capabilities()
            .iter()
            .find(|capability| MEDIATED_CAPABILITIES.contains(capability))
            .copied();
        // The principal's cumulative allowance gates a mediated search the
        // same way it gates a run's dispatch: reserve the adapter's declared
        // published price where one exists, refuse an exhausted allowance
        // under strict before anything crosses, and commit the observed
        // charge on the receipt. `query` is never gated — the internal
        // corpus is the operator's own supply and no money can move.
        let mut consulted: Option<(AllowanceContext, GateDecision)> = None;
        if mediated == Some(ProviderCapability::Search) && !self.policy.allowances().is_empty() {
            // The policy's source is `<home>/policy.json`, so its parent is
            // the operator home the ledger lives in.
            let Some(home) = self
                .policy
                .source()
                .parent()
                .map(std::path::Path::to_path_buf)
            else {
                return Err(format!(
                    "unavailable: the policy source {} has no parent directory, so the \
                     allowance ledger cannot be located and the declared allowance cannot be \
                     enforced. No search was attempted.",
                    self.policy.source().display()
                ));
            };
            let context = AllowanceContext::new(
                &home,
                self.policy.principal().to_owned(),
                self.policy.allowances().to_vec(),
            );
            let decision = context.pre_dispatch(
                supplier
                    .published_price(ProviderCapability::Search)
                    .as_ref(),
                self.policy.mode(),
                Utc::now(),
                &format!("session {}", self.session.session_id()),
            );
            if let Some(ruling) = &decision.ruling
                && ruling.is_refusal()
            {
                return Err(format!(
                    "refused before the crossing: {} (operator policy in {})",
                    ruling.reason().unwrap_or_default(),
                    self.policy.source().display()
                ));
            }
            consulted = Some((context, decision));
        }
        let acquired = match mediated {
            // Unscoped: a mediated search carries no job, so no allowed-host
            // list scopes it; the session's source policy still rules each
            // crossing.
            Some(ProviderCapability::Search) => supplier.search(query, limit, &[]),
            Some(ProviderCapability::Query) => supplier.query(query, limit),
            _ => {
                return Err(format!(
                    "unavailable: {provider} declares no capability this session surface \
                     mediates. Mediated tools search and query; they never invoke a skill or \
                     settle a purchase, because this surface takes no decision that could \
                     authorise either."
                ));
            }
        };
        let acquisition = match acquired {
            Ok(acquisition) => acquisition,
            Err(error) => {
                // A held reservation outlives no failed search, and a
                // release the ledger refuses is stated in the tool error
                // beside the failure that caused it.
                let mut detail = if redact_supplier_errors {
                    "unavailable: supplier request failed; no usable receipt was returned (charge and supplier latency unknown)".to_owned()
                } else {
                    error.to_string()
                };
                if let Some((context, mut decision)) = consulted
                    && let Some(reservation) = decision.reservation.take()
                    && let Some(gap) = self.release_mediated(
                        &context,
                        &mut decision.record,
                        &reservation,
                        &format!("the search failed before any receipt ({detail})"),
                    )
                {
                    detail = format!("{detail}; {gap}");
                }
                return Err(detail);
            }
        };
        let mut delivery = self.deliver(&acquisition);
        if let Some(reason) = provider_ruling.reason() {
            delivery["provider_policy_breach"] = json!(reason);
        }
        if let Some((context, mut decision)) = consulted {
            self.settle_mediated(
                &context,
                &mut decision.record,
                decision.reservation.as_ref(),
                &acquisition.charge,
            );
            delivery["allowance"] = decision.record;
        }
        Ok(delivery)
    }

    /// Execute selected providers through the same admission and allowance
    /// path as MCP, persisting each outcome before attempting the next.
    pub(crate) fn compare(
        &mut self,
        request: &crate::compare::CompareRequest,
    ) -> Result<Value, String> {
        let mut results = Vec::new();
        for provider in &request.providers {
            self.session
                .record_comparison("comparison_provider_started", json!({"provider": provider}))
                .map_err(|e| format!("comparison evidence unavailable before dispatch: {e}"))?;
            let started = std::time::Instant::now();
            let arguments = json!({"query": request.query, "provider": provider, "limit": crate::compare::RESULT_LIMIT});
            let mut result = match self.governed_search(&arguments, true) {
                Ok(mut result) => {
                    result["status"] = json!("completed");
                    result["results_count"] =
                        json!(result["results"].as_array().map_or(0, Vec::len));
                    result["cost"] = result["charge"].clone();
                    // The private-address record floor also applies to the
                    // comparison summary; no snippet text is retained here.
                    if let Some(items) = result["results"].as_array_mut() {
                        items.retain(|item| {
                            item["url"]
                                .as_str()
                                .is_some_and(|url| self.policy.records_address(url))
                        });
                        for item in items {
                            if let Some(object) = item.as_object_mut() {
                                object.remove("text");
                            }
                        }
                    }
                    result["metadata_withheld"] = json!(
                        result["results_count"].as_u64().unwrap_or(0)
                            - result["results"].as_array().map_or(0, Vec::len) as u64
                    );
                    result
                }
                Err(reason) => {
                    json!({"provider": provider, "status": if reason.starts_with("refused") { "refused" } else { "unavailable" }, "error": reason, "cost": null, "latency_ms": null})
                }
            };
            result["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
            result["latency_basis"] = json!(
                "adapter request wall-clock; elapsed_ms includes policy, admission and recording"
            );
            if let Some(reason) = self.evidence_error.take() {
                return Err(format!(
                    "comparison source evidence incomplete: {reason}; supplier calls may have incurred charges"
                ));
            }
            self.session.record_comparison("comparison_provider_finished", result.clone())
                .map_err(|e| format!("comparison result could not be recorded: {e}; supplier calls may have incurred charges"))?;
            results.push(result);
        }
        let result = json!({"schema": crate::compare::SCHEMA, "kind": "completed", "status": 200,
            "notice": "Retrieval comparison recorded locally. No answer-quality evaluation was run.",
            "comparison_id": self.session.session_id(), "query": request.query,
            "selected_providers": request.providers, "requested_limit": crate::compare::RESULT_LIMIT,
            "effective_limit": crate::compare::RESULT_LIMIT, "results": results,
            "recorded_in": self.session.path().display().to_string(), "sharing": "local_only",
            "cwd": self.cwd, "policy_identity": self.policy.identity(), "policy_mode": self.policy.mode(),
            "principal": self.policy.principal(), "authentication_basis": self.policy.authentication_basis().as_str()});
        self.session.record_comparison("comparison_finished", result.clone())
            .map_err(|e| format!("comparison completion could not be recorded: {e}; supplier calls may have incurred charges"))?;
        Ok(result)
    }

    /// End a reservation after a failed search, checked as the run path
    /// checks it. A ledger that refuses the release leaves the reservation
    /// held until the expiry sweep releases it; that is recorded as an
    /// allowance gap in the session evidence and returned for the tool
    /// error, so the log and the agent both state it before the sweep runs.
    fn release_mediated(
        &mut self,
        context: &AllowanceContext,
        record: &mut Value,
        reservation: &Reservation,
        reason: &str,
    ) -> Option<String> {
        let error = context
            .release_after_failure(record, reservation, reason, Utc::now())
            .err()?;
        let detail = format!(
            "the allowance reservation {} could not be released ({error}); it stays held until \
             the expiry sweep releases it",
            reservation.id
        );
        if let Err(error) =
            self.session
                .record_allowance_gap(&self.host, Some(reservation.id), None, &detail)
        {
            self.evidence_error = Some(error.to_string());
        }
        Some(detail)
    }

    /// Settle the gate's outcome against the receipt, as the run path does.
    /// The ledger retries the write once under its lock; a settlement it
    /// still does not take is an allowance gap in the session evidence
    /// naming the reservation, the observed charge and the reason, because
    /// the money has moved and the ledger's total omits it.
    fn settle_mediated(
        &mut self,
        context: &AllowanceContext,
        record: &mut Value,
        reservation: Option<&Reservation>,
        charge: &AcquisitionCharge,
    ) {
        if let Err(failure) = context.settle_success(
            record,
            reservation,
            charge,
            Utc::now(),
            &format!("session {}", self.session.session_id()),
        ) && let Err(error) = self.session.record_allowance_gap(
            &self.host,
            failure.reservation_id,
            failure.observed.as_ref(),
            &failure.to_string(),
        ) {
            self.evidence_error = Some(error.to_string());
        }
    }

    /// Judge, record and shape what a provider returned. Separate from
    /// reaching the provider, which is the half that needs a credential.
    fn deliver(&mut self, acquisition: &Acquisition) -> Value {
        let mut results = Vec::new();
        let mut refusals = Vec::new();
        for envelope in &acquisition.envelopes {
            // The privacy floor is the record-admission predicate whichever
            // pipe carried the fact: a provider naming a private address is
            // still the operator's own business, and the observed path drops
            // the identical fact. It is still judged and can still be refused
            // — only the record is withheld, here and for the processor
            // invocation that would otherwise name the same address.
            let recordable = self.policy.records_address(&envelope.source_url);
            let ruling = self
                .policy
                .admit_from_provider(envelope, Some(&acquisition.provider));
            // A search snippet enters the agent's context like any other
            // text, so the admit-stage screens scan it under the same mode
            // discipline as a fetched page: the PII detector, then the
            // injection screen.
            let (pii, injection) = match (ruling.is_refusal(), envelope.text.as_deref()) {
                (false, Some(text)) => {
                    let (pii_invocation, pii) = commonmeasure_runtime::processor::pii::invoke(
                        self.policy.mode(),
                        source_class(
                            &self.policy,
                            &envelope.source_url,
                            Some(&acquisition.provider),
                        ),
                        self.policy.refuse_on_pii(),
                        &envelope.source_url,
                        &envelope.source_url,
                        text,
                        envelope.content_hash.as_deref(),
                    );
                    let (injection_invocation, injection) =
                        commonmeasure_runtime::processor::injection::invoke(
                            self.policy.mode(),
                            &envelope.source_url,
                            &envelope.source_url,
                            text,
                            envelope.content_hash.as_deref(),
                        );
                    if recordable {
                        if let Err(error) = self.session.record_processor(pii_invocation.to_value())
                        {
                            self.evidence_error = Some(error.to_string());
                        }
                        if let Err(error) = self
                            .session
                            .record_processor(injection_invocation.to_value())
                        {
                            self.evidence_error = Some(error.to_string());
                        }
                    }
                    (Some(pii), Some(injection))
                }
                _ => (None, None),
            };
            let screen_refusal = [pii.as_ref(), injection.as_ref()]
                .into_iter()
                .flatten()
                .filter(|ruling| ruling.is_refusal())
                .find_map(Ruling::reason)
                .map(str::to_owned);
            let refusal = if ruling.is_refusal() {
                Some(ruling.reason().unwrap_or_default().to_owned())
            } else {
                screen_refusal
            };
            let refused = refusal.is_some();
            let breach = (!refused)
                .then(|| {
                    let breach = merge_breaches(
                        ruling.reason().map(str::to_owned),
                        pii.as_ref().and_then(Ruling::reason),
                    );
                    merge_breaches(breach, injection.as_ref().and_then(Ruling::reason))
                })
                .flatten();
            if recordable {
                if let Some(reason) = &refusal {
                    refusals.push(json!({"url": envelope.source_url, "reason": reason}));
                }
                self.record(FetchFacts::delivered(
                    &envelope.source_url,
                    &acquisition.provider,
                    envelope.content_hash.clone(),
                    envelope.text.as_deref().map(grounding::estimate_tokens),
                    refusal,
                    breach,
                    envelope.licence.clone(),
                ));
            }
            if refused {
                continue;
            }
            // The licence and date exactly as the supplier declared them —
            // for the internal corpus a declaration the operator wrote in
            // corpus.json, and flattening it to "unknown" would falsify it.
            // Where nothing was declared, unknown stays unknown.
            results.push(json!({
                "url": envelope.source_url,
                "title": envelope.title,
                "text": envelope.text,
                "content_hash": envelope.content_hash,
                "licence": envelope.licence,
                "declared_date": envelope.declared_date,
            }));
        }

        json!({
            "provider": acquisition.provider,
            "results": results,
            "received": acquisition.envelopes.len(),
            "refused": acquisition.envelopes.len() - results.len(),
            "refusals": refusals,
            "charge": acquisition.charge,
            "capability": acquisition.capability,
            "endpoint": acquisition.endpoint,
            "http_status": acquisition.http_status,
            "provider_request_id": acquisition.provider_request_id,
            "adapter_version": commonmeasure_supply::ADAPTER_VERSION,
            "response_sha256": commonmeasure_types::canonical::sha256_digest(&acquisition.raw_response),
            "latency_ms": acquisition.latency_ms,
            "recorded_in": self.session.path().display().to_string(),
        })
    }

    fn tool_enrol(&mut self, arguments: &Value) -> Result<Value, String> {
        use crate::directory;
        let named = arguments["directory"]
            .as_str()
            .ok_or("name the actual host project directory explicitly")?;
        let root = directory::selected(std::path::Path::new(named))?;
        let server = self
            .cwd
            .as_deref()
            .ok_or("MCP process directory is unknown")?;
        if directory::selected(std::path::Path::new(server))? != root {
            return Err(format!(
                "MCP process directory {server} differs from selected {}; restart this server in the host project before claiming governance",
                root.display()
            ));
        }
        let home = self
            .session
            .path()
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or("session home unavailable")?
            .to_path_buf();
        match arguments["action"].as_str().unwrap_or("status") {
            "status" => {}
            "sync" => directory::sync_all(&home)?,
            "enrol" => {
                let name = arguments["name"].as_str().ok_or("ask for a project name")?;
                let reporting = match arguments["reporting"].as_str() {
                    Some("hub") => true,
                    Some("local") => false,
                    _ => return Err("ask for local recording or hub reporting".into()),
                };
                if reporting && arguments["include_history"] != true {
                    return Err("reporting covers this canonical root and descendants, including existing eligible witnessed evidence; include_history must confirm this coverage".into());
                }
                let project = directory::Registry::enrol(&home, &root, name, reporting)?;
                if reporting && crate::managed::is_managed(&home)? {
                    directory::request(&home, &project)?;
                    directory::sync(&home)?;
                }
            }
            "remove" => {
                let registry =
                    directory::Registry::read(&home)?.ok_or("directory is not enrolled")?;
                let project = registry
                    .matching(named)
                    .ok_or("directory is not enrolled")?;
                if project.root != root {
                    return Err("remove reporting at the enrolled root".into());
                }
                let project = directory::Registry::enrol(&home, &root, &project.name, false)?;
                if crate::managed::is_managed(&home)? {
                    let _ = directory::request(&home, &project);
                }
            }
            _ => return Err("action must be status, enrol, remove or sync".into()),
        }
        self.policy = SessionPolicy::load(&home, self.cwd.as_deref())?;
        directory::status(&home, &root)
    }

    fn tool_status(&self) -> Value {
        let released = self.released_standing();
        // Only providers this surface can actually reach: those whose
        // declared capability `context_search` can dispatch (`search`, or
        // `query` for the internal corpus). Anything else would be
        // advertised `configured: true` while every call to it failed
        // `CapabilityUnavailable`, and the list would drift from the sealed
        // enum in the tool's own schema. A quote-then-buy provider is
        // excluded even where it also declares `search`: buying needs a
        // purchase decision this surface does not have, and advertising the
        // provider here would let a session tool spend the operator's
        // balance silently.
        let providers: Vec<Value> = commonmeasure_supply::IMPLEMENTED_PROVIDERS
            .iter()
            .filter(|provider| {
                commonmeasure_supply::declared_provider_ref(provider).is_some_and(|declared| {
                    declared
                        .capabilities
                        .iter()
                        .any(|capability| MEDIATED_CAPABILITIES.contains(capability))
                        && !declared.capabilities.contains(&ProviderCapability::Quote)
                })
            })
            .map(|provider| {
                let configured = supplier_with_released(provider, self.released.as_ref()).is_ok();
                // Where a configured provider's variable came from: set into
                // the environment from the operator file at start, already
                // in the environment the harness was launched with, or
                // released by the hub where neither holds it. Names only —
                // the value itself is never reported.
                let source = configured
                    .then(|| commonmeasure_supply::required_variable(provider))
                    .flatten()
                    .map(|variable| {
                        let from_file = self.credentials.loaded.as_ref().is_some_and(|loaded| {
                            loaded.applied.iter().any(|name| name == variable)
                        });
                        let from_hub = released.as_ref().is_some_and(|standing| {
                            standing.in_use.iter().any(|name| name.variable == variable)
                        });
                        if from_file {
                            "operator-file"
                        } else if from_hub {
                            "hub"
                        } else {
                            "environment"
                        }
                    });
                json!({
                    "provider": provider,
                    "configured": configured,
                    "source": source,
                })
            })
            .collect();
        json!({
            "session_id": self.session.session_id(),
            "cwd": self.cwd,
            "host": self.host,
            "client": self.client,
            "evidence": self.session.path().display().to_string(),
            "policy": self.policy.describe(),
            "credentials": credentials_payload(&self.credentials, released.as_ref()),
            "providers": providers,
            "processors": commonmeasure_runtime::processor::installed(),
        })
    }

    /// The registered instance's authority, checked before anything crosses
    /// (`crate::instance::Retained::check`). A session with no registration
    /// passes untouched. Where the session is registered, an ended validity
    /// window refuses the work and a check that cannot be made refuses it as
    /// unavailable, with no grace; either is recorded before the tool error
    /// is returned. The source policy has already ruled by the time this
    /// runs and rules again on whatever is fetched: a registration admits
    /// nothing the policy refuses.
    fn instance_permits(&mut self, tool: &str, url: Option<&str>) -> Result<(), String> {
        let home = self
            .policy
            .source()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let stopped = match crate::instance::Retained::for_session(&home, self.session.session_id())
        {
            Ok(None) => return Ok(()),
            Ok(Some(mut retained)) => match retained.check(&home, &self.identity, Utc::now()) {
                Ok(()) => return Ok(()),
                Err(stopped) => stopped,
            },
            // A registration this runtime cannot read is not an unregistered
            // session: its authority cannot be established.
            Err(reason) => crate::instance::Stopped {
                outcome: "unavailable",
                reason: format!("instance_record_unreadable: {reason}"),
            },
        };
        let _ = self
            .session
            .record_instance_refusal(&self.host, tool, url, &stopped);
        Err(format!(
            "{} before the crossing: this session's registered instance does not hold \
             authority for it ({}). Nothing was fetched. `commonmeasure instance status` \
             shows the binding; a new registration or a renewal is an operator action.",
            stopped.outcome, stopped.reason
        ))
    }

    /// Record the crossing. Legacy tool calls continue after a failed write;
    /// explicit host-observation fetches require a durable acquisition handle
    /// before returning content. Either failure leaves the log owing a gap,
    /// which `commonmeasure_runtime::EvidenceLog` materialises on the next write.
    fn record(&mut self, facts: FetchFacts) {
        let crossing = Crossing {
            session_id: self.session.session_id().to_owned(),
            timestamp: Utc::now(),
            mode: CrossingMode::Mediated,
            host: self.host.clone(),
            client: self.client.clone(),
            tool: None,
            agent_type: None,
            agent_id: None,
            turn_id: None,
            cwd: self.cwd.clone(),
            host_name: grounding::host_of(&facts.url),
            internal: grounding::matches_internal_prefix(
                &facts.url,
                self.policy.internal_prefixes(),
            ),
            url: facts.url,
            content_hash: facts.content_hash,
            retrieved_hash: facts.retrieved_hash,
            estimated_tokens: facts.estimated_tokens,
            grounded: facts.grounded,
            licence: facts.licence,
            refusal: facts.refusal,
            policy_scope: self.policy.scope().map(str::to_owned),
            principal: Some(self.policy.principal().to_owned()),
            authentication_basis: Some(self.policy.authentication_basis().as_str().to_owned()),
            breach: facts.breach,
            derived_from: None,
            http_status: facts.http_status,
            failure: facts.failure,
            declarations: facts.declarations,
            named_by: facts.named_by,
            manifest_record: facts.manifest_record,
            content_telemetry_id: facts.content_telemetry_id,
            allowance: facts.allowance,
            identity: facts.identity,
            challenge: facts.challenge,
            supplier: facts.supplier,
        };
        if let Err(error) = self.session.record_crossing(&crossing) {
            self.evidence_error = Some(error.to_string());
        }
    }

    /// Who named the URL a fetch was asked for, from the prompts the hook
    /// recorded for this session. Read at each fetch, because the prompt
    /// hook and this server are different processes sharing one log.
    fn named_by(&mut self, url: &str) -> crate::prompt::NamedBy {
        let seen = self.session.prompt_hashes_since(&mut self.prompts);
        crate::prompt::named_by(url, seen.then_some(self.prompts.hashes.as_slice()))
    }
}

/// What one mediated crossing established, gathered as the fetch proceeds
/// and written once. The constructors name the two shapes every path ends
/// in: a refusal, and a crossing that was carried.
struct FetchFacts {
    url: String,
    content_hash: Option<String>,
    retrieved_hash: Option<String>,
    estimated_tokens: Option<u64>,
    grounded: bool,
    licence: LicenceState,
    refusal: Option<String>,
    breach: Option<String>,
    http_status: Option<u16>,
    failure: Option<String>,
    declarations: Option<Declarations>,
    named_by: Option<crate::prompt::NamedBy>,
    manifest_record: Option<u64>,
    content_telemetry_id: Option<uuid::Uuid>,
    allowance: Option<Value>,
    identity: Option<PresentedIdentity>,
    challenge: Option<String>,
    supplier: Option<String>,
}

impl FetchFacts {
    fn carried(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            content_hash: None,
            retrieved_hash: None,
            estimated_tokens: None,
            grounded: false,
            licence: LicenceState::Unknown,
            refusal: None,
            breach: None,
            http_status: None,
            failure: None,
            declarations: None,
            named_by: None,
            manifest_record: None,
            content_telemetry_id: None,
            allowance: None,
            identity: None,
            challenge: None,
            supplier: None,
        }
    }

    fn refused(url: &str, reason: String) -> Self {
        Self {
            refusal: Some(reason),
            ..Self::carried(url)
        }
    }

    /// A search result as the supplier delivered it: the licence and hash
    /// are the supplier's, and nothing about a source's declarations was
    /// read, because no page was fetched. The supplier is named on the
    /// record so the relay can say who served the result.
    fn delivered(
        url: &str,
        supplier: &str,
        content_hash: Option<String>,
        estimated_tokens: Option<u64>,
        refusal: Option<String>,
        breach: Option<String>,
        licence: LicenceState,
    ) -> Self {
        Self {
            content_hash,
            estimated_tokens,
            refusal,
            breach,
            licence,
            supplier: Some(supplier.to_owned()),
            ..Self::carried(url)
        }
    }
}

/// The receiver named in `<home>/relay.json`, read as the relay reads it:
/// the `receiver` field, or none when the file is absent or does not name
/// one. The relay refuses a malformed file before projecting; here a
/// malformed file is no receiver, which is the conservative reading for a
/// demand that needs one.
fn configured_receiver(home: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(home.join("relay.json")).ok()?;
    let config: Value = serde_json::from_slice(&bytes).ok()?;
    config["receiver"].as_str().map(str::to_owned)
}

/// Whether the origin refused the fetcher rather than the resource, and what
/// it answered with.
///
/// A 404 says the page is not there; a challenge says this request was not
/// welcome from whoever made it. Both are non-2xx and both leave a crossing
/// with no hash, so without this they read as one outcome — and they are not:
/// the second is the answer to "does the identity work here?", which is what
/// an operator deciding whether to enrol, or a publisher deciding whether to
/// admit the bot, is actually asking.
///
/// `cf-mitigated` is Cloudflare's own marker: it says the origin turned the
/// request away for being an unverified bot, so that answer alone is read as a
/// refusal of this runtime's identity. A bare 401 or 403 says only that the
/// request was refused rather than the resource; it may equally be a paywall,
/// a geographic block or a missing permission, and nothing here can tell
/// which, so the record says no more than that. A rate limit, a redirect loop
/// and a server fault are neither, because they answer about load or the
/// origin rather than about the request.
fn challenge_of(response: &Response) -> Option<String> {
    if let Some(mitigation) = response.headers.get("cf-mitigated") {
        return Some(format!(
            "the origin answered {} with cf-mitigated: {mitigation}, which refuses this \
             runtime's identity. A fetcher that cannot run JavaScript cannot pass a challenge, \
             so this page is reachable only where the site admits the signed identity.",
            response.status
        ));
    }
    matches!(response.status, 401 | 403).then(|| {
        format!(
            "the origin answered {}, which refuses the request rather than the page. Why it \
             refused is not stated: a paywall, a block and an unwelcome fetcher all answer this \
             way.",
            response.status
        )
    })
}

/// A breach under the session's mode: refused in strict, carried with the
/// breach recorded in observe and prefer. The one place the declaration
/// rulings take their mode from.
fn breach(mode: PolicyMode, reason: String, gap: &str) -> Ruling {
    let gap = Gap::new(GapReason::PolicyRefused, gap);
    match mode {
        PolicyMode::Strict => Ruling::Refused { reason, gap },
        PolicyMode::Prefer | PolicyMode::Observe => Ruling::AllowedWithBreach { reason, gap },
    }
}

/// A ruling that refuses the crossing in every policy mode: a `robots.txt`
/// `Disallow` for this fetcher (owner decision, 14 September 2026), and a
/// licence's reporting demand this session cannot meet (owner decision, 22
/// September 2026). The operator's mode is not
/// the source's consent, and RSL 1.0 §3.12 says an activity whose applicable
/// demands are not satisfied is unlicensed, so taking the content while
/// declining the duty is the case the standard names. The operator's choice
/// is whether to report, which the `relay/manual` marker expresses, not
/// whether to decline reporting and still read.
fn binding_breach(reason: String, gap: &str) -> Ruling {
    Ruling::Refused {
        reason,
        gap: Gap::new(GapReason::PolicyRefused, gap),
    }
}

fn first_refusal(rulings: &[Ruling]) -> Option<&String> {
    rulings.iter().find_map(|ruling| match ruling {
        Ruling::Refused { reason, .. } => Some(reason),
        _ => None,
    })
}

/// Every carried breach among the rulings, as one text.
fn breaches_of(rulings: &[Ruling]) -> Option<String> {
    let reasons: Vec<&str> = rulings
        .iter()
        .filter_map(|ruling| match ruling {
            Ruling::AllowedWithBreach { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    (!reasons.is_empty()).then(|| reasons.join(" "))
}

/// The price the licence read before the request quotes for AI input, where
/// operator terms do not govern instead and the licence's amount is one this
/// runtime can represent exactly. A paid licence quoting no amount is a
/// price unknown, which the allowance gate never reserves as zero; it is
/// ruled on as an unmet payment term instead.
fn quoted_price(declarations: &Declarations) -> Option<commonmeasure_types::Money> {
    if declarations.governing == Governing::OperatorTerms {
        return None;
    }
    declarations
        .licence_terms()
        .and_then(|(_, terms)| terms.payment.as_ref())
        .filter(|payment| payment.is_monetary())
        .and_then(|payment| payment.price())
}

/// The licence a crossing records: the operator's terms reference where terms
/// govern the host, else the URL of the RSL licence whose terms were read,
/// else unknown.
fn licence_of(declarations: &Declarations) -> LicenceState {
    if declarations.governing == Governing::OperatorTerms
        && let Some(terms) = &declarations.terms
    {
        return LicenceState::Declared {
            reference: terms.reference.clone(),
        };
    }
    match declarations.licence_terms() {
        Some((licence, _)) => LicenceState::Declared {
            reference: licence.url.clone(),
        },
        None => LicenceState::Unknown,
    }
}

/// What the agent is told about the source's declarations: the effective
/// preference per category, each statement with its source, and what
/// governed. The full record, with cache and status detail, is on the
/// crossing.
fn declarations_summary(declarations: &Declarations) -> Value {
    json!({
        "effective": declarations.effective,
        "statements": declarations.statements,
        "governing": declarations.governing,
        "assessment_decision": declarations.assessment_decision,
        "terms": declarations.terms.as_ref().map(|terms| &terms.reference),
        "robots_group": declarations.robots.reading.as_ref().and_then(|r| r.group.clone()),
        "robots": robots_summary(&declarations.robots),
        "redirects": declarations.redirects.iter().map(robots_summary).collect::<Vec<_>>(),
        "licences": declarations.licences.iter().map(|licence| json!({
            "url": licence.url,
            "mechanism": licence.mechanism,
            "payment": licence.terms.as_ref().and_then(|t| t.payment.clone()),
            "reporting": licence.terms.as_ref().map(|t| t.reporting.clone()),
            "unavailable": licence.unavailable,
            "status": licence.status,
            "missing": licence.missing,
        })).collect::<Vec<_>>(),
    })
}

/// The `robots.txt` attribution for one requested URL, as the agent reads
/// it: the URL, the file that governs it, the group and rule matched with a
/// wildcard named as one, and what the mode did.
fn robots_summary(outcome: &discovery::RobotsOutcome) -> Value {
    let reading = outcome.reading.as_ref();
    json!({
        "requested_url": outcome.requested_url,
        "robots_url": outcome.url,
        "final_url": outcome.final_url,
        "declined_redirect": outcome.declined_redirect,
        "status": outcome.status,
        "group": reading.and_then(|r| r.group.clone()),
        "group_is_wildcard": reading.map(|r| r.group_is_wildcard),
        "access_rule": reading.and_then(|r| r.access_rule.clone()),
        "access_rule_wildcard": reading.and_then(|r| r.access_rule_wildcard),
        "crawlable": reading.and_then(|r| r.crawlable),
        "crawl_delay": reading.and_then(|r| r.crawl_delay.clone()),
        "delay": outcome.delay,
        "unavailable": outcome.unavailable,
        "unreachable": outcome.unreachable,
        "cut_short": outcome.cut_short,
        "held_copy": outcome.held_copy,
        "truncated": outcome.truncated,
        "mode": outcome.mode,
        "outcome": outcome.outcome,
        "explanation": outcome.attribution(),
    })
}

fn names_any(standing: &commonmeasure_supply::credentials::ReleasedStanding) -> bool {
    !standing.in_use.is_empty() || !standing.shadowed.is_empty() || !standing.expired.is_empty()
}

/// The `credentials` member of `credentials_loaded` and of `context_status`
/// (`docs/contracts/session-evidence.md` §Credential provenance): the
/// operator file's contribution and, where this process holds hub-released
/// credentials, the ones in use under `hub`, the ones the environment shadows
/// under `hub_shadowed`, and the ones past the store's maximum age and no
/// longer used under `hub_expired`. `shadowed` stays the file's list of
/// variable names.
fn credentials_payload(
    credentials: &commonmeasure_supply::credentials::CredentialsStatus,
    standing: Option<&commonmeasure_supply::credentials::ReleasedStanding>,
) -> Value {
    let mut payload = credentials.to_value();
    let Some(standing) = standing.filter(|standing| names_any(standing)) else {
        return payload;
    };
    payload["hub"] = json!(standing.in_use);
    if !standing.shadowed.is_empty() {
        payload["hub_shadowed"] = standing
            .shadowed
            .iter()
            .map(|name| {
                json!({
                    "connection_id": name.connection_id,
                    "provider": name.provider,
                    "variable": name.variable,
                })
            })
            .collect();
    }
    if !standing.expired.is_empty() {
        payload["hub_expired"] = standing
            .expired
            .iter()
            .map(|name| {
                json!({
                    "connection_id": name.connection_id,
                    "provider": name.provider,
                    "variable": name.variable,
                    "fetched_at": name.fetched_at,
                    "max_age_seconds": standing.max_age.as_secs(),
                })
            })
            .collect();
    }
    payload
}

/// The unavailable message a missing credential earns. It names the exact
/// file to create and the fallback the operator already has, because "not
/// configured" alone leaves a new user grepping for where configuration
/// lives.
fn unavailable_credential(provider: &str, variable: &str, path: &std::path::Path) -> String {
    format!(
        "unavailable: {provider} needs {variable}, which is not configured. Set it in {} \
         (KEY=VALUE lines, chmod 600), or export it in the environment that launches the \
         harness — the environment wins where both name it. No search was attempted.",
        path.display()
    )
}

/// Where a source stands for the PII detector's strict rule, from the
/// classification policy already makes and nothing new: a named internal
/// prefix, a loopback or private address (which the mediated path reaches
/// only under `allow_private_hosts`) and the operator's own corpus are
/// internal; everything else is public.
fn source_class(policy: &SessionPolicy, url: &str, provider: Option<&str>) -> SourceClass {
    if provider == Some(INTERNAL_PROVIDER)
        || grounding::matches_internal_prefix(url, policy.internal_prefixes())
        || !grounding::recordable(url)
    {
        SourceClass::Internal
    } else {
        SourceClass::Public
    }
}

/// Two breaches on one crossing stay two sentences on one record.
fn merge_breaches(host: Option<String>, pii: Option<&str>) -> Option<String> {
    match (host, pii) {
        (Some(host), Some(pii)) => Some(format!("{host} {pii}")),
        (Some(host), None) => Some(host),
        (None, Some(pii)) => Some(pii.to_owned()),
        (None, None) => None,
    }
}

/// The reason a crossing to the enrolled hub's own origin is refused with,
/// the same text in the tool result and in the record's `refusal`. The URL
/// is on the record beside it, so the text names no host and does not vary.
const HUB_ORIGIN_REFUSAL: &str = "This address is at the origin of the hub this edge is enrolled \
     with. A mediated crossing is signed with the enrolled key, which the hub's own routes \
     accept, so none is made to it. No policy setting lifts this.";

/// The authorities at which the enrolled hub takes this edge's signature: the
/// hub URL `connect` was given, the origin the enrolment's `Signature-Agent`
/// names, which is the public origin the hub checks a signature's
/// `@authority` against and can differ from the first behind a proxy, and a
/// managed deployment's `policy_url`. `record` is the read the identity was
/// made from. An edge with no record has no hub origin, asks for no set and
/// fetches as before.
///
/// `Err` where the edge signs (`signs`) and the set cannot be stated in full:
/// the deployment cannot be read, or a named URL has no authority. The caller
/// stops signing with that reason. Where the edge does not sign the set is
/// whatever can be read, since nothing is presented at the hub either way.
fn hub_authorities(
    home: &std::path::Path,
    record: &crate::enrolment::EnrolmentRecord,
    signs: bool,
) -> Result<Vec<HubAuthority>, String> {
    let lenient = !signs;
    let mut urls = vec![record.hub.clone(), record.identity.origin.clone()];
    match crate::managed::Deployment::read(home) {
        Ok(crate::managed::Deployment::Managed { policy_url, .. }) => urls.push(policy_url),
        Ok(crate::managed::Deployment::Local) => {}
        Err(_) if lenient => {}
        Err(reason) => {
            return Err(format!(
                "{reason}; whether it names a policy_url at the hub is not known, so nothing \
                 is signed"
            ));
        }
    }
    let mut authorities = Vec::new();
    for url in &urls {
        match HubAuthority::of(url) {
            Some(authority) => authorities.push(authority),
            None if lenient => {}
            None => {
                return Err(format!(
                    "the enrolment names {url:?}, which has no authority to keep crossings \
                     away from, so nothing is signed"
                ));
            }
        }
    }
    authorities.sort();
    authorities.dedup();
    Ok(authorities)
}

/// What the guard compares of a URL: the host, lower case and without a fully
/// qualified name's trailing dot, since `hub.example.` reaches the host
/// `hub.example` does, and the port. The scheme is not compared. It decides
/// neither where a request connects, which is the host and the port, nor
/// what its signature names: RFC 9421 `@authority` is the host, with the
/// port only where the URL's scheme would not have implied it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HubAuthority {
    host: String,
    /// The port a request for the URL connects to: the one it names, or its
    /// scheme's default.
    port: u16,
    /// Whether that port is the scheme's default, so that `@authority` is
    /// the bare host.
    port_implied: bool,
}

impl HubAuthority {
    /// `None` for a URL with no host, or with no port and a scheme that
    /// implies none; neither is one `follow` requests.
    fn of(url: &str) -> Option<Self> {
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed
            .host_str()?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        Some(Self {
            host,
            port: parsed.port_or_known_default()?,
            // `port` is `None` for the scheme's default port, named or not.
            port_implied: parsed.port().is_none(),
        })
    }

    /// Whether a request for `asked` would connect where the hub listens or
    /// carry the `@authority` the hub checks. The first catches
    /// `http://hub.example:443/` for a hub at `https://hub.example`, whose
    /// authority strings differ. The second catches `http://hub.example/`,
    /// which connects to port 80 and is signed for `hub.example` exactly as
    /// the `https` URL is, so a proxy that passes port 80 to the hub would
    /// hand it a signature that verifies.
    fn covers(&self, asked: &Self) -> bool {
        self.host == asked.host
            && (self.port == asked.port || (self.port_implied && asked.port_implied))
    }
}

/// Why a fetch produced no bytes. Every variant names the URL the record must
/// carry, which is the hop that failed rather than the one first asked for.
enum FetchFailure {
    /// Policy refused this hop before it happened.
    Refused { url: String, reason: String },
    /// Nothing usable answered.
    Failed { url: String, detail: String },
    /// This edge did not send the hop for a reason of its own: the request
    /// could not be signed, or the call's time limit left nothing for it.
    NotSent { url: String, detail: String },
}

/// Whether the addresses a hop resolved to are ones this policy may reach.
///
/// The privacy floor classifies a URL's spelling; DNS decides where the
/// connection goes. A public name whose record points at loopback or into an
/// RFC 1918 range would otherwise be mediated straight into a service on the
/// operator's own machine — and recorded under the public name it was spelled
/// with, which is also what clears the relay's egress floor. So the resolved
/// address is put to the same floor, spelled as the address it is: one
/// classifier for what private means, asked about both halves.
fn reaches_allowed_addresses(
    policy: &SessionPolicy,
    url: &str,
    addresses: &[SocketAddr],
) -> Result<(), String> {
    // Supply the operator named as internal is theirs by construction, and
    // that consent is about the name: an intranet name resolving inside the
    // intranet is where it was always going. Not where the floor is held:
    // there the name was refused before resolution, and a public name that
    // resolves inward is refused here.
    if !policy.holds_private_floor()
        && grounding::matches_internal_prefix(url, policy.internal_prefixes())
    {
        return Ok(());
    }
    for address in addresses {
        if !policy.mediates_address(&format!("http://{address}/")) {
            // The address itself stays out of the refusal, and so out of the
            // record: which service an operator runs on their own network is
            // exactly what the floor keeps to them.
            return Err(format!(
                "{} resolves to a local or private address, which Common Measure does not mediate.",
                grounding::host_of(url)
            ));
        }
    }
    Ok(())
}

/// Whether a hop's URL may be reached, judged before its name is looked up.
type UrlCheck<'a> = &'a dyn Fn(&str) -> Result<(), String>;

/// Whether a hop's URL may be reached at the addresses it resolved to.
type AddressCheck<'a> = &'a dyn Fn(&str, &[SocketAddr]) -> Result<(), String>;

/// Follow redirects to the resource that actually answered, so the recorded URL
/// is the one whose bytes were hashed rather than the one first asked for.
///
/// Two checks per hop, at the two moments they can be made: `allowed` judges
/// the URL before the name is looked up at all — a denied host must not even be
/// asked about — and `reaches` judges the addresses that lookup returned,
/// before a socket is opened. The connection is then made to exactly those
/// addresses, leaving no second lookup for a different answer to land in.
///
/// `on_hop` runs for each redirect hop once `allowed` has admitted it and
/// before its name is looked up: the caller reads what the hop's own origin
/// declares and refuses the hop where its mode says to, so a disallowed hop
/// is never requested. The first URL is the caller's to evaluate before
/// calling.
///
/// `identity` signs each hop separately rather than the fetch once. The
/// signature covers the authority it is sent to and the correlation id the hop
/// carries, and a redirect changes both: one signature reused across hops
/// would attest to the host that redirected and would be refused by the host
/// that answered. An edge with no key to sign with sends every hop unsigned.
fn follow(
    url: &str,
    request: Request,
    budget: &dyn Fn() -> Result<Duration, String>,
    identity: Option<&SigningIdentity>,
    allowed: UrlCheck<'_>,
    reaches: AddressCheck<'_>,
    on_hop: UrlCheck<'_>,
) -> Result<(String, Response), FetchFailure> {
    let mut current = url.to_owned();
    let first_host = grounding::host_of(url);
    for _ in 0..=MAX_REDIRECTS {
        let mut hop = Request::get("/");
        hop.headers = request.headers.clone();
        // The correlation id is re-attached on a same-domain hop and left off
        // a cross-domain one (section 7.2): the target domain may not be a
        // telemetry participant, and the id was minted for the first.
        if grounding::host_of(&current) != first_host {
            hop.headers.remove(CONTENT_TELEMETRY_ID);
        }
        if let Some(identity) = identity
            && let Err(reason) = identity.sign_request(
                &current,
                &mut hop,
                &[CONTENT_TELEMETRY_ID],
                Utc::now().timestamp(),
            )
        {
            // The identity is the point of the request. Sending it unsigned
            // instead would present the product token as though it were the
            // network identity, which is the claim this package exists to
            // stop being false.
            return Err(FetchFailure::NotSent {
                detail: format!("the request could not be signed: {reason}"),
                url: current,
            });
        }
        let addresses = match commonmeasure_http::resolve(&current) {
            Ok(addresses) => addresses,
            Err(error) => {
                return Err(FetchFailure::Failed {
                    detail: format!("{error:#}"),
                    url: current,
                });
            }
        };
        if let Err(reason) = reaches(&current, &addresses) {
            return Err(FetchFailure::Refused {
                url: current,
                reason,
            });
        }
        let timeout = match budget() {
            Ok(timeout) => timeout,
            Err(reason) => {
                return Err(FetchFailure::NotSent {
                    detail: format!("{current} was not requested: {reason}"),
                    url: current,
                });
            }
        };
        let response = match commonmeasure_http::send_to(&current, &addresses, hop, timeout) {
            Ok(response) => response,
            Err(error) => {
                return Err(FetchFailure::Failed {
                    detail: format!("{error:#}"),
                    url: current,
                });
            }
        };
        if !matches!(response.status, 301 | 302 | 307 | 308) {
            return Ok((current, response));
        }
        let Some(location) = response.headers.get("Location") else {
            return Err(FetchFailure::Failed {
                url: current,
                detail: "redirect without a Location header".to_owned(),
            });
        };
        let next = if location.starts_with("http://") || location.starts_with("https://") {
            location.to_owned()
        } else {
            match url::Url::parse(&current).and_then(|base| base.join(location)) {
                Ok(joined) => joined.to_string(),
                Err(error) => {
                    return Err(FetchFailure::Failed {
                        detail: error.to_string(),
                        url: current,
                    });
                }
            }
        };
        current = next;
        // A redirect can leave the address space and the hosts the operator
        // allowed, so each hop is checked again rather than trusted because
        // the first one passed.
        if let Err(reason) = allowed(&current) {
            return Err(FetchFailure::Refused {
                url: current,
                reason,
            });
        }
        if let Err(reason) = on_hop(&current) {
            return Err(FetchFailure::Refused {
                url: current,
                reason,
            });
        }
    }
    Err(FetchFailure::Failed {
        url: current,
        detail: "redirect limit exceeded".to_owned(),
    })
}

/// Fold a redirect chain into the declarations the record carries. The
/// last hop that was evaluated supplies `robots`, `statements` and
/// `licences`, since its origin is the one whose rules govern the URL the
/// record names; each earlier hop's `robots.txt` evaluation is kept in
/// `redirects` in request order, so a shortener's rule stays attributed to
/// the shortener. Returns the folded declarations, the rulings of the
/// earlier hops (breaches already carried on the way) and the last hop's
/// own rulings.
fn fold_hops(
    first: (Declarations, Vec<Ruling>),
    hops: Vec<(Declarations, Vec<Ruling>)>,
) -> (Declarations, Vec<Ruling>, Vec<Ruling>) {
    let (mut declarations, mut last) = first;
    let mut earlier = Vec::new();
    for (hop, rulings) in hops {
        let previous = std::mem::replace(&mut declarations, hop);
        declarations.redirects = previous.redirects;
        declarations.redirects.push(previous.robots);
        earlier.append(&mut last);
        last = rulings;
    }
    (declarations, earlier, last)
}

fn tool_definitions() -> Value {
    // The tool annotations of 2025-03-26 and later. The three tools that
    // read (fetch, search, status) declare `readOnlyHint`, because a host
    // that sees no hint treats the tool as a write and asks the person to
    // confirm each call; `context_enrol` changes the operator home and
    // declares nothing.
    let read_only = json!({"readOnlyHint": true});
    json!([
        {
            "name": "context_fetch",
            "description": "Fetch a URL through Common Measure. An HTML page is delivered as its readable text, not its markup; any other body is delivered as received. The crossing is checked against operator source policy before it happens and recorded either way, with the hash of exactly what entered context and the hash of the bytes the origin served. Prefer this over WebFetch when the operator wants an evidence trail.",
            "inputSchema": {
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            },
            "annotations": read_only.clone()
        },
        {
            "name": "context_search",
            "description": "Search a configured content provider (exa, firecrawl, keenable, linkup, nimble, ozone, parallel, search1api, serpdive, tavily, tinyfish, tollbit, you), or query the operator's own internal corpus (provider \"internal\", a bounded directory named by COMMONMEASURE_INTERNAL_CORPUS — deterministic retrieval with a declared licence, not a web search). Each result is checked against operator source policy and recorded. Reports unavailable, naming the missing credential or variable, when the provider is not configured.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "provider": {"type": "string", "enum": ["exa", "firecrawl", "internal", "keenable", "linkup", "nimble", "ozone", "parallel", "search1api", "serpdive", "tavily", "tinyfish", "tollbit", "you"]},
                    "limit": {"type": "integer"}
                },
                "required": ["query"]
            },
            "annotations": read_only.clone()
        },
        {
            "name": "context_status",
            "description": "The current session, where its evidence is written, the operator's source policy, and which providers are configured.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": read_only.clone()
        },
        {
            "name": "context_enrol",
            "description": "Explicitly enrol, inspect, sync or remove reporting for the actual agent project. Ask for project name and local/hub reporting; explain canonical root and descendants and historical evidence before enabling hub reporting. No new shell or filesystem permissions are granted.",
            "inputSchema": {"type": "object", "properties": {
                "directory": {"type": "string"}, "action": {"type": "string", "enum": ["status", "enrol", "remove", "sync"]},
                "name": {"type": "string"}, "reporting": {"type": "string", "enum": ["local", "hub"]}, "include_history": {"type": "boolean"}
            }, "required": ["directory"]}
        }
    ])
}

fn tool_result(id: Value, value: &Value, is_error: bool) -> Value {
    ok_response(
        id,
        json!({
            "content": [{"type": "text", "text": value.to_string()}],
            "isError": is_error,
        }),
    )
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A synthetic envelope for a bare URL, so host policy runs through exactly the
/// same code the batch runner uses rather than a second implementation.
pub(crate) fn envelope_for(url: &str) -> ContextEnvelope {
    ContextEnvelope {
        source_url: url.to_owned(),
        host: grounding::host_of(url),
        title: None,
        // Set so the shared admission check does not refuse this for carrying
        // no text: at fetch time the bytes have not been retrieved yet, and
        // whether they exist is what the fetch is about to establish.
        text: Some(String::new()),
        content_hash: None,
        licence: LicenceState::Unknown,
        declared_date: None,
        native_metadata: json!({}),
        retrieval_rank: 1,
    }
}

/// The job a standing session policy stands in for. Sessions have no
/// ContextJob, but the operator's source rules are the same rules, so they are
/// expressed in the same vocabulary and enforced by the same functions.
pub(crate) fn policy_job(policy: &SessionPolicy) -> ContextJob {
    ContextJob {
        id: uuid::Uuid::nil(),
        kind: "harness.session".to_owned(),
        prompt: "standing operator policy".to_owned(),
        policy_mode: policy.mode(),
        objective: commonmeasure_types::Objective::MaximiseQuality,
        constraints: policy.constraints().to_vec(),
        evidence_requirements: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_http::{Response, Server};
    use commonmeasure_types::canonical::sha256_digest;
    use commonmeasure_types::{AllowanceDeclaration, AllowancePeriod, Money, PolicyMode};

    /// Live origins bind inside this workspace's reserved range, leaving the
    /// top of it free for the port a test needs nothing listening on.
    fn bind_local() -> Server {
        (47700..47799)
            .find_map(|port| Server::bind(&format!("127.0.0.1:{port}")).ok())
            .expect("a free port in 47700-47798")
    }

    fn server(policy: &str) -> (tempfile::TempDir, McpServer) {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join("policy.json"), policy).expect("policy");
        let server = server_at(home.path());
        (home, server)
    }

    /// A server over a home already holding its policy and whatever else the
    /// test wrote there: the identity is read when the server is made.
    fn server_at(home: &std::path::Path) -> McpServer {
        let loaded = SessionPolicy::load(home, None).expect("the policy loads");
        let log = SessionLog::open(home, "test-session").expect("session log");
        let credentials = commonmeasure_supply::credentials::CredentialsStatus {
            path: home.join(commonmeasure_supply::credentials::CREDENTIALS_FILE),
            loaded: None,
        };
        McpServer::new(log, loaded, "claude-code", None, credentials)
    }

    fn records(home: &std::path::Path) -> Vec<Value> {
        let path = home.join("sessions/test-session.ndjson");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
            .collect()
    }

    fn crossings(home: &std::path::Path) -> Vec<Value> {
        records(home)
            .into_iter()
            .filter(|record| {
                record["event"]
                    .as_str()
                    .is_some_and(|event| event.starts_with("crossing_"))
            })
            .collect()
    }

    fn allowance_gaps(home: &std::path::Path) -> Vec<Value> {
        records(home)
            .into_iter()
            .filter(|record| record["event"] == "allowance_gap")
            .collect()
    }

    /// A 0.020000 USD day with 0.007000 USD of it reserved, as a mediated
    /// search reserves an adapter's published price before dispatch.
    fn held_reservation(home: &std::path::Path) -> (AllowanceContext, GateDecision) {
        let context = AllowanceContext::new(
            home,
            "os-user:test".to_owned(),
            vec![AllowanceDeclaration {
                period: AllowancePeriod::Day,
                amount: Money::new("USD", 20_000),
                timezone: "UTC".to_owned(),
            }],
        );
        let decision = context.pre_dispatch(
            Some(&Money::new("USD", 7_000)),
            PolicyMode::Observe,
            Utc::now(),
            "session test-session",
        );
        assert!(decision.reservation.is_some(), "{}", decision.record);
        (context, decision)
    }

    /// The ledger takes no further write: the reservation line is on disk
    /// and the file is read-only.
    fn refuse_ledger_writes(context: &AllowanceContext) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            context.ledger.file(),
            std::fs::Permissions::from_mode(0o444),
        )
        .expect("chmod");
    }

    /// A refused redirect hop is enforcement, and enforcement must be on the
    /// record. Probed live against the real binary: the refusal was correctly
    /// returned to the agent and the session log was never created, while the
    /// identical refusal on a directly named URL was recorded.
    #[test]
    fn a_refused_redirect_hop_records_the_refusal_it_enforced() {
        // `robots.txt` answers 404, no rules: a `robots.txt` redirected to the
        // denied host would itself be unreachable and refuse the asked-for
        // URL before its redirect was reached.
        let mut origin = bind_local()
            .spawn(|request| {
                if request.target == "/robots.txt" {
                    return Response::text(404, "none");
                }
                let mut response = Response::new(302, Vec::new());
                response
                    .headers
                    .set("Location", "https://denied.example/handbook");
                response
            })
            .expect("spawn");
        let (home, mut server) = server(
            r#"{"policy_mode":"strict","allow_private_hosts":true,
                "constraints":[{"kind":"denied_source_host","host":"denied.example"}]}"#,
        );

        let error = server
            .tool_fetch(&json!({"url": format!("{}/doc", origin.url())}))
            .expect_err("a redirect to a denied host is refused");
        assert!(error.contains("denied.example"), "{error}");

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1, "the enforcement is on the record");
        assert_eq!(recorded[0]["event"], "crossing_refused");
        assert_eq!(
            recorded[0]["payload"]["url"], "https://denied.example/handbook",
            "the record names the hop that was refused, not the one asked for"
        );
        origin.stop();
    }

    /// Enrol `home` with `hub` through the runtime's own types, as `connect`
    /// leaves it: a stored key and the enrolment record that names it.
    fn enrol(home: &std::path::Path, hub: &str, origin: &str) {
        use crate::enrolment::{EnrolledIdentity, EnrolledOrganization, EnrolmentRecord};
        let key = crate::identity::EdgeKey::generate().expect("a key");
        key.store(home).expect("store the key");
        EnrolmentRecord {
            hub: hub.to_owned(),
            organization: EnrolledOrganization {
                id: "org-1".to_owned(),
                name: "Org A Media".to_owned(),
            },
            name: "laptop-7".to_owned(),
            key_id: key.thumbprint(),
            identity: EnrolledIdentity {
                origin: origin.to_owned(),
                bot_page: format!("{origin}/bot"),
                contact: None,
            },
            enrolled_at: "2026-09-06T00:00:00.000Z".to_owned(),
            revoked_at: None,
            revocation: None,
            revocation_learnt_at: None,
        }
        .store(home)
        .expect("store the enrolment record");
    }

    /// An origin that counts the requests it receives and answers each `200`.
    fn counting_origin() -> (
        commonmeasure_http::ServerHandle,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&calls);
        let origin = bind_local()
            .spawn(move |request| {
                seen.lock().expect("lock").push(request.target.clone());
                Response::text(200, "an answer from the hub's origin")
            })
            .expect("spawn");
        (origin, calls)
    }

    /// The guard compares the host and the port, never the scheme: a URL is
    /// the hub's where a request for it would connect to the hub's listener
    /// or be signed for the hub's `@authority`.
    #[test]
    fn a_hub_authority_is_compared_by_host_and_port() {
        let cases: [(&str, &[&str], &[&str]); 3] = [
            (
                "https://hub.example",
                &[
                    "https://HUB.example/api/v1/supplier-credentials",
                    "https://hub.example:443/",
                    "https://hub.example./",
                    "https://agent@hub.example/",
                    // The hub's listener, asked for in clear.
                    "http://hub.example:443/",
                    // Port 80, signed for `hub.example` as the hub's own URL is.
                    "http://hub.example/",
                ],
                &[
                    "https://hub.example:8443/",
                    "https://hub.example:80/",
                    "http://hub.example:8080/",
                    "https://hub.example.evil.test/",
                ],
            ),
            (
                "http://hub.example",
                &[
                    "http://hub.example:80/",
                    "https://hub.example:80/",
                    "https://hub.example/",
                ],
                &["http://hub.example:443/", "https://hub.example:8443/"],
            ),
            (
                "http://127.0.0.1:3000",
                &["http://127.0.0.1:3000/api", "https://127.0.0.1:3000/"],
                &["http://127.0.0.1:3001/", "http://127.0.0.1/"],
            ),
        ];
        for (hub_url, same, other) in cases {
            let hub = HubAuthority::of(hub_url).expect("an authority");
            for url in same {
                let asked = HubAuthority::of(url).expect("an authority");
                assert!(hub.covers(&asked), "{url} is at {hub_url}");
            }
            for url in other {
                let asked = HubAuthority::of(url).expect("an authority");
                assert!(!hub.covers(&asked), "{url} is not at {hub_url}");
            }
        }
        assert_eq!(HubAuthority::of("mailto:owner@hub.example"), None);
        assert_eq!(HubAuthority::of("hub.example"), None);
    }

    /// The enrolment names the hub twice, the URL `connect` was given and
    /// the origin `Signature-Agent` names, and a managed deployment names a
    /// `policy_url`. Each is an authority the hub takes this edge's signature
    /// at. No network: the three URLs are refused before any lookup.
    #[test]
    fn every_authority_the_enrolment_names_is_refused_and_recorded() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .expect("policy");
        enrol(
            home.path(),
            "https://hub.example",
            "https://agents.hub.example",
        );
        std::fs::write(
            home.path().join("deployment.json"),
            format!(
                r#"{{"mode":"managed","organisation":"org-1","policy_url":"https://policy.hub.example/api/v1/policy","signer":{{"key_id":"k","algorithm":"ed25519","public_key":"{}"}}}}"#,
                "00".repeat(32)
            ),
        )
        .expect("deployment");
        let mut server = server_at(home.path());

        let urls = [
            "https://hub.example/api/v1/supplier-credentials",
            "https://agents.hub.example/.well-known/http-message-signatures-directory",
            "https://policy.hub.example/api/v1/policy",
        ];
        for url in urls {
            let error = server
                .tool_fetch(&json!({"url": url}))
                .expect_err("the hub's origin is refused");
            assert!(error.contains(HUB_ORIGIN_REFUSAL), "{error}");
        }
        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), urls.len());
        for (crossing, url) in recorded.iter().zip(urls) {
            assert_eq!(crossing["event"], "crossing_refused");
            assert_eq!(crossing["payload"]["url"], url);
            assert_eq!(crossing["payload"]["refusal"], HUB_ORIGIN_REFUSAL);
        }
    }

    /// An enrolled edge whose `deployment.json` cannot be read does not know
    /// whether a `policy_url` names another authority at the hub, so it stops
    /// signing and every record says why.
    #[test]
    fn an_edge_that_cannot_state_the_hubs_origins_does_not_sign() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .expect("policy");
        enrol(
            home.path(),
            "https://hub.example",
            "https://agents.hub.example",
        );
        std::fs::write(home.path().join("deployment.json"), r#"{"mode":"manag"#)
            .expect("deployment");
        let server = server_at(home.path());
        let unsigned = server
            .identity
            .presented()
            .unsigned
            .expect("the edge does not sign");
        assert!(unsigned.contains("deployment"), "{unsigned}");
        assert!(unsigned.contains("nothing is signed"), "{unsigned}");
    }

    /// The guarded set comes from the record the identity was made from and
    /// not from a second read: given the record of one hub while the file
    /// names another, it states the first. With one read there is no record
    /// that can name another key than the one loaded, so that refusal is gone.
    #[test]
    fn the_guarded_set_is_the_given_records_and_not_the_files() {
        let home = tempfile::tempdir().expect("tempdir");
        enrol(
            home.path(),
            "https://hub.example",
            "https://agents.hub.example",
        );
        let read = crate::enrolment::EnrolmentRecord::load(home.path())
            .expect("the record reads")
            .expect("a record");
        enrol(
            home.path(),
            "https://other-hub.example",
            "https://agents.other-hub.example",
        );

        let mut expected = vec![
            HubAuthority::of("https://hub.example").expect("an authority"),
            HubAuthority::of("https://agents.hub.example").expect("an authority"),
        ];
        expected.sort();
        assert_eq!(hub_authorities(home.path(), &read, true), Ok(expected));
    }

    /// An enrolment whose hub URL has no authority leaves an origin the guard
    /// cannot keep crossings away from, so a signing edge stops signing. An
    /// edge that does not sign guards the authorities it can read.
    #[test]
    fn an_enrolment_naming_a_url_with_no_authority_does_not_sign() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .expect("policy");
        enrol(
            home.path(),
            "mailto:owner@hub.example",
            "https://agents.hub.example",
        );

        let server = server_at(home.path());
        let unsigned = server
            .identity
            .presented()
            .unsigned
            .expect("the edge does not sign");
        assert!(unsigned.contains("mailto:owner@hub.example"), "{unsigned}");
        assert!(unsigned.contains("no authority"), "{unsigned}");
        assert!(unsigned.contains("nothing is signed"), "{unsigned}");

        let record = crate::enrolment::EnrolmentRecord::load(home.path())
            .expect("the record reads")
            .expect("a record");
        assert_eq!(
            hub_authorities(home.path(), &record, false),
            Ok(vec![
                HubAuthority::of("https://agents.hub.example").expect("an authority")
            ]),
        );
    }

    /// The declaration probes are signed requests to URLs a publisher
    /// chooses: a `robots.txt` that redirects is followed. The counting
    /// origin stands where the hub would and shows only whether a request
    /// was sent to it; the page itself is still fetched.
    #[test]
    fn a_declaration_probe_redirected_to_the_hub_is_not_sent() {
        let (mut hub, calls) = counting_origin();
        let hub_url = hub.url();
        let pages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&pages);
        let mut publisher = bind_local()
            .spawn(move |request| {
                seen.lock().expect("lock").push(request.target.clone());
                if request.target == "/robots.txt" {
                    let mut response = Response::new(302, Vec::new());
                    response.headers.set(
                        "Location",
                        &format!("{hub_url}/api/v1/supplier-credentials"),
                    );
                    response
                } else {
                    Response::text(200, "the handbook page")
                }
            })
            .expect("spawn");
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
        )
        .expect("policy");
        enrol(home.path(), &hub.url(), "https://agents.hub.example");
        let mut server = server_at(home.path());

        // The redirect is not followed, so the file is unreachable and, with
        // no copy held, a complete disallow (RFC 9309 §2.3.1.2, §2.3.1.4).
        let error = server
            .tool_fetch(&json!({"url": format!("{}/doc", publisher.url())}))
            .expect_err("a robots.txt that redirects to the hub is unreachable");
        for needed in [
            "redirected to",
            HUB_ORIGIN_REFUSAL,
            "An unreachable robots.txt is a complete disallow",
        ] {
            assert!(error.contains(needed), "missing {needed:?} in {error}");
        }
        assert!(
            calls.lock().expect("lock").is_empty(),
            "a probe reached the hub's origin: {:?}",
            calls.lock().expect("lock")
        );
        assert_eq!(
            *pages.lock().expect("lock"),
            ["/robots.txt"],
            "the page was not requested"
        );
        publisher.stop();
        hub.stop();
    }

    /// A redirect hop's `robots.txt`, probed with less than a whole probe's
    /// budget left, from a host that answers after the call's time limit but
    /// within `PROBE_TIMEOUT`, is cut short by this edge rather than
    /// unreachable: the hop is refused naming the time limit, nothing is
    /// kept against the host, and the next crossing, with its whole budget,
    /// reads the file and fetches the page. The first crossing's ceiling is
    /// shortened to 1 s on this thread (`crawl_delay::TEST_CEILING`); the
    /// probe, the redirect and the origins are the production path.
    #[test]
    fn a_probe_cut_short_by_the_call_time_limit_is_not_kept_as_the_hosts_failure() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let mut slow = bind_local()
            .spawn(move |request| {
                log.lock().expect("lock").push(request.target.clone());
                if request.target == "/robots.txt" {
                    std::thread::sleep(Duration::from_millis(1500));
                    Response::text(200, "User-agent: *\nAllow: /\n")
                } else {
                    Response::text(200, "the destination page")
                }
            })
            .expect("spawn");
        // `localhost` keeps the slow host's cache record apart from the
        // redirecting origin's, which is `127.0.0.1`.
        let destination = format!("http://localhost:{}/page", slow.addr().port());
        let location = destination.clone();
        let mut site = bind_local()
            .spawn(move |request| {
                if request.target == "/robots.txt" {
                    Response::text(200, "User-agent: *\nAllow: /\n")
                } else {
                    let mut response = Response::new(302, Vec::new());
                    response.headers.set("Location", &location);
                    response
                }
            })
            .expect("spawn");
        let (home, mut server) = server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        let start = format!("{}/start", site.url());

        crate::crawl_delay::TEST_CEILING.set(Some(Duration::from_secs(1)));
        let error = server
            .tool_fetch(&json!({"url": start}))
            .expect_err("the hop's robots.txt did not answer within the call");
        crate::crawl_delay::TEST_CEILING.set(None);
        for needed in [
            "was not read for",
            "this call reached its time limit",
            "refused before the request",
        ] {
            assert!(error.contains(needed), "missing {needed:?} in {error}");
        }
        assert!(!error.contains("could not be reached"), "{error}");
        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["event"], "crossing_refused");
        assert_eq!(recorded[0]["payload"]["url"], destination);
        let robots = &recorded[0]["payload"]["declarations"]["robots"];
        assert_eq!(robots["outcome"], "cut_short", "{robots}");
        assert_eq!(robots["cut_short"], true, "{robots}");
        assert!(robots["unreachable"].is_null(), "{robots}");
        let kept = discovery::DeclarationCache::open(home.path())
            .load("localhost")
            .robots
            .expect("the probe is recorded");
        assert!(
            kept.cut_short && kept.expires_at == kept.fetched_at,
            "{kept:?}"
        );
        assert_eq!(*seen.lock().expect("lock"), ["/robots.txt"]);

        server
            .tool_fetch(&json!({"url": start}))
            .expect("the whole budget reads the file and fetches the page");
        // The Content Telemetry manifest is asked for after the page.
        assert_eq!(
            seen.lock().expect("lock")[..3],
            ["/robots.txt", "/robots.txt", "/page"]
        );
        let recorded = crossings(home.path());
        assert_eq!(recorded[1]["event"], "crossing_mediated");
        assert_eq!(
            recorded[1]["payload"]["declarations"]["robots"]["outcome"],
            "allowed"
        );
        slow.stop();
        site.stop();
    }

    /// A hosted endpoint's host word sends no session-end event, so a
    /// reporting demand is met there only where the service relays the home
    /// on its interval; the transport says so with `interval_relay`.
    #[test]
    fn a_hosted_session_meets_a_reporting_demand_only_under_the_interval_relay() {
        let home = tempfile::tempdir().expect("tempdir");
        let work = home.path().join("reporting-cleared");
        std::fs::create_dir_all(&work).expect("workspace");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"strict","scopes":[{"match":"reporting-cleared",
                "engagement":"research","allow_telemetry_egress":true}]}"#,
        )
        .expect("policy");
        std::fs::write(
            home.path().join("relay.json"),
            r#"{"receiver":"https://receiver.example/v1/events"}"#,
        )
        .expect("relay.json");
        let open = |interval_relay: bool| {
            let loaded = SessionPolicy::load(home.path(), work.to_str()).expect("the policy loads");
            let log = SessionLog::open(home.path(), "test-session").expect("session log");
            let credentials = commonmeasure_supply::credentials::CredentialsStatus {
                path: home
                    .path()
                    .join(commonmeasure_supply::credentials::CREDENTIALS_FILE),
                loaded: None,
            };
            let server = McpServer::new(log, loaded, "claude-connector", None, credentials);
            if interval_relay {
                server.interval_relay()
            } else {
                server
            }
        };

        let relayed = open(true).reporting_ruling_for(None, None);
        assert!(relayed.met, "{:?}", relayed.reason);
        let unrelayed = open(false).reporting_ruling_for(None, None);
        assert!(!unrelayed.met);
        let reason = unrelayed.reason.expect("a reason");
        assert!(
            reason.contains("this host (claude-connector) sends no session-end event"),
            "{reason}"
        );
    }

    /// A refused redirect hop keeps the breaches carried on the hops before
    /// it. Under observe the publisher's `Content-Signal: ai-input=no` is
    /// carried as a breach; the redirect to the hub's origin is then
    /// refused, and the refusal still names the `Content-Signal`.
    ///
    /// Until WP-29 this used a `Disallow`, which observe carried. A
    /// `Disallow` now binds in every mode, so the same publisher is refused
    /// on its own `robots.txt` before the page is asked for and the redirect
    /// is never seen.
    #[test]
    fn a_refused_redirect_hop_keeps_the_breaches_carried_before_it() {
        for (robots, carried) in [
            ("User-agent: *\nDisallow: /\n", false),
            (
                "User-agent: *\nAllow: /\nContent-Signal: ai-input=no\n",
                true,
            ),
        ] {
            let (mut hub, calls) = counting_origin();
            let hub_url = hub.url();
            let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = std::sync::Arc::clone(&asked);
            let mut publisher = bind_local()
                .spawn(move |request| {
                    log.lock().expect("lock").push(request.target.clone());
                    if request.target == "/robots.txt" {
                        Response::text(200, robots)
                    } else {
                        let mut response = Response::new(302, Vec::new());
                        response
                            .headers
                            .set("Location", &format!("{hub_url}/api/v1/policy"));
                        response
                    }
                })
                .expect("spawn");
            let home = tempfile::tempdir().expect("tempdir");
            std::fs::write(
                home.path().join("policy.json"),
                r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
            )
            .expect("policy");
            enrol(home.path(), &hub.url(), "https://agents.hub.example");
            let mut server = server_at(home.path());

            let error = server
                .tool_fetch(&json!({"url": format!("{}/doc", publisher.url())}))
                .expect_err("refused");
            assert!(calls.lock().expect("lock").is_empty());
            let recorded = crossings(home.path());
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0]["event"], "crossing_refused");
            let payload = &recorded[0]["payload"];
            if carried {
                assert!(error.contains(HUB_ORIGIN_REFUSAL), "{error}");
                assert_eq!(*asked.lock().expect("lock"), ["/robots.txt", "/doc"]);
                let breach = payload["breach"]
                    .as_str()
                    .unwrap_or_else(|| panic!("the Content-Signal stays a breach: {payload}"));
                assert!(breach.contains("Content-Signal"), "{breach}");
            } else {
                assert!(error.contains("`Disallow: /`"), "{error}");
                assert!(error.contains("binds in every policy mode"), "{error}");
                assert_eq!(
                    *asked.lock().expect("lock"),
                    ["/robots.txt"],
                    "the disallowed page was never requested"
                );
                assert_eq!(payload["declarations"]["robots"]["mode"], "observe");
                assert_eq!(payload["declarations"]["robots"]["outcome"], "refused");
                assert!(payload["breach"].is_null(), "{payload}");
            }
            publisher.stop();
            hub.stop();
        }
    }

    /// A publisher can name the hub with no redirect: a `License:` line in
    /// `robots.txt`, read before the page, and a `Link` header on the page,
    /// read after it. Each is the first hop of a signed probe, which only the
    /// guard at the top of `probe` rules on; the hop closure sees later hops.
    /// The counting origin stands where the hub would and shows only whether
    /// a request was sent to it. A licence that is not read has unknown terms,
    /// so the page is not admitted under it in any mode: the `robots.txt`
    /// licence refuses the crossing before the page is asked for, and the
    /// `Link` licence withholds the bytes.
    #[test]
    fn a_licence_a_publisher_names_at_the_hub_is_not_probed() {
        let (mut hub, calls) = counting_origin();
        let publisher_for = |in_robots: bool| {
            let hub_url = hub.url();
            bind_local()
                .spawn(move |request| {
                    if request.target == "/robots.txt" {
                        let licence = if in_robots {
                            format!("License: {hub_url}/api/v1/supplier-credentials\n")
                        } else {
                            String::new()
                        };
                        Response::text(200, &format!("User-agent: *\nAllow: /\n{licence}"))
                    } else {
                        let mut response = Response::text(200, "the handbook page");
                        response.headers.set(
                            "Link",
                            &format!(
                                "<{hub_url}/api/v1/policy>; rel=\"license\"; type=\"application/rsl+xml\""
                            ),
                        );
                        response
                    }
                })
                .expect("spawn")
        };
        for in_robots in [true, false] {
            let mut publisher = publisher_for(in_robots);
            let home = tempfile::tempdir().expect("tempdir");
            std::fs::write(
                home.path().join("policy.json"),
                r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
            )
            .expect("policy");
            enrol(home.path(), &hub.url(), "https://agents.hub.example");
            let mut server = server_at(home.path());

            let refused = server
                .tool_fetch(&json!({"url": format!("{}/doc", publisher.url())}))
                .expect_err("a licence that is not read admits nothing, in observe too");
            assert!(refused.contains("could not be read"), "{refused}");
            assert!(
                refused.starts_with(if in_robots {
                    "refused before the crossing"
                } else {
                    "withheld from context"
                }),
                "{refused}"
            );
            assert!(
                calls.lock().expect("lock").is_empty(),
                "a probe reached the hub's origin: {:?}",
                calls.lock().expect("lock")
            );
            let recorded = crossings(home.path());
            assert_eq!(recorded.len(), 1);
            let licences = recorded[0]["payload"]["declarations"]["licences"]
                .as_array()
                .expect("the record names the licence it tried to read");
            assert_eq!(licences.len(), 1, "{licences:?}");
            let unavailable = licences[0]["unavailable"]
                .as_str()
                .expect("why it was not read");
            assert!(unavailable.contains(HUB_ORIGIN_REFUSAL), "{unavailable}");
            assert_eq!(licences[0]["unread"], true, "{licences:?}");
            publisher.stop();
        }
        hub.stop();
    }

    /// An edge with no enrolment has no hub origin: the same origin is
    /// fetched as any other loopback origin is under this policy.
    #[test]
    fn an_unenrolled_edge_has_no_hub_origin_to_refuse() {
        let (mut origin, calls) = counting_origin();
        let (_home, mut server) = server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        server
            .tool_fetch(&json!({"url": format!("{}/api/v1/supplier-credentials", origin.url())}))
            .expect("nothing names this origin as a hub");
        assert!(!calls.lock().expect("lock").is_empty());
        origin.stop();
    }

    /// A non-2xx answer means the request crossed the network and came back
    /// with nothing for context. That is a crossing that happened, ungrounded
    /// and unrefused — an absent record would claim the agent never reached.
    #[test]
    fn a_non_2xx_answer_records_the_crossing_that_happened() {
        let mut origin = bind_local()
            .spawn(|_| Response::text(403, "forbidden"))
            .expect("spawn");
        let (home, mut server) = server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        let url = format!("{}/doc", origin.url());

        let error = server
            .tool_fetch(&json!({"url": url}))
            .expect_err("403 is not content");
        assert!(error.contains("403"), "{error}");

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1, "the request crossed the network");
        assert_eq!(recorded[0]["event"], "crossing_mediated");
        assert_eq!(recorded[0]["payload"]["url"], url);
        assert_eq!(recorded[0]["payload"]["grounded"], false);
        assert!(
            recorded[0]["payload"]["content_hash"].is_null(),
            "nothing entered context, so there is nothing to hash"
        );
        assert!(
            recorded[0]["payload"]["refusal"].is_null(),
            "nothing refused this; the origin simply answered no content"
        );
        origin.stop();
    }

    /// The floor guards addresses, not spellings — and it has to guard them in
    /// this direction too. `localhost.` is a name no private-host rule
    /// matches, and it points at the loopback interface: without vetting what
    /// the name resolved to, the mediated fetch reaches the operator's own
    /// service and the crossing is recorded under a public-looking name, which
    /// is also what would clear the relay's egress floor.
    #[test]
    fn a_public_name_resolving_into_private_space_is_refused_and_recorded() {
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&reached);
        let mut origin = bind_local()
            .spawn(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Response::text(200, "the operator's own service")
            })
            .expect("spawn");
        let (home, mut server) = server(r#"{"policy_mode":"observe"}"#);
        let url = format!("http://localhost.:{}/admin", origin.addr().port());

        let error = server
            .tool_fetch(&json!({"url": url}))
            .expect_err("a name resolving into private space is not mediated");
        assert!(
            error.contains("resolves to a local or private address"),
            "{error}"
        );
        assert_eq!(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the refusal has to land before a socket is opened"
        );

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1, "the enforcement is on the record");
        assert_eq!(recorded[0]["event"], "crossing_refused");
        assert_eq!(recorded[0]["payload"]["url"], url);
        assert!(
            !records(home.path())
                .iter()
                .any(|record| record.to_string().contains("127.0.0.1")),
            "the address the operator runs a service on stays out of the record"
        );
        origin.stop();
    }

    /// And the operator who says so still gets their own network. The vetting
    /// is the same floor the spelling is put to, so the same setting lifts it.
    #[test]
    fn allow_private_hosts_admits_the_address_the_name_resolves_to() {
        let mut origin = bind_local()
            .spawn(|_| Response::text(200, "internal handbook"))
            .expect("spawn");
        let (home, mut server) = server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        let url = format!("http://localhost.:{}/handbook", origin.addr().port());

        let fetched = server
            .tool_fetch(&json!({"url": url}))
            .expect("the operator allowed private hosts");
        assert_eq!(fetched["content"], "internal handbook");

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["event"], "crossing_mediated");
        assert_eq!(recorded[0]["payload"]["grounded"], true);
        origin.stop();
    }

    /// A transport failure took the same unrecorded exit as the two above.
    /// Where nothing answers at all, `robots.txt` is unreachable and the
    /// crossing is refused before the page (RFC 9309 §2.3.1.4), and that is
    /// recorded. Where a current copy of the file is held, the page is
    /// attempted, and the failed attempt is recorded.
    #[test]
    fn a_transport_failure_records_the_attempt() {
        // Bound only to learn the port is free, then let go: nothing answers
        // here, which is what the crossing has to record.
        let closed = Server::bind("127.0.0.1:47799").expect("47799 is reserved for this test");
        let url = format!("http://{}/doc", closed.local_addr().expect("local addr"));
        drop(closed);

        for held in [false, true] {
            let (home, mut server) =
                server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
            if held {
                // An answer three days old, past its day: the file is asked
                // for again, nothing answers, and the held answer rules.
                let then = Utc::now() - chrono::Duration::days(3);
                discovery::DeclarationCache::open(home.path()).save(
                    "127.0.0.1",
                    &discovery::HostRecord {
                        robots: Some(discovery::Probe {
                            url: discovery::robots_url_of(&url),
                            fetched_at: then,
                            expires_at: then + discovery::ROBOTS_CACHE_AGE,
                            final_url: None,
                            status: Some(200),
                            body: Some("User-agent: *\nAllow: /\n".to_owned()),
                            truncated: None,
                            error: None,
                            not_sent: false,
                            declined_redirect: None,
                            cut_short: false,
                        }),
                        ..Default::default()
                    },
                );
                // The server reads the cache as it is on disk.
                server = server_at(home.path());
            }
            let error = server
                .tool_fetch(&json!({"url": url}))
                .expect_err("nothing answers");

            let recorded = crossings(home.path());
            assert_eq!(recorded.len(), 1, "held={held}");
            assert_eq!(recorded[0]["payload"]["url"], url, "held={held}");
            if held {
                assert_eq!(recorded[0]["event"], "crossing_mediated");
                assert_eq!(recorded[0]["payload"]["grounded"], false);
                assert!(recorded[0]["payload"]["failure"].is_string());
                let robots = &recorded[0]["payload"]["declarations"]["robots"];
                assert_eq!(robots["cache"], "fetched", "{robots}");
                assert_eq!(robots["unreachable"], true, "{robots}");
                assert_eq!(robots["held_copy"]["status"], 200, "{robots}");
                assert_eq!(robots["outcome"], "allowed", "{robots}");
            } else {
                assert_eq!(recorded[0]["event"], "crossing_refused");
                assert!(error.contains("could not be reached"), "{error}");
                assert_eq!(
                    recorded[0]["payload"]["declarations"]["robots"]["outcome"],
                    "unreachable"
                );
            }
        }
    }

    fn envelope(url: &str) -> ContextEnvelope {
        ContextEnvelope {
            source_url: url.to_owned(),
            host: grounding::host_of(url),
            title: Some("a result".to_owned()),
            text: Some("a snippet".to_owned()),
            content_hash: Some(sha256_digest(b"a snippet")),
            licence: LicenceState::Unknown,
            declared_date: None,
            native_metadata: json!({}),
            retrieval_rank: 1,
        }
    }

    #[test]
    fn supplier_source_permission_applies_only_to_the_delivering_adapter() {
        let (home, mut server) = server(
            r#"{"policy_mode":"strict","constraints":[{"kind":"allowed_source_host","host":"allowed.example"},{"kind":"allowed_source_provider","provider":"ozone"},{"kind":"denied_source_host","host":"denied.example"}]}"#,
        );
        let mut supplied = acquisition(vec![envelope("https://publisher.example/article")]);
        supplied.envelopes[0].native_metadata = json!({"provider":"ozone"});
        assert_eq!(
            server.deliver(&supplied)["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        supplied.provider = "ozone".into();
        assert_eq!(
            server.deliver(&supplied)["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            server
                .tool_fetch(&json!({"url":"https://publisher.example/article"}))
                .is_err()
        );
        supplied.envelopes = vec![envelope("https://denied.example/article")];
        assert_eq!(
            server.deliver(&supplied)["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let recorded = crossings(home.path());
        assert!(
            recorded
                .iter()
                .any(|record| record["payload"]["url"] == "https://publisher.example/article")
        );
    }

    #[test]
    fn denied_provider_is_refused_before_credentials_or_dispatch() {
        let (home, mut server) = server(
            r#"{"policy_mode":"strict","constraints":[{"kind":"allowed_source_provider","provider":"ozone"},{"kind":"denied_provider","provider":"ozone"}]}"#,
        );
        let error = server
            .tool_search(&json!({"query":"private fixture query", "provider":"ozone"}))
            .unwrap_err();
        assert!(error.contains("denies provider ozone"), "{error}");
        let records = records(home.path());
        assert!(
            records
                .iter()
                .any(|record| record["event"] == "provider_policy_ruled"
                    && record["payload"]["outcome"] == "refused")
        );
        assert!(
            !serde_json::to_string(&records)
                .unwrap()
                .contains("private fixture query")
        );
        assert_eq!(
            server.deliver(&Acquisition {
                provider: "ozone".into(),
                ..acquisition(vec![envelope("https://allowed.example/article")])
            })["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn principal_refusal_precedes_search_dispatch() {
        let (_home, mut server) = server(
            r#"{"policy_mode":"observe","principals":[{"principal":"unbound","subject":"not-this-session"}]}"#,
        );
        let error = server
            .tool_search(&json!({"query":"fixture", "provider":"ozone"}))
            .unwrap_err();
        assert!(error.contains("Principal authority refused"), "{error}");
    }

    #[test]
    fn observed_provider_breach_survives_empty_or_failed_search() {
        let (home, mut server) = server(
            r#"{"policy_mode":"observe","constraints":[{"kind":"denied_provider","provider":"not-a-supplier"}]}"#,
        );
        assert!(
            server
                .tool_search(&json!({"query":"fixture", "provider":"not-a-supplier"}))
                .is_err()
        );
        assert!(
            records(home.path())
                .iter()
                .any(|record| record["event"] == "provider_policy_ruled"
                    && record["payload"]["outcome"] == "allowed_with_breach")
        );
        let mut supplied = acquisition(vec![envelope("https://allowed.example/article")]);
        supplied.provider = "not-a-supplier".into();
        assert_eq!(
            server.deliver(&supplied)["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            crossings(home.path())
                .iter()
                .any(|record| record["payload"].to_string().contains("denies provider"))
        );
    }

    fn acquisition(envelopes: Vec<ContextEnvelope>) -> Acquisition {
        Acquisition {
            provider: "exa".to_owned(),
            capability: ProviderCapability::Search,
            endpoint: "https://api.example/search".to_owned(),
            http_status: Some(200),
            latency_ms: 1,
            provider_request_id: None,
            charge: AcquisitionCharge::default(),
            envelopes,
            raw_response: Vec::new(),
            invocation: None,
        }
    }

    /// One fact, one rule. A search result naming a private address is the
    /// byte-identical fact an observed hook drops at the floor, and which pipe
    /// carried it must not decide whether it enters the record.
    #[test]
    fn a_private_address_in_a_search_result_is_not_recorded() {
        let (home, mut server) = server(r#"{"policy_mode":"observe"}"#);
        let delivered = server.deliver(&acquisition(vec![
            envelope("http://192.168.1.4/x"),
            envelope("https://www.gov.uk/a"),
        ]));

        assert_eq!(
            delivered["results"].as_array().expect("results").len(),
            2,
            "the floor withholds the record, not the result the agent asked for"
        );
        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["payload"]["url"], "https://www.gov.uk/a");
        assert!(
            !records(home.path())
                .iter()
                .any(|record| record.to_string().contains("192.168.1.4")),
            "nothing in the log may name an address the floor keeps out"
        );
    }

    /// The operator's named prefix lowers the floor here exactly as it does on
    /// the observed path, so an internal corpus stays recordable.
    #[test]
    fn a_named_internal_prefix_is_recorded_from_a_search_result() {
        let (home, mut server) = server(
            r#"{"policy_mode":"observe",
                "record_internal_prefixes":["https://rag.corp.internal/"]}"#,
        );
        server.deliver(&acquisition(vec![
            envelope("https://rag.corp.internal/kb/leave"),
            envelope("https://wiki.corp.internal/private"),
        ]));

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0]["payload"]["url"],
            "https://rag.corp.internal/kb/leave"
        );
        assert_eq!(recorded[0]["payload"]["internal"], true);
    }

    /// `context_status` must not advertise a provider no tool on this surface
    /// can reach, and must list every provider `context_search` can dispatch
    /// — which now includes the internal corpus, whose declared capability is
    /// `query`. The status list and the tool schema's sealed enum are one
    /// set.
    #[test]
    fn status_lists_only_the_providers_this_surface_can_reach() {
        let (_home, server) = server(r#"{"policy_mode":"observe"}"#);
        let listed: Vec<String> = server.tool_status()["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .map(|entry| entry["provider"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(
            listed.iter().any(|provider| provider == "internal"),
            "the internal corpus is dispatchable via query and must be advertised"
        );
        assert!(
            !listed.iter().any(|provider| provider == "redpine"),
            "a quote-then-buy provider must not be reachable from a surface with no \
             purchase decision; advertising it would let a session tool spend silently"
        );
        let sealed: Vec<String> =
            tool_definitions()[1]["inputSchema"]["properties"]["provider"]["enum"]
                .as_array()
                .expect("the schema seals the provider list")
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_owned())
                .collect();
        assert_eq!(listed, sealed);
    }

    /// A new user's first failure is a missing credential, and the message is
    /// the onboarding surface: it must name the exact file to create, not
    /// just the variable. Tested through the message builder rather than a
    /// live `tool_search`, which would depend on whatever keys this
    /// machine's environment happens to hold.
    #[test]
    fn a_missing_credential_names_the_operator_file_to_create() {
        let message = unavailable_credential(
            "exa",
            "EXA_API_KEY",
            std::path::Path::new("/home/op/.commonmeasure/credentials.env"),
        );
        assert!(
            message.contains("unavailable: exa needs EXA_API_KEY"),
            "{message}"
        );
        assert!(
            message.contains("/home/op/.commonmeasure/credentials.env"),
            "{message}"
        );
        assert!(message.contains("No search was attempted."), "{message}");
    }

    /// `context_status` carries credential provenance — path, digest and
    /// variable names — so a dossier can say where a key came from without
    /// the record ever holding one.
    #[test]
    fn status_reports_where_credentials_came_from_and_never_a_value() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .expect("policy");
        let loaded = SessionPolicy::load(home.path(), None).expect("the policy loads");
        let log = SessionLog::open(home.path(), "test-session").expect("session log");
        let path = home
            .path()
            .join(commonmeasure_supply::credentials::CREDENTIALS_FILE);
        let credentials = commonmeasure_supply::credentials::CredentialsStatus {
            path: path.clone(),
            loaded: Some(commonmeasure_supply::credentials::LoadedCredentials {
                sha256: "sha256:abc".to_owned(),
                applied: vec!["EXA_API_KEY".to_owned()],
                shadowed: vec!["TAVILY_API_KEY".to_owned()],
            }),
        };
        let server = McpServer::new(log, loaded, "claude-code", None, credentials);
        let status = server.tool_status();
        assert_eq!(status["credentials"]["present"], true);
        assert_eq!(
            status["credentials"]["path"],
            path.display().to_string().as_str()
        );
        assert_eq!(status["credentials"]["sha256"], "sha256:abc");
        assert_eq!(status["credentials"]["applied"][0], "EXA_API_KEY");
        assert_eq!(status["credentials"]["shadowed"][0], "TAVILY_API_KEY");
    }

    // Catches: a released credential missing from the provenance record; a
    // shadowed one reported as in use, without its connection id or mixed
    // into the file's `shadowed` list of strings; an expired one reported as
    // in use or not at all; the record changing shape for a process that
    // holds no release.
    #[test]
    fn released_credentials_are_named_under_hub_and_shadowed_ones_with_their_connection() {
        use commonmeasure_supply::credentials::{
            CredentialsStatus, LoadedCredentials, ReleasedName, ReleasedStanding,
        };
        let name = |connection: &str, provider: &str, variable: &str| ReleasedName {
            connection_id: connection.to_owned(),
            provider: provider.to_owned(),
            variable: variable.to_owned(),
            fetched_at: "2026-09-19T10:00:00.000Z".to_owned(),
            rotated_at: None,
        };
        let file = CredentialsStatus {
            path: "/home/op/.commonmeasure/credentials.env".into(),
            loaded: Some(LoadedCredentials {
                sha256: "sha256:abc".to_owned(),
                applied: vec!["TAVILY_API_KEY".to_owned()],
                shadowed: vec!["YOU_API_KEY".to_owned()],
            }),
        };
        let standing = ReleasedStanding {
            in_use: vec![name("conn-exa", "exa", "EXA_API_KEY")],
            shadowed: vec![name("conn-tavily", "tavily", "TAVILY_API_KEY")],
            ..ReleasedStanding::default()
        };
        let payload = credentials_payload(&file, Some(&standing));
        assert_eq!(
            payload["hub"],
            json!([{
                "connection_id": "conn-exa", "provider": "exa", "variable": "EXA_API_KEY",
                "fetched_at": "2026-09-19T10:00:00.000Z",
            }])
        );
        assert_eq!(payload["shadowed"], json!(["YOU_API_KEY"]));
        assert_eq!(
            payload["hub_shadowed"],
            json!([{
                "connection_id": "conn-tavily", "provider": "tavily",
                "variable": "TAVILY_API_KEY",
            }])
        );
        assert!(payload.get("hub_expired").is_none());

        let absent = CredentialsStatus {
            path: "/home/op/.commonmeasure/credentials.env".into(),
            loaded: None,
        };
        let payload = credentials_payload(&absent, Some(&standing));
        assert_eq!(payload["present"], false);
        assert_eq!(payload["hub"][0]["connection_id"], "conn-exa");
        assert!(payload.get("shadowed").is_none());
        assert_eq!(payload["hub_shadowed"][0]["connection_id"], "conn-tavily");

        // A set past its maximum age is named with the bound it passed, and
        // nothing is listed as in use.
        let lapsed = ReleasedStanding {
            expired: vec![name("conn-exa", "exa", "EXA_API_KEY")],
            max_age: std::time::Duration::from_secs(900),
            ..ReleasedStanding::default()
        };
        let payload = credentials_payload(&absent, Some(&lapsed));
        assert_eq!(payload["hub"], json!([]));
        assert_eq!(
            payload["hub_expired"],
            json!([{
                "connection_id": "conn-exa", "provider": "exa", "variable": "EXA_API_KEY",
                "fetched_at": "2026-09-19T10:00:00.000Z", "max_age_seconds": 900,
            }])
        );

        for unheld in [None, Some(&ReleasedStanding::default())] {
            assert_eq!(credentials_payload(&file, unheld), file.to_value());
        }
    }

    /// The licence a supplier declared travels through delivery untouched:
    /// the internal corpus is the one adapter whose envelopes carry a
    /// written declaration, and flattening it to "unknown" would falsify
    /// what the operator wrote down. The crossing record carries the same
    /// declaration.
    #[test]
    fn a_declared_licence_is_delivered_and_recorded_not_flattened_to_unknown() {
        let (home, mut server) = server(
            r#"{"policy_mode":"observe",
                "record_internal_prefixes":["file:///corpus/"]}"#,
        );
        let mut declared = envelope("file:///corpus/guide.md");
        declared.licence = LicenceState::Declared {
            reference: "fictive-systems/doc-licence-v1".to_owned(),
        };
        declared.declared_date = Some(commonmeasure_types::DeclaredDate {
            date: "2026-03-01".to_owned(),
            provenance: "operator-declared: document date in corpus.json".to_owned(),
        });
        let delivered = server.deliver(&acquisition(vec![declared]));

        let result = &delivered["results"][0];
        assert_eq!(result["licence"]["state"], "declared");
        assert_eq!(
            result["licence"]["reference"],
            "fictive-systems/doc-licence-v1"
        );
        assert_eq!(result["declared_date"]["date"], "2026-03-01");

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["payload"]["internal"], true);
        assert_eq!(
            recorded[0]["payload"]["licence"]["reference"], "fictive-systems/doc-licence-v1",
            "the record carries the declaration, not a flattened unknown"
        );
    }

    /// A release the ledger refuses is not a silent hold until the expiry
    /// sweep: the session records the gap naming the reservation, and the
    /// tool error names it too.
    #[test]
    fn a_refused_release_is_recorded_before_the_sweep_reclaims_it() {
        let (home, mut server) = server(r#"{"policy_mode":"observe"}"#);
        let (context, mut decision) = held_reservation(home.path());
        let reservation = decision.reservation.take().expect("held");
        refuse_ledger_writes(&context);

        let detail = server
            .release_mediated(
                &context,
                &mut decision.record,
                &reservation,
                "the search failed before any receipt (test)",
            )
            .expect("a refused release is reported, not dropped");
        assert!(detail.contains(&reservation.id.to_string()), "{detail}");
        assert!(detail.contains("could not be released"), "{detail}");
        assert_eq!(decision.record["settlement"]["reconciled"], false);
        assert_eq!(
            decision.record["settlement"]["reservation_id"],
            json!(reservation.id)
        );

        let gaps = allowance_gaps(home.path());
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let payload = &gaps[0]["payload"];
        assert_eq!(payload["reason"], "evidence_missing");
        assert_eq!(payload["reservation_id"], json!(reservation.id));
        assert_eq!(payload["observed"], Value::Null);
        assert_eq!(payload["detail"], detail);
        assert_eq!(payload["host"], "claude-code");
    }

    /// The double fault on the mediated surface: the reservation was
    /// written, the receipt reported the charge, and the ledger refused the
    /// commit twice. The session states that money moved and the ledger did
    /// not count it, naming the reservation and the amount.
    #[test]
    fn a_settlement_the_ledger_cannot_write_leaves_a_gap_naming_the_money_that_moved() {
        let (home, mut server) = server(r#"{"policy_mode":"observe"}"#);
        let (context, mut decision) = held_reservation(home.path());
        let reservation = decision.reservation.take().expect("held");
        refuse_ledger_writes(&context);

        let charge = AcquisitionCharge {
            money: Some(Money::new("USD", 7_000)),
            native: None,
        };
        server.settle_mediated(&context, &mut decision.record, Some(&reservation), &charge);

        let settlement = &decision.record["settlement"];
        assert_eq!(settlement["reconciled"], false);
        assert_eq!(settlement["reservation_id"], json!(reservation.id));
        assert_eq!(settlement["observed"]["micros"], 7_000);
        assert!(
            settlement["error"]
                .as_str()
                .expect("the error is stated")
                .contains("retried once under the lock"),
            "{settlement}"
        );

        let gaps = allowance_gaps(home.path());
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let payload = &gaps[0]["payload"];
        assert_eq!(payload["reason"], "evidence_missing");
        assert_eq!(payload["reservation_id"], json!(reservation.id));
        assert_eq!(payload["observed"]["currency"], "USD");
        assert_eq!(payload["observed"]["micros"], 7_000);
        let detail = payload["detail"].as_str().expect("the detail is stated");
        assert!(detail.contains("Money moved"), "{detail}");
        assert!(detail.contains("0.007000 USD"), "{detail}");
        assert!(detail.contains(&reservation.id.to_string()), "{detail}");
    }

    /// The seam of the PII rule under strict: a public page is admitted with the finding recorded; a named
    /// internal prefix, a private address and the operator's corpus are
    /// refused; the switch refuses the public page too. A loopback origin
    /// cannot stand in for a public source here, because the privacy floor
    /// classifies it as private, so the rule is held at the classifier and
    /// the processor, and the public case is exercised live.
    #[test]
    fn a_public_source_is_admitted_with_the_finding_recorded_and_the_other_classes_are_refused() {
        use commonmeasure_runtime::processor::pii;
        let text = "Contact the Land Registry at customersupport@landregistry.gov.uk.";
        let public = "https://www.gov.uk/government/organisations/land-registry";
        let (home, strict) = server(
            r#"{"policy_mode":"strict","allow_private_hosts":true,
                "record_internal_prefixes":["https://rag.corp.internal/"]}"#,
        );
        let rule = |url: &str, provider: Option<&str>| {
            let class = source_class(&strict.policy, url, provider);
            let (_, ruling) = pii::invoke(
                strict.policy.mode(),
                class,
                strict.policy.refuse_on_pii(),
                url,
                url,
                text,
                None,
            );
            (class, ruling.is_refusal())
        };
        assert_eq!(rule(public, None), (SourceClass::Public, false));
        assert_eq!(
            rule("https://rag.corp.internal/contacts", None),
            (SourceClass::Internal, true)
        );
        assert_eq!(
            rule("http://127.0.0.1:8080/handbook", None),
            (SourceClass::Internal, true)
        );
        assert_eq!(
            rule("file:///corpus/contacts.md", Some(INTERNAL_PROVIDER)),
            (SourceClass::Internal, true)
        );
        assert_eq!(
            rule("https://a.example/result", Some("exa")),
            (SourceClass::Public, false)
        );

        let (_, switched) = server(r#"{"policy_mode":"strict","refuse_on_pii":true}"#);
        let (_, ruling) = pii::invoke(
            switched.policy.mode(),
            source_class(&switched.policy, public, None),
            switched.policy.refuse_on_pii(),
            public,
            public,
            text,
            None,
        );
        assert!(ruling.is_refusal(), "the switch refuses the public page");
        drop(home);
    }
}
