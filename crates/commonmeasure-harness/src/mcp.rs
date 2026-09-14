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

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use commonmeasure_http::{Request, Response};
use commonmeasure_runtime::allowance::{AllowanceContext, GateDecision, Reservation};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_runtime::processor::pii::SourceClass;
use commonmeasure_supply::{
    Acquisition, INTERNAL_PROVIDER, SupplyError, supplier_from_environment,
};
use commonmeasure_types::{
    AcquisitionCharge, ContextEnvelope, ContextJob, Gap, GapReason, LicenceState, PolicyMode,
    ProviderCapability,
};
use serde_json::{Value, json};

use crate::declarations::{Effective, PRODUCT_TOKEN, TELEMETRY_PROFILE};
use crate::discovery::{self, DeclarationCache, Declarations, Governing, ManifestCache};
use crate::grounding;
use crate::identity::{Identity, PresentedIdentity, SigningIdentity};
use crate::policy::SessionPolicy;
use crate::session::{Crossing, CrossingMode, SessionLog};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
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
    /// is started and never asked for anything leaves no session file.
    client: Option<crate::session::ClientIdentity>,
    /// The protocol version the client asked for in `initialize`.
    client_protocol: Option<String>,
    /// Whether the `client_identified` record has been written.
    client_recorded: bool,
    /// The directory this server was started in. Stdio carries no cwd, but
    /// the server inherits the harness's, and it is the same directory the
    /// session's policy scope was resolved against.
    cwd: Option<String>,
    /// Where provider credentials came from at start: the operator file's
    /// path and digest plus variable names, never a value. What an
    /// unavailable message points at, and what `context_status` reports.
    credentials: commonmeasure_supply::credentials::CredentialsStatus,
    /// The per-host cache of `robots.txt` and licence documents
    /// (`crate::discovery`), under the operator home beside the policy.
    declarations: DeclarationCache,
    /// The per-host cache of Content Telemetry discovery manifests.
    manifests: ManifestCache,
    /// The prompt records read so far from this session's log, advanced at
    /// each fetch rather than re-read from the start.
    prompts: crate::session::PromptCursor,
    /// The receiver named in `<home>/relay.json` when the server started, or
    /// none. A reporting demand is met only where reports can leave, and the
    /// relay reads the same file.
    receiver: Option<String>,
    /// What every request this server makes presents to a publisher: the
    /// enrolled key it signs with, or why it signs nothing. Read once at
    /// start, because it is the identity of the session, and a session that
    /// changed identity halfway through would leave a record no publisher
    /// could reconcile with its own logs.
    identity: Identity,
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
        Self {
            session,
            policy,
            host: host.to_owned(),
            client: None,
            client_protocol: None,
            client_recorded: false,
            cwd,
            credentials,
            declarations: DeclarationCache::open(&home),
            manifests: ManifestCache::open(&home),
            prompts: crate::session::PromptCursor::default(),
            receiver: configured_receiver(&home),
            // An identity this runtime cannot read is not an unenrolled
            // edge: it is a broken one, and it must not fetch as though it
            // had no key. The reason is what every record then carries.
            identity: Identity::load(&home).unwrap_or_else(|reason| Identity::Unsigned { reason }),
        }
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
            let Some(response) = self.handle_line(&line) else {
                continue; // A notification: no response is owed.
            };
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(())
    }

    fn handle_line(&mut self, line: &str) -> Option<Value> {
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
        // Requests carry an id and are owed a response — even a request too
        // malformed to name a method, or a host waiting on it hangs.
        // Notifications carry no id and get nothing.
        let id = message.get("id").cloned()?;
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(error_response(id, -32600, "request has no method"));
        };
        let method = method.to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(self.handle_request(id, &method, &params))
    }

    fn handle_request(&mut self, id: Value, method: &str, params: &Value) -> Value {
        match method {
            "initialize" => {
                self.identify_client(params);
                ok_response(
                    id,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {
                            "name": "commonmeasure",
                            "title": "Common Measure mediated context",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                )
            }
            "ping" => ok_response(id, json!({})),
            "tools/list" => ok_response(id, json!({"tools": tool_definitions()})),
            "tools/call" => self.handle_tool_call(id, params),
            _ => error_response(id, -32601, &format!("method {method} not found")),
        }
    }

    /// Keep what the client said about itself. A client that sends no
    /// `clientInfo` leaves no record and no name on its crossings: the
    /// absence is the fact, and no default is put in its place. Nothing is
    /// written here: hosts start a server per window or per launch, and a
    /// server that is never asked for anything must leave no session file.
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

    /// Write the `client_identified` record once, before the first record a
    /// tool call leaves, so it precedes every crossing in the log. A failed
    /// append cannot fail the tool call; the log owes a gap.
    fn record_client(&mut self) {
        if self.client_recorded {
            return;
        }
        if let Some(client) = &self.client {
            let _ = self.session.record_client_identified(
                &self.host,
                client,
                self.client_protocol.as_deref(),
            );
        }
        self.client_recorded = true;
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
            "context_fetch" => self.tool_fetch(&arguments),
            "context_search" => self.tool_search(&arguments),
            "context_status" => Ok(self.tool_status()),
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
        self.record_client();
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .ok_or("url is required")?;
        if !self.policy.mediates_address(url) {
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

        // What the host declares, read before the request: robots.txt for
        // the product token's group and the licence it names. The probe goes
        // through the same host policy and address floor as the page.
        let terms = self.policy.terms_for(&grounding::host_of(url)).cloned();
        let mut declarations = discovery::before_fetch(
            &self.declarations,
            url,
            terms.as_ref(),
            Utc::now(),
            &|probe_url| self.probe(probe_url),
        );
        // Pre-authorisation, before any other ruling on the licence. Where
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
            facts.declarations = Some(declarations);
            facts.named_by = named_by;
            facts.allowance =
                self.release_authorisation(authorised, "the fetch was refused before the request");
            self.record(facts);
            return Err(format!(
                "refused before the crossing: {reason} (operator policy in {}; the source's \
                 declarations are recorded in {})",
                self.policy.source().display(),
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
            match policy.admit_host(candidate) {
                Ruling::Refused { reason, .. } => Err(reason.clone()),
                _ => Ok(()),
            }
        };
        let reaches = |candidate: &str, addresses: &[SocketAddr]| {
            reaches_allowed_addresses(policy, candidate, addresses)
        };
        let presented = self.identity.presented();
        let (final_url, response) = match follow(
            url,
            request,
            commonmeasure_http::CLIENT_TIMEOUT,
            self.identity.signer(),
            &allowed,
            &reaches,
        ) {
            Ok(reached) => reached,
            // A refused hop is enforcement, and enforcement is on the record:
            // the same `crossing_refused` a directly named URL earns, naming
            // the hop that was refused rather than the one asked for.
            Err(FetchFailure::Refused {
                url: refused,
                reason,
            }) => {
                let mut facts = FetchFacts::refused(&refused, reason.clone());
                facts.declarations = Some(declarations);
                facts.named_by = named_by;
                facts.content_telemetry_id = content_telemetry_id;
                facts.identity = Some(presented.clone());
                facts.allowance =
                    self.release_authorisation(authorised, "a redirect hop was refused");
                self.record(facts);
                let hop = if refused == url {
                    String::new()
                } else {
                    format!("a redirect to {refused}; ")
                };
                return Err(format!(
                    "refused before the crossing: {reason} \
                     ({hop}operator policy in {})",
                    self.policy.source().display()
                ));
            }
            // Nothing answered, but the request left the machine. An
            // unrecorded attempt would make the log claim the agent never
            // reached for this.
            Err(FetchFailure::Failed {
                url: attempted,
                detail,
            }) => {
                let mut facts = FetchFacts::carried(&attempted);
                facts.breach = merge_breaches(
                    self.breach_at(url, &attempted, &ruling),
                    breaches_of(&before).as_deref(),
                );
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
            &|probe_url| self.probe(probe_url),
        );
        let after = self.rule_on_declarations(&mut declarations);
        let licence = licence_of(&declarations);

        // The body becomes the text the screens rule on and the agent
        // reads: an HTML page is extracted to its readable text, anything
        // else is decoded and delivered as it stands. The extraction record
        // is written before the crossing and carries the hash of the bytes
        // received beside the hash of the text delivered, so the crossing's
        // two hashes are tied by a record a reader can re-derive.
        let (extraction_invocation, extraction) = commonmeasure_runtime::processor::extract::invoke(
            &final_url,
            &response.body,
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
        // was for: under strict the bytes were fetched and are withheld from
        // context, hash recorded, grounded false, the same shape as a PII
        // refusal; under observe and prefer they are carried with the breach.
        if let Some(reason) = first_refusal(&after) {
            let mut facts = FetchFacts::refused(&final_url, reason.clone());
            facts.http_status = Some(response.status);
            facts.content_hash = Some(hash);
            facts.retrieved_hash = Some(retrieved_hash);
            facts.estimated_tokens = Some(tokens);
            facts.breach = host_breach;
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
        // first so the crossing can reference it by sequence.
        let manifest_record = self.resolve_manifest(&final_url);
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

        Ok(json!({
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
        }))
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
    fn resolve_manifest(&mut self, page_url: &str) -> Option<u64> {
        let resolution =
            discovery::resolve_manifest(&self.manifests, page_url, Utc::now(), &|probe_url| {
                self.probe(probe_url)
            })?;
        let host = grounding::host_of(page_url);
        self.session.record_manifest(&host, &resolution).ok()
    }

    /// Fetch one URL for a declaration or manifest probe, under the same host
    /// policy and address floor as a page and a shorter budget. A refused
    /// host is an error naming the refusal, so the record says why the
    /// document could not be read.
    fn probe(&self, url: &str) -> Result<(String, Response), String> {
        if !self.policy.mediates_address(url) {
            return Err(format!(
                "{url} is a local or private address, which Common Measure does not mediate"
            ));
        }
        if let Ruling::Refused { reason, .. } = self.policy.admit_host(url) {
            return Err(format!("{url} is refused by policy: {reason}"));
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
            match policy.admit_host(candidate) {
                Ruling::Refused { reason, .. } => Err(reason.clone()),
                _ => Ok(()),
            }
        };
        let reaches = |candidate: &str, addresses: &[SocketAddr]| {
            reaches_allowed_addresses(policy, candidate, addresses)
        };
        follow(
            url,
            request,
            discovery::PROBE_TIMEOUT,
            self.identity.signer(),
            &allowed,
            &reaches,
        )
        .map_err(|failure| match failure {
            FetchFailure::Refused { url, reason } => {
                format!("{url} is refused by policy: {reason}")
            }
            FetchFailure::Failed { url, detail } => format!("{url}: {detail}"),
        })
    }

    /// Rule on what the source declared, under the session's mode: the
    /// robots.txt access rule for the product token, a disallowed AI-input
    /// statement, a licence whose AI-input permission is conditional on a
    /// payment this edge cannot make, and a reporting demand this session
    /// cannot meet. Terms the operator holds for the host govern over a
    /// preference and over the licence's own terms (vocabulary draft
    /// section 5.2), so under terms only the operator's own reporting duty
    /// is ruled on.
    fn rule_on_declarations(&self, declarations: &mut Declarations) -> Vec<Ruling> {
        let mode = self.policy.mode();
        let mut rulings = Vec::new();
        let mut reporting = None;
        if let Some(reading) = &declarations.robots.reading
            && reading.crawlable == Some(false)
        {
            rulings.push(breach(
                mode,
                format!(
                    "robots.txt disallows {PRODUCT_TOKEN} here (group {}, {}).",
                    reading.group.as_deref().unwrap_or("*"),
                    reading.access_rule.as_deref().unwrap_or("Disallow")
                ),
                "The source's robots.txt disallows this fetcher at this path.",
            ));
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
                    for demand in &terms.reporting {
                        if demand.kind != "telemetry" {
                            continue;
                        }
                        let ruling = self.reporting_ruling(demand);
                        if let Some(reason) = &ruling.reason {
                            rulings.push(breach(
                                mode,
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
        declarations.reporting = reporting;
        rulings
    }

    /// Rule on a licence's telemetry reporting demand: the profile must be
    /// the Content Telemetry binding this relay speaks, the demanded
    /// conformance level one it emits (retrieval or grounding), the
    /// session's scope must clear telemetry egress and a receiver must be
    /// configured, or nothing would ever be reported. An unrecognised profile
    /// is a demand this client cannot comply with, which RSL says leaves the
    /// activity unlicensed.
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
    /// telemetry egress and a receiver is configured, with the receiver
    /// named in the record.
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
            None
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
        self.record_client();
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

        let supplier = supplier_from_environment(provider).map_err(|error| match error {
            SupplyError::CredentialMissing { variable } => {
                unavailable_credential(provider, &variable, self.credentials.path.as_path())
            }
            other => other.to_string(),
        })?;
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
                let mut detail = error.to_string();
                if let Some((context, mut decision)) = consulted
                    && let Some(reservation) = decision.reservation.take()
                    && let Some(gap) = self.release_mediated(
                        &context,
                        &mut decision.record,
                        &reservation,
                        &format!("the search failed before any receipt ({error})"),
                    )
                {
                    detail = format!("{detail}; {gap}");
                }
                return Err(detail);
            }
        };
        let mut delivery = self.deliver(&acquisition);
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
        let _ = self
            .session
            .record_allowance_gap(&self.host, Some(reservation.id), None, &detail);
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
        ) {
            let _ = self.session.record_allowance_gap(
                &self.host,
                failure.reservation_id,
                failure.observed.as_ref(),
                &failure.to_string(),
            );
        }
    }

    /// Judge, record and shape what a provider returned. Separate from
    /// reaching the provider, which is the half that needs a credential.
    fn deliver(&mut self, acquisition: &Acquisition) -> Value {
        let mut results = Vec::new();
        for envelope in &acquisition.envelopes {
            // The privacy floor is the record-admission predicate whichever
            // pipe carried the fact: a provider naming a private address is
            // still the operator's own business, and the observed path drops
            // the identical fact. It is still judged and can still be refused
            // — only the record is withheld, here and for the processor
            // invocation that would otherwise name the same address.
            let recordable = self.policy.records_address(&envelope.source_url);
            let ruling = self.policy.admit(envelope);
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
                        let _ = self.session.record_processor(pii_invocation.to_value());
                        let _ = self
                            .session
                            .record_processor(injection_invocation.to_value());
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
            "refused": acquisition.envelopes.len() - results.len(),
            "charge": acquisition.charge,
            "latency_ms": acquisition.latency_ms,
            "recorded_in": self.session.path().display().to_string(),
        })
    }

    fn tool_status(&self) -> Value {
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
                let configured = supplier_from_environment(provider).is_ok();
                // Where a configured provider's variable came from: set into
                // the environment from the operator file at start, or already
                // in the environment the harness was launched with. Names
                // only — the value itself is never reported.
                let source = configured
                    .then(|| commonmeasure_supply::required_variable(provider))
                    .flatten()
                    .map(|variable| {
                        let from_file = self.credentials.loaded.as_ref().is_some_and(|loaded| {
                            loaded.applied.iter().any(|name| name == variable)
                        });
                        if from_file {
                            "operator-file"
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
            "host": self.host,
            "client": self.client,
            "evidence": self.session.path().display().to_string(),
            "policy": self.policy.describe(),
            "credentials": self.credentials.to_value(),
            "providers": providers,
            "processors": commonmeasure_runtime::processor::installed(),
        })
    }

    /// Record the crossing, and never let a failure to record take down the
    /// agent's tool call. An unrecorded crossing leaves the log owing a gap,
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
        let _ = self.session.record_crossing(&crossing);
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
    if let Some(terms) = &declarations.terms {
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
        "terms": declarations.terms.as_ref().map(|terms| &terms.reference),
        "robots_group": declarations.robots.reading.as_ref().and_then(|r| r.group.clone()),
        "licences": declarations.licences.iter().map(|licence| json!({
            "url": licence.url,
            "mechanism": licence.mechanism,
            "payment": licence.terms.as_ref().and_then(|t| t.payment.clone()),
            "reporting": licence.terms.as_ref().map(|t| t.reporting.clone()),
            "unavailable": licence.unavailable,
        })).collect::<Vec<_>>(),
    })
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

/// Why a fetch produced no bytes. Both variants name the URL the record must
/// carry, which is the hop that failed rather than the one first asked for.
enum FetchFailure {
    /// Policy refused this hop before it happened.
    Refused { url: String, reason: String },
    /// Nothing usable answered.
    Failed { url: String, detail: String },
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
    // intranet is where it was always going.
    if grounding::matches_internal_prefix(url, policy.internal_prefixes()) {
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
/// `identity` signs each hop separately rather than the fetch once. The
/// signature covers the authority it is sent to and the correlation id the hop
/// carries, and a redirect changes both: one signature reused across hops
/// would attest to the host that redirected and would be refused by the host
/// that answered. An edge with no key to sign with sends every hop unsigned.
fn follow(
    url: &str,
    request: Request,
    budget: Duration,
    identity: Option<&SigningIdentity>,
    allowed: UrlCheck<'_>,
    reaches: AddressCheck<'_>,
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
            return Err(FetchFailure::Failed {
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
        let response = match commonmeasure_http::send_to(&current, &addresses, hop, budget) {
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
    }
    Err(FetchFailure::Failed {
        url: current,
        detail: "redirect limit exceeded".to_owned(),
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "context_fetch",
            "description": "Fetch a URL through Common Measure. An HTML page is delivered as its readable text, not its markup; any other body is delivered as received. The crossing is checked against operator source policy before it happens and recorded either way, with the hash of exactly what entered context and the hash of the bytes the origin served. Prefer this over WebFetch when the operator wants an evidence trail.",
            "inputSchema": {
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }
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
            }
        },
        {
            "name": "context_status",
            "description": "The current session, where its evidence is written, the operator's source policy, and which providers are configured.",
            "inputSchema": {"type": "object", "properties": {}}
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
        let loaded = SessionPolicy::load(home.path(), None).expect("the policy loads");
        let log = SessionLog::open(home.path(), "test-session").expect("session log");
        let credentials = commonmeasure_supply::credentials::CredentialsStatus {
            path: home
                .path()
                .join(commonmeasure_supply::credentials::CREDENTIALS_FILE),
            loaded: None,
        };
        (
            home,
            McpServer::new(log, loaded, "claude-code", None, credentials),
        )
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
        let mut origin = bind_local()
            .spawn(|_| {
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
    #[test]
    fn a_transport_failure_records_the_attempt() {
        // Bound only to learn the port is free, then let go: nothing answers
        // here, which is what the crossing has to record.
        let closed = Server::bind("127.0.0.1:47799").expect("47799 is reserved for this test");
        let url = format!("http://{}/doc", closed.local_addr().expect("local addr"));
        drop(closed);

        let (home, mut server) = server(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        assert!(server.tool_fetch(&json!({"url": url})).is_err());

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["event"], "crossing_mediated");
        assert_eq!(recorded[0]["payload"]["url"], url);
        assert_eq!(recorded[0]["payload"]["grounded"], false);
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

    /// The seam of the PII rule under strict: the public page from the
    /// partner rehearsal is admitted with the finding recorded; a named
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
