//! The purpose-limited projection from the evidence logs to the wire.
//!
//! What crosses: cleared minimal turn boundaries, and retrieval and grounding
//! facts from witnessed crossings (`observed`, `mediated`) in session logs and
//! from admitted sources in published runs, and, per session, how many
//! crossings policy refused, as a count and nothing else. What stays home, by
//! construction: prompts, answers, evaluator output, every refused crossing
//! itself, sources the run rejected, policy detail, every reconstructed
//! crossing, and every crossing to a private address or a named internal
//! prefix. A transcript claim must
//! never arrive at a publisher looking witnessed, and the standard has no
//! field that could carry the caveat, so reconstructed crossings are not
//! projected at all rather than projected with a private marking. Internal
//! crossings are the same shape of decision: `record_internal_prefixes` is
//! consent to *record*, never consent to *send*, so the projection re-applies
//! the privacy floor with no exceptions — an internal corpus is
//! operator-record only, as `docs/contracts/session-evidence.md` promises.
//!
//! Event identity is derived deterministically from the evidence record each
//! event projects (session and log position, or run, plan and rank), so a
//! redelivery after a crash carries the same ids and the receiver counts each
//! fact once.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_types::canonical::canonical_json;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::wire::{TurnPrivacy, WireBatch, WireEvent, WireEventKind, WireInstance, WireTurn};

/// Receivers cap batches (oa-server refuses more than 500 events); staying
/// under the cap here keeps a large evidence log deliverable without negotiation.
pub const MAX_EVENTS_PER_BATCH: usize = 500;

/// The agent identifier where no enrolled key signed the requests the events
/// report: a session an edge recorded while holding no key, and every
/// published run, whose sources a supply adapter acquired under its own
/// contract rather than the signed mediated fetch.
///
/// It names the software, not an identity. A key id exists so a publisher can
/// match a signed request to the report it later receives; these events answer
/// no signed request, so naming a key id would offer a match that cannot be
/// made. The product name says that plainly, where a per-machine value would
/// look like an identifier without being one.
pub const EMITTER_ID: &str = "commonmeasure";

/// The extension field carrying the operator's host tool. Namespaced, as the
/// standard asks of custom fields, under the namespace every format
/// identifier in sealed evidence uses; the hyphen cannot collide with a
/// standard field, which are all snake_case.
pub const HOST_TOOL_FIELD: &str = "contextops-host-tool";

/// The extension field naming the supplier that served an acquisition: the
/// provider name a plan and a mediated search use (`exa`, `ozone`). Present on
/// the retrieval and grounding events of a supplied source and on nothing
/// else. It takes the product's current name rather than the `contextops-`
/// namespace of [`HOST_TOOL_FIELD`]: that namespace is kept only for
/// identifiers already inside sealed evidence, and this field was born after
/// the rename.
///
/// The supplier is on the wire so a content owner, or the network reporting
/// for it, can tell a retrieval served through a licensed supplier from one
/// fetched from the open web. The operator's own corpus (`internal`) and a
/// catalogued skill (`skill:<name>`) are not suppliers and are never named:
/// the first would disclose operator layout, the second a program that
/// published nothing (`wire_supplier`).
pub const SUPPLIER_FIELD: &str = "commonmeasure-supplier";

/// Informational declaration on each event: batch envelopes have no standard
/// coverage field. The selection rule is defined in the session-evidence
/// contract under this opaque, versioned reference.
pub const PROJECTION_FIELD: &str = "commonmeasure-projection";
/// Opaque reference for the exclusion rule in the session-evidence contract.
pub const SELECTION_TERMS: &str = "commonmeasure:telemetry-selection:v1";

fn projection_data() -> Map<String, Value> {
    let mut data = Map::new();
    data.insert(
        PROJECTION_FIELD.to_owned(),
        json!({
            "conformance_level": "grounding",
            "coverage": {"mode": "selected", "terms_ref": SELECTION_TERMS},
        }),
    );
    data
}

/// Counts without their recorded basis are unknown on the wire. The custom
/// `token_basis` field preserves the existing estimate vocabulary; it is not
/// a standard Content Telemetry field or a claim of comparable tokenisation.
fn ingestion(data: &mut Map<String, Value>, count: &Value, basis: &Value) {
    if let (Some(tokens), Some(basis)) = (count.as_u64(), basis.as_str())
        && !basis.trim().is_empty()
    {
        data.insert("tokens_ingested".to_owned(), json!(tokens));
        data.insert("token_basis".to_owned(), json!(basis));
    }
}

/// Namespace for every id this relay derives. Fixed by derivation from a name
/// no other emitter plausibly uses, so ids are stable across runs and machines
/// without a stored counter.
pub fn namespace() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, b"https://contextops.dev/relay/v1")
}

/// The projected session identifier: the evidence log's own id when it already is a
/// UUID (hosts like Claude Code supply one), otherwise derived from it.
pub fn session_uuid(session_id: &str) -> Uuid {
    Uuid::parse_str(session_id)
        .unwrap_or_else(|_| derived_id(&json!({"kind": "session", "session": session_id})))
}

/// A UUIDv5 over the canonical JSON of the pre-image, in this relay's
/// namespace.
///
/// Every id the relay derives is named this way, so a receiver or a reviewer
/// recomputes one with a canonical serialiser and a version 5 UUID and
/// nothing else, rather than reproducing a delimiter convention by eye. The
/// pre-image says which record the id stands for and carries no content
/// (`crates/commonmeasure-types/src/canonical.rs`).
fn derived_id(pre_image: &Value) -> Uuid {
    Uuid::new_v5(&namespace(), canonical_json(pre_image).as_bytes())
}

fn normalise_timestamp(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw).ok().map(|parsed| {
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

fn license_ref(payload: &Value) -> Option<String> {
    (payload["licence"]["state"] == json!("declared"))
        .then(|| payload["licence"]["reference"].as_str().map(str::to_owned))
        .flatten()
}

/// The origin batches are being projected for, where the receiver names one.
/// The relay's one origin comparison: the instance issuer and the owner of an
/// API key are both judged by it.
pub(crate) fn receiver_origin(receiver: Option<&str>) -> Option<url::Origin> {
    commonmeasure_harness::enrolment::origin_of(receiver?)
}

/// The record's `instance` reference as it may appear on a content event.
///
/// The reference is private to the operator and the registration service
/// that issued it, so it is projected only where the receiver's origin is the
/// recorded issuer: the operator's own hub, which joins the event to the
/// instance's reporting duties and removes the member before onward delivery.
/// Any other receiver, and a caller that names none, gets no member. A record
/// without a reference, or with one that is not an issuer, an identifier and
/// an integer revision, projects nothing: absence claims nothing, and the
/// event itself is unaffected.
fn wire_instance(payload: &Value, receiver: Option<&url::Origin>) -> Option<WireInstance> {
    let reference = &payload["instance"];
    let issuer = reference["issuer"].as_str()?.trim_end_matches('/');
    let id = reference["id"].as_str().filter(|id| !id.is_empty())?;
    let revision = reference["revision"].as_i64()?;
    (url::Url::parse(issuer).ok()?.origin() == *receiver?).then(|| WireInstance {
        issuer: issuer.to_owned(),
        id: id.to_owned(),
        revision,
    })
}

/// Remove the `instance` member from each event of a batch document that this
/// receiver did not issue, by the rule projection applies (`wire_instance`).
///
/// Projection decides for the receiver named when the batch was queued. A
/// batch can wait in the spool across a change of receiver (`--receiver`, or
/// enrolment with another hub rewriting `relay.json`), so delivery applies
/// the rule again to the document it is about to post. A batch carries no
/// digest or signature over its events, so removing the member invalidates
/// nothing. The spooled file is left as queued: a later delivery to the
/// issuer still carries the member.
///
/// Returns how many members were removed. A document without an `events`
/// array is left as found: the relay queues none, and indexing for a mutable
/// array would insert `"events": null` into it.
pub(crate) fn withhold_unissued_instances(document: &mut Value, receiver: &str) -> u64 {
    let receiver = receiver_origin(Some(receiver));
    let mut withheld = 0;
    let events = document.get_mut("events").and_then(Value::as_array_mut);
    for event in events.into_iter().flatten() {
        if event.get("instance").is_some()
            && wire_instance(event, receiver.as_ref()).is_none()
            && let Some(members) = event.as_object_mut()
        {
            members.remove("instance");
            withheld += 1;
        }
    }
    withheld
}

/// The URL as it may appear on the wire: the witnessed URL with any userinfo
/// removed. Credentials embedded in a fetched URL
/// (`https://user:secret@host/path`) are the operator's authentication, never
/// part of content identity, and the projection is the last point before they
/// would be written to the spool and posted to the receiver. Only the userinfo
/// goes: a token in a query string is indistinguishable from an ordinary
/// parameter here, so it stays the operator's to redact through the privacy
/// floor rather than something this function guesses at.
fn wire_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let authority_start = scheme_end + "://".len();
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |offset| authority_start + offset);
    // The last `@` in the authority delimits the userinfo; an unescaped `@`
    // cannot appear in a host, so anything after it is the host.
    match url[authority_start..authority_end].rfind('@') {
        Some(at) => format!(
            "{}{}",
            &url[..authority_start],
            &url[authority_start + at + 1..]
        ),
        None => url.to_owned(),
    }
}

/// Whether a witnessed URL may leave the machine. The privacy floor holds
/// unconditionally on egress: a crossing that entered the record because the
/// operator named its prefix internal is exactly the crossing that must not
/// be projected, so the named prefixes are an exclusion here, the inverse of
/// the role they play at capture.
fn projectable(url: &str, internal_prefixes: &[String]) -> bool {
    commonmeasure_harness::grounding::recordable(url)
        && !commonmeasure_harness::grounding::matches_internal_prefix(url, internal_prefixes)
}

/// Whether an evidence record is a crossing this machine watched happen.
/// `crossing_refused` is a rejected source and `crossing_reconstructed` is a
/// transcript claim; neither is witnessed, and neither leaves. The one home for
/// the predicate, because the caller deciding what may be projected and the
/// projection deciding what to build must be asking the same question.
pub fn is_witnessed(record: &Value) -> bool {
    matches!(
        record["event"].as_str(),
        Some("crossing_observed" | "crossing_mediated")
    )
}

/// Whether the host recorded a turn boundary. Its clearance is resolved from
/// `payload.detail.cwd`, independently of any neighbouring crossing.
pub fn is_turn_boundary(record: &Value) -> bool {
    matches!(
        record["event"].as_str(),
        Some("turn_started" | "turn_completed")
    )
}

/// Whether an evidence record is a crossing policy refused. Nothing of it is
/// projected; the session's batches carry how many there were.
pub fn is_refused(record: &Value) -> bool {
    record["event"] == json!("crossing_refused")
}

/// One session's projection: the batches to deliver, and where each of their
/// events came from.
pub struct SessionProjection {
    pub batches: Vec<WireBatch>,
    /// The owning log position for each projected event, in projection order.
    /// Context-entry events use the acquisition's position for its clearance
    /// and engagement; their IDs still use the observation's own position.
    /// A caller that resolved a per-crossing property — the relay
    /// resolves each crossing's governing engagement before asking
    /// `may_project` — can attribute the events back to it through this,
    /// instead of re-deriving an event id from the naming scheme above and
    /// owning a second copy of it.
    pub event_positions: Vec<(Uuid, usize)>,
    /// The refused crossings `may_project` cleared, counted; what every
    /// batch of this session carries as `refused`.
    pub refused: u64,
}

/// Resolve a host observation through its admitted acquisition. Clearance and
/// privacy belong to that acquisition, never to a URL supplied by the host.
fn acquisition_position(
    records: &[Value],
    handle: &Value,
    internal_prefixes: &[String],
    may_project: &dyn Fn(usize) -> bool,
) -> Option<usize> {
    let handle = handle.as_str()?;
    Uuid::parse_str(handle).ok()?;
    records.iter().enumerate().find_map(|(position, record)| {
        let p = &record["payload"];
        (record["event"] == "crossing_mediated"
            && p["acquisition_id"] == handle
            && p["context_observation"] == "host_required"
            && p["refusal"].is_null()
            && p["failure"].is_null()
            && p["http_status"]
                .as_u64()
                .is_some_and(|status| (200..300).contains(&status))
            && p["content_hash"]
                .as_str()
                .is_some_and(commonmeasure_harness::session::observations::valid_hash)
            && p["timestamp"]
                .as_str()
                .and_then(normalise_timestamp)
                .is_some()
            && p["internal"] != true
            && p["url"]
                .as_str()
                .is_some_and(|url| projectable(url, internal_prefixes))
            && may_project(position))
        .then_some(position)
    })
}

fn observed_context(record: &Value) -> bool {
    let p = &record["payload"];
    record["event"] == "context_entered"
        && p["observer"] == "host"
        && p["grade"] == "observed"
        && p["representation_hash"]
            .as_str()
            .is_some_and(commonmeasure_harness::session::observations::valid_hash)
        && p["generation_id"]
            .as_str()
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
        && p["timestamp"]
            .as_str()
            .and_then(normalise_timestamp)
            .is_some()
}

/// Count explicit output markers separately from context entry. A completion
/// without an output observation retains unknown use rather than gaining zero.
fn output_associations(
    records: &[Value],
    boundary_position: usize,
    internal_prefixes: &[String],
    may_project: &dyn Fn(usize) -> bool,
) -> Option<u64> {
    let boundary = &records[boundary_position]["payload"];
    let output_id = boundary["output_id"].as_str()?;
    let (output_position, output) =
        records[..boundary_position]
            .iter()
            .enumerate()
            .find(|(_, record)| {
                let p = &record["payload"];
                record["event"] == "output_associated"
                    && p["output_id"] == output_id
                    && p["generation_id"] == boundary["turn_id"]
                    && p["host"] == boundary["host"]
                    && p["observer"] == "host"
                    && p["grade"] == "observed"
                    && p["output_hash"]
                        .as_str()
                        .is_some_and(commonmeasure_harness::session::observations::valid_hash)
            })?;
    let handles = output["payload"]["acquisition_ids"].as_array()?;
    Some(
        handles
            .iter()
            .filter(|handle| {
                let Some(acquisition) =
                    acquisition_position(records, handle, internal_prefixes, may_project)
                else {
                    return false;
                };
                if acquisition >= output_position {
                    return false;
                }
                records[acquisition + 1..output_position]
                    .iter()
                    .any(|record| {
                        observed_context(record)
                            && record["payload"]["acquisition_id"] == **handle
                            && record["payload"]["generation_id"] == boundary["turn_id"]
                    })
            })
            .count() as u64,
    )
}

/// Project one session evidence log. Returns no batch when nothing in the log
/// is eligible, which is the correct projection of a session that only ever
/// searched, was refused, was internal-only, or is known only from a
/// transcript. `internal_prefixes` is the operator's `record_internal_prefixes`
/// list; crossings matching it are operator-record only and never projected.
///
/// `may_project` is asked about turn boundaries and crossings by
/// its position in `records`, and it is a filter rather than a shorter list on purpose: the
/// event ids below are derived from that position, and the relay treats them
/// as delivery identity across runs. Hand this function a compacted list and
/// the same crossing changes id whenever anything before it is excluded — an
/// already-delivered event redelivers under a new id, and a newly included one
/// inherits an id already burned and is silently never sent. `records` must
/// always be the whole log, exactly as it was read.
///
/// `receiver` is the base URL the batches are projected for, where the caller
/// has one. It decides one thing: a content event carries its source record's
/// `instance` reference only where this receiver is the reference's issuer
/// (`wire_instance`). Event ids, selection and every other member are the
/// same for every receiver.
pub fn project_session(
    receiver: Option<&str>,
    session_id: &str,
    records: &[Value],
    internal_prefixes: &[String],
    may_project: &dyn Fn(usize) -> bool,
) -> SessionProjection {
    let mut events = Vec::new();
    let mut event_positions = Vec::new();
    let receiver = receiver_origin(receiver);
    // Refusals cross as a count and nothing else: a refused crossing has no
    // event, and the position filter is the same clearance the witnessed
    // crossings meet, so a refusal in work the operator kept home is not
    // counted either.
    let refused = records
        .iter()
        .enumerate()
        .filter(|(position, record)| is_refused(record) && may_project(*position))
        .count() as u64;
    // The agent identifier is the enrolled edge's key id, read from the
    // session's own `edge_identity` record: the identity that made these
    // requests, whatever the edge's standing by the time the relay runs. A
    // session recorded before this edge enrolled carries none, and projects
    // under the emitter id rather than borrowing a key it did not hold.
    let agent_id = records
        .iter()
        .find(|record| record["event"] == json!("edge_identity"))
        .and_then(|record| record["payload"]["key_id"].as_str())
        .unwrap_or(EMITTER_ID);
    let host_tool = records
        .iter()
        .find(|record| is_witnessed(record))
        .and_then(|record| record["payload"]["host"].as_str())
        .map(str::to_owned);
    for (position, record) in records.iter().enumerate() {
        if !may_project(position) {
            continue;
        }
        let payload = &record["payload"];
        if is_turn_boundary(record) {
            let Some(timestamp) = payload["timestamp"].as_str().and_then(normalise_timestamp)
            else {
                continue;
            };
            // Older boundaries without a declaration cannot establish the
            // privacy claim. Never borrow a neighbouring record's level.
            if payload["privacy_level"] != "minimal" {
                continue;
            }
            let id = derived_id(&json!({
                "kind": "event", "scope": "session", "session": session_id,
                "position": position, "event": record["event"],
            }));
            event_positions.push((id, position));
            let mut data = host_tool_data(payload["host"].as_str());
            if record["event"] == "turn_completed"
                && let Some(count) =
                    output_associations(records, position, internal_prefixes, may_project)
            {
                data.insert(
                    "commonmeasure-output-associations".to_owned(),
                    json!({"count": count}),
                );
            }
            events.push(WireEvent {
                id,
                kind: if record["event"] == "turn_started" {
                    WireEventKind::TurnStarted
                } else {
                    WireEventKind::TurnCompleted
                },
                timestamp,
                source_role: "agent".to_owned(),
                content_telemetry_id: None,
                content_url: String::new(),
                turn_id: payload["turn_id"].as_str().map(str::to_owned),
                turn: Some(WireTurn {
                    privacy_level: TurnPrivacy::Minimal,
                }),
                license_ref: None,
                // A boundary identifies no source and answers no duty.
                instance: None,
                data,
            });
            continue;
        }
        if !is_witnessed(record) {
            continue;
        }
        let (Some(url), Some(raw_timestamp)) =
            (payload["url"].as_str(), payload["timestamp"].as_str())
        else {
            continue;
        };
        let Some(timestamp) = normalise_timestamp(raw_timestamp) else {
            continue;
        };
        // The capture-time classification outlives the configuration that
        // produced it: a crossing recorded as internal supply never leaves,
        // whatever the prefix list says by the time the relay runs. The
        // current-list check below still covers records from before the
        // marker existed.
        if payload["internal"] == json!(true) {
            continue;
        }
        if !projectable(url, internal_prefixes) {
            continue;
        }
        // A mediated fetch the origin answered outside 2xx, or that no
        // origin answered, retrieved nothing: the request left the machine
        // and is on the private record, and it is never reported to an
        // owner as a retrieval. A record without a status is a search result
        // or an older record, and projects as before.
        if payload["failure"].is_string()
            || payload["http_status"]
                .as_u64()
                .is_some_and(|status| !(200..300).contains(&status))
        {
            continue;
        }
        let content_url = wire_url(url);
        // The supplier that served a search result, recorded on the crossing
        // by the mediated search; a fetch carries none.
        let supplier = payload["supplier"].as_str();

        let retrieved = derived_id(&json!({
            "kind": "event", "scope": "session", "session": session_id,
            "position": position, "event": "retrieved",
        }));
        event_positions.push((retrieved, position));
        events.push(WireEvent {
            id: retrieved,
            kind: WireEventKind::ContentRetrieved,
            timestamp: timestamp.clone(),
            source_role: "agent".to_owned(),
            content_telemetry_id: payload["content_telemetry_id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok()),
            content_url: content_url.clone(),
            turn_id: None,
            turn: None,
            license_ref: license_ref(payload),
            instance: wire_instance(payload, receiver.as_ref()),
            data: with_supplier(host_tool_data(host_tool.as_deref()), supplier),
        });

        if payload["context_observation"] == "host_required" {
            for (observation_position, observation) in records.iter().enumerate().skip(position + 1)
            {
                let observed = &observation["payload"];
                if observed["acquisition_id"] != payload["acquisition_id"]
                    || !observed_context(observation)
                    || acquisition_position(
                        records,
                        &observed["acquisition_id"],
                        internal_prefixes,
                        may_project,
                    ) != Some(position)
                {
                    continue;
                }
                let id = derived_id(&json!({
                    "kind": "event", "scope": "session", "session": session_id,
                    "position": observation_position, "event": "context_entered",
                }));
                let mut data = with_supplier(host_tool_data(observed["host"].as_str()), supplier);
                data.insert("scope".to_owned(), json!("turn"));
                data.insert(
                    "content_hash".to_owned(),
                    observed["representation_hash"].clone(),
                );
                event_positions.push((id, position));
                events.push(WireEvent {
                    id,
                    kind: WireEventKind::ContentGrounded,
                    timestamp: normalise_timestamp(
                        observed["timestamp"].as_str().expect("validated timestamp"),
                    )
                    .expect("validated timestamp"),
                    source_role: "agent".to_owned(),
                    content_telemetry_id: None,
                    content_url: content_url.clone(),
                    turn_id: observed["generation_id"].as_str().map(str::to_owned),
                    turn: None,
                    license_ref: license_ref(payload),
                    // The observation is this event's source record, and it
                    // may have been written under a later revision than the
                    // acquisition it refers to.
                    instance: wire_instance(observed, receiver.as_ref()),
                    data,
                });
            }
        } else if payload["grounded"] == json!(true) {
            let mut data = with_supplier(host_tool_data(host_tool.as_deref()), supplier);
            data.insert("scope".to_owned(), json!("session"));
            // No `provenance` and no `cached`. The hook witnesses the
            // crossing, not the host's delivery path: a host such as Claude
            // Code serves repeat fetches from its own cache (WebFetch, fifteen
            // minutes) without telling the hook, so `agent_fetched, cached:
            // false` would be wiring, not observation. The standard makes
            // `provenance` optional for exactly this case and forbids a
            // consumer inferring a value from its absence (section 6.4), and
            // the pairing rule (section 5.7.5) is satisfied by omitting both.
            // Absence is the honest emission until the host exposes a cache
            // marker the hook can read.
            if let Some(hash) = payload["content_hash"].as_str() {
                data.insert("content_hash".to_owned(), json!(hash));
            }
            ingestion(
                &mut data,
                &payload["estimated_tokens"],
                &payload["token_basis"],
            );
            let grounded = derived_id(&json!({
                "kind": "event", "scope": "session", "session": session_id,
                "position": position, "event": "grounded",
            }));
            event_positions.push((grounded, position));
            events.push(WireEvent {
                id: grounded,
                kind: WireEventKind::ContentGrounded,
                timestamp,
                source_role: "agent".to_owned(),
                content_telemetry_id: None,
                content_url,
                turn_id: None,
                turn: None,
                license_ref: license_ref(payload),
                instance: wire_instance(payload, receiver.as_ref()),
                data,
            });
        }
    }
    // Boundaries accompany selected source activity; they must not disclose
    // an otherwise internal-only, refused-only or reconstructed session.
    if !events
        .iter()
        .any(|event| event.kind == WireEventKind::ContentRetrieved)
    {
        events.clear();
        event_positions.clear();
    }
    SessionProjection {
        batches: into_batches(session_uuid(session_id), agent_id, events, Some(refused)),
        event_positions,
        refused,
    }
}

/// The host tool the crossings were witnessed through, in the extension
/// container the standard reserves for it (section 5.1.3, namespaced).
///
/// It is the operator's own tooling, not an identity a content owner can act
/// on: an owner matching a report against its logs needs the key id that
/// signed the requests, which is the agent identifier. It stays on the wire
/// because the fleet view groups by it, and the operator reading that view is
/// the one who chose the tool.
fn host_tool_data(host_tool: Option<&str>) -> Map<String, Value> {
    let mut data = projection_data();
    if let Some(host_tool) = host_tool {
        data.insert(HOST_TOOL_FIELD.to_owned(), json!(host_tool));
    }
    data
}

/// The supplier name as it may appear on the wire, or `None` for a provider
/// that is not a supplier: the operator's internal corpus and every
/// catalogued skill stay home (see [`SUPPLIER_FIELD`]).
fn wire_supplier(provider: Option<&str>) -> Option<&str> {
    provider.filter(|name| {
        *name != "internal"
            && !name.starts_with(commonmeasure_supply::SKILL_PROVIDER_PREFIX)
            && !name.is_empty()
    })
}

/// `data` with the supplier field added where the provider is a supplier.
fn with_supplier(mut data: Map<String, Value>, supplier: Option<&str>) -> Map<String, Value> {
    if let Some(supplier) = wire_supplier(supplier) {
        data.insert(SUPPLIER_FIELD.to_owned(), json!(supplier));
    }
    data
}

/// Project one published run from its `summary.json`. The run is its own
/// session on the wire: its id is already a UUID and its sources were admitted
/// into one assembled context. Sources the run rejected are supply
/// alternatives and stay home, and admitted sources cross the same egress
/// floor as session crossings: private addresses and named internal prefixes
/// — an internal corpus adapter admits `file://` documents — do not leave.
///
/// The agent identifier is [`EMITTER_ID`] whatever key the edge holds. A run
/// acquires its sources through the supply adapters, which reach a supplier's
/// API under the operator's own credential and sign nothing, so there is no
/// signed request for an owner to match a key id against. A run's summary
/// carries no `instance` reference, so its events carry none.
pub fn project_run(summary: &Value, internal_prefixes: &[String]) -> Result<Vec<WireBatch>> {
    let run_id = summary["run"]["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .context("summary.json carries no run id")?;
    let started_at = summary["run"]["started_at"]
        .as_str()
        .and_then(normalise_timestamp)
        .context("summary.json carries no started_at")?;

    let mut events = Vec::new();
    for plan in summary["plans"].as_array().into_iter().flatten() {
        let plan_id = plan["id"].as_str().unwrap_or("unnamed-plan");
        // A run's sources were acquired through one provider per plan; the
        // supplier is that provider where it is one (`wire_supplier`).
        let supplier = plan["provider"].as_str();
        for source in plan["sources"].as_array().into_iter().flatten() {
            if source["admitted"] != json!(true) {
                continue;
            }
            let Some(url) = source["url"].as_str() else {
                continue;
            };
            if !projectable(url, internal_prefixes) {
                continue;
            }
            let rank = source["retrieval_rank"].as_u64().unwrap_or(0);
            let content_url = wire_url(url);
            events.push(WireEvent {
                id: derived_id(&json!({
                    "kind": "event", "scope": "run", "run": run_id,
                    "plan": plan_id, "rank": rank, "event": "retrieved",
                })),
                kind: WireEventKind::ContentRetrieved,
                timestamp: started_at.clone(),
                source_role: "agent".to_owned(),
                content_telemetry_id: None,
                content_url: content_url.clone(),
                turn_id: None,
                turn: None,
                license_ref: license_ref(source),
                instance: None,
                data: with_supplier(projection_data(), supplier),
            });
            if let Some(hash) = source["content_hash"].as_str() {
                let mut data = with_supplier(projection_data(), supplier);
                data.insert("scope".to_owned(), json!("session"));
                data.insert("content_hash".to_owned(), json!(hash));
                ingestion(&mut data, &source["tokens"], &summary["run"]["token_basis"]);
                events.push(WireEvent {
                    id: derived_id(&json!({
                        "kind": "event", "scope": "run", "run": run_id,
                        "plan": plan_id, "rank": rank, "event": "grounded",
                    })),
                    kind: WireEventKind::ContentGrounded,
                    timestamp: started_at.clone(),
                    source_role: "agent".to_owned(),
                    content_telemetry_id: None,
                    content_url,
                    turn_id: None,
                    turn: None,
                    license_ref: license_ref(source),
                    instance: None,
                    data,
                });
            }
        }
    }
    Ok(into_batches(run_id, EMITTER_ID, events, None))
}

/// Chunk into standard-conformant batches sharing one session envelope. The
/// envelope's `started_at` is the earliest event in the projection, which for
/// a run is the run's own start. `refused` is the session's count, repeated
/// on every batch of it, or `None` for a run.
fn into_batches(
    session_id: Uuid,
    agent_id: &str,
    events: Vec<WireEvent>,
    refused: Option<u64>,
) -> Vec<WireBatch> {
    if events.is_empty() {
        return Vec::new();
    }
    let started_at = events
        .iter()
        .map(|event| event.timestamp.as_str())
        .min()
        .expect("a non-empty projection has a first event")
        .to_owned();
    let mut batches = Vec::new();
    let mut remaining = events;
    while !remaining.is_empty() {
        let tail = remaining.split_off(remaining.len().min(MAX_EVENTS_PER_BATCH));
        let mut batch = WireBatch::new(session_id, agent_id, &started_at);
        batch.refused = refused;
        batch.events = remaining;
        batches.push(batch);
        remaining = tail;
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crossing(event: &str, url: &str, grounded: bool) -> Value {
        json!({
            "event": event,
            "payload": {
                "session_id": "s",
                "timestamp": "2026-08-02T10:00:00.000Z",
                "mode": "observed",
                "host": "claude-code",
                "url": url,
                "host_name": "example.com",
                "grounded": grounded,
                "licence": {"state": "unknown"},
            },
        })
    }

    fn host_observation_session() -> Vec<Value> {
        let handle = "d7397a14-13eb-49a0-b4cf-28b9f9694485";
        let unused = "8cdcf073-1c8a-4b34-81ca-d0a532d7c365";
        let generation = "006ecf11-e015-444c-aeed-1a3682da7508";
        let output = "d7d18e30-7dde-45e3-8a1a-e38c0ec98d7c";
        let mut acquisition =
            crossing("crossing_mediated", "https://publisher.example/used", false);
        acquisition["payload"]["acquisition_id"] = json!(handle);
        acquisition["payload"]["context_observation"] = json!("host_required");
        acquisition["payload"]["http_status"] = json!(200);
        acquisition["payload"]["content_hash"] = json!(format!("sha256:{}", "a".repeat(64)));
        let mut unused_acquisition = acquisition.clone();
        unused_acquisition["payload"]["acquisition_id"] = json!(unused);
        unused_acquisition["payload"]["url"] = json!("https://publisher.example/unused");
        let entry = json!({"event":"context_entered", "payload":{
            "host":"pi", "observer":"host", "grade":"observed", "timestamp":"2026-09-16T10:00:01Z",
            "acquisition_id":handle, "generation_id":generation,
            "representation_hash":format!("sha256:{}", "b".repeat(64))}});
        let associated = json!({"event":"output_associated", "payload":{
            "host":"pi", "observer":"host", "grade":"observed", "timestamp":"2026-09-16T10:00:02Z",
            "generation_id":generation, "output_id":output, "output_hash":format!("sha256:{}", "c".repeat(64)),
            "acquisition_ids":[handle, handle, unused, "a9d0b2ab-e2ce-465f-ad2a-e87e36490e97"]}});
        let completed = json!({"event":"turn_completed", "payload":{
            "host":"pi", "timestamp":"2026-09-16T10:00:02Z", "privacy_level":"minimal",
            "turn_id":generation, "output_id":output, "detail":{"cwd":"/private/project"}}});
        vec![
            acquisition,
            unused_acquisition,
            entry,
            associated,
            completed,
            crossing("crossing_refused", "https://refused.example/secret", false),
        ]
    }

    #[test]
    fn explicit_context_and_output_counts_are_distinct_and_incrementally_stable() {
        let mut records = host_observation_session();
        let before_output = project_session(None, "s", &records[..3], &[], &|_| true);
        let projected = project_session(None, "s", &records, &[], &|_| true);
        let events = &projected.batches[0].events;
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == WireEventKind::ContentRetrieved)
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == WireEventKind::ContentGrounded)
                .count(),
            1
        );
        let ground = events
            .iter()
            .find(|e| e.kind == WireEventKind::ContentGrounded)
            .unwrap();
        assert_eq!(
            ground.data["content_hash"],
            records[2]["payload"]["representation_hash"]
        );
        assert!(!ground.data.contains_key("tokens_ingested"));
        assert_eq!(
            projected
                .event_positions
                .iter()
                .find(|(id, _)| id == &ground.id)
                .unwrap()
                .1,
            0
        );
        let completed = events
            .iter()
            .find(|e| e.kind == WireEventKind::TurnCompleted)
            .unwrap();
        assert_eq!(
            completed.data["commonmeasure-output-associations"]["count"],
            2
        );
        for previous in &before_output.batches[0].events {
            assert_eq!(events.iter().find(|e| e.id == previous.id), Some(previous));
        }
        assert_eq!(projected.refused, 1);
        let wire = serde_json::to_value(&projected.batches[0]).unwrap();
        let text = wire.to_string();
        for private in [
            "output_hash",
            "output_id",
            "acquisition_id",
            "/private/project",
            "refused.example",
        ] {
            assert!(!text.contains(private), "{private}");
        }
        let session_schema: Value =
            serde_json::from_str(include_str!("../../../schema/telemetry-session.v1.json"))
                .unwrap();
        let batch_schema: Value = serde_json::from_str(include_str!(
            "../../../schema/telemetry-event-batch.v1.json"
        ))
        .unwrap();
        let validator = jsonschema::JSONSchema::options()
            .with_document(
                session_schema["$id"].as_str().unwrap().to_owned(),
                session_schema.clone(),
            )
            .should_validate_formats(true)
            .compile(&batch_schema)
            .unwrap();
        if let Err(errors) = validator.validate(&wire) {
            panic!(
                "{}",
                errors
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }

        // History reuse adds a grounding occurrence, never another acquisition.
        let mut reuse = records[2].clone();
        reuse["payload"]["generation_id"] = json!(Uuid::new_v4());
        records.push(reuse);
        let projected = project_session(None, "s", &records, &[], &|_| true);
        assert_eq!(
            projected.batches[0]
                .events
                .iter()
                .filter(|e| e.kind == WireEventKind::ContentGrounded)
                .count(),
            2
        );
        assert_eq!(
            projected.batches[0]
                .events
                .iter()
                .filter(|e| e.kind == WireEventKind::ContentRetrieved)
                .count(),
            2
        );
    }

    #[test]
    fn grounding_preserves_multiple_representations_of_one_source_in_a_generation() {
        let mut records = host_observation_session();
        let mut transformed = records[2].clone();
        transformed["payload"]["representation_hash"] = json!(format!("sha256:{}", "d".repeat(64)));
        records.insert(3, transformed);
        let projected = project_session(None, "s", &records, &[], &|_| true);
        let events = &projected.batches[0].events;
        let grounded: Vec<_> = events
            .iter()
            .filter(|event| event.kind == WireEventKind::ContentGrounded)
            .collect();
        assert_eq!(grounded.len(), 2);
        assert_ne!(grounded[0].id, grounded[1].id);
        assert_ne!(
            grounded[0].data["content_hash"],
            grounded[1].data["content_hash"]
        );
        assert_eq!(grounded[0].turn_id, grounded[1].turn_id);
        assert_eq!(grounded[0].content_url, grounded[1].content_url);
        assert_eq!(
            events
                .iter()
                .find(|event| event.kind == WireEventKind::TurnCompleted)
                .unwrap()
                .data["commonmeasure-output-associations"]["count"],
            2
        );
    }

    #[test]
    fn host_observations_inherit_source_privacy_and_completion_needs_own_clearance() {
        for exclusion in [
            "uncleared",
            "internal",
            "prefix",
            "loopback",
            "refused",
            "failed",
            "missing",
        ] {
            let mut records = host_observation_session();
            let mut prefixes = vec![];
            match exclusion {
                "internal" => records[0]["payload"]["internal"] = json!(true),
                "prefix" => prefixes.push("https://publisher.example/used".to_owned()),
                "loopback" => records[0]["payload"]["url"] = json!("http://127.0.0.1/source"),
                "refused" => records[0]["event"] = json!("crossing_refused"),
                "failed" => records[0]["payload"]["failure"] = json!("unavailable"),
                "missing" => records[2]["payload"]["acquisition_id"] = json!(Uuid::new_v4()),
                _ => {}
            }
            let p = project_session(None, "s", &records, &prefixes, &|position| {
                exclusion != "uncleared" || position != 0
            });
            let events = &p.batches[0].events;
            assert!(
                !events
                    .iter()
                    .any(|e| e.kind == WireEventKind::ContentGrounded),
                "{exclusion}"
            );
            assert_eq!(
                events
                    .iter()
                    .find(|e| e.kind == WireEventKind::TurnCompleted)
                    .unwrap()
                    .data["commonmeasure-output-associations"]["count"],
                0,
                "{exclusion}"
            );
        }
        let records = host_observation_session();
        let p = project_session(None, "s", &records, &[], &|position| position != 4);
        assert!(
            !p.batches[0]
                .events
                .iter()
                .any(|e| e.kind == WireEventKind::TurnCompleted)
        );
        let mut no_output = records.clone();
        no_output[3]["event"] = json!("observation_unavailable");
        let p = project_session(None, "s", &no_output, &[], &|_| true);
        let boundary = p.batches[0]
            .events
            .iter()
            .find(|e| e.kind == WireEventKind::TurnCompleted)
            .unwrap();
        assert!(
            !boundary
                .data
                .contains_key("commonmeasure-output-associations")
        );
    }

    /// Which agent identifier each projection carries, and why the two differ.
    ///
    /// Catches: a run borrowing the enrolled key id, which would offer an
    /// owner a match against a signed request that was never made — a run's
    /// sources come from the supply adapters, which sign nothing — and a
    /// session losing the key id, which would withhold the match an owner is
    /// meant to be able to make.
    #[test]
    fn a_session_carries_the_enrolled_key_id_and_a_run_carries_the_emitter_id() {
        let enrolled = json!({
            "event": "edge_identity",
            "payload": {"key_id": "key-1", "standing": "enrolled", "hub": "https://hub.example"},
        });
        let fetch = crossing("crossing_mediated", "https://a.example/x", true);

        let batches =
            project_session(None, "s", &[enrolled, fetch.clone()], &[], &|_| true).batches;
        assert_eq!(batches[0].agent_id, "key-1");

        // The same crossings from an edge that holds no key.
        let batches = project_session(None, "s", &[fetch], &[], &|_| true).batches;
        assert_eq!(batches[0].agent_id, EMITTER_ID);

        // A run, whatever the edge holds: its sources were acquired by a
        // supply adapter under the operator's credential, not by a signed
        // fetch, so no key id can be matched to them.
        let summary = json!({
            "run": {"id": "6e0f9b3a-4c1d-4f2e-8a5b-9d7c2e1f0a3b",
                    "started_at": "2026-08-20T10:00:00Z"},
            "plans": [{"id": "p", "sources": [
                {"admitted": true, "url": "https://a.example/x", "retrieval_rank": 1}]}],
        });
        let batches = project_run(&summary, &[]).expect("project run");
        assert_eq!(batches[0].agent_id, EMITTER_ID);
    }

    #[test]
    fn a_grounded_crossing_projects_retrieval_and_grounding() {
        let batches = project_session(
            None,
            "s",
            &[crossing("crossing_observed", "https://a.example/x", true)],
            &[],
            &|_| true,
        )
        .batches;
        assert_eq!(batches.len(), 1);
        let kinds: Vec<WireEventKind> = batches[0].events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                WireEventKind::ContentRetrieved,
                WireEventKind::ContentGrounded
            ]
        );
    }

    #[test]
    fn projection_is_deterministic_across_calls() {
        let records = [crossing("crossing_mediated", "https://a.example/x", true)];
        let first = project_session(None, "s", &records, &[], &|_| true).batches;
        let second = project_session(None, "s", &records, &[], &|_| true).batches;
        assert_eq!(first, second);
    }

    /// The privacy floor holds on egress with no exceptions: a crossing to a
    /// private address produces no wire event, whatever let it into the
    /// record. The probe set is the one the 2 Aug gate delivered to a live
    /// receiver through the missing filter.
    #[test]
    fn private_addresses_produce_no_wire_event() {
        for private in [
            "http://[::1]:8080/admin",
            "http://127.0.0.2/secret",
            "http://0.0.0.0/x",
            "http://localhost:3000/x",
            "file:///home/user/notes.md",
        ] {
            assert!(
                project_session(
                    None,
                    "s",
                    &[crossing("crossing_observed", private, true)],
                    &[],
                    &|_| true
                )
                .batches
                .is_empty(),
                "{private} must not be projected"
            );
        }
    }

    /// A named internal prefix is consent to record, never consent to send:
    /// the same list that lowers the floor at capture excludes on egress, and
    /// public traffic beside it still crosses. The prefix is a public-looking
    /// domain deliberately — a `.internal` host would be dropped by the
    /// address floor anyway, and this test is about the prefix path.
    #[test]
    fn named_internal_prefixes_are_operator_record_only() {
        let prefixes = vec!["https://intranet.example.com/private/".to_owned()];
        let records = [
            crossing(
                "crossing_observed",
                "https://intranet.example.com/private/handbook",
                true,
            ),
            crossing("crossing_observed", "https://www.gov.uk/x", true),
        ];
        let batches = project_session(None, "s", &records, &prefixes, &|_| true).batches;
        assert_eq!(batches.len(), 1);
        assert!(
            batches[0]
                .events
                .iter()
                .all(|event| event.content_url == "https://www.gov.uk/x"),
            "only the public crossing may leave"
        );
    }

    /// The classification recorded at capture outlives the prefix list that
    /// produced it: a crossing marked internal projects nothing even after
    /// the operator removes the prefix, although its URL is otherwise public.
    #[test]
    fn a_recorded_internal_marking_outlives_the_prefix_list() {
        let mut record = crossing(
            "crossing_observed",
            "https://intranet.example.com/private/handbook",
            true,
        );
        record["payload"]["internal"] = json!(true);
        assert!(
            project_session(None, "s", &[record], &[], &|_| true)
                .batches
                .is_empty(),
            "operator-record-only is a recorded fact, not a re-read of policy.json"
        );
    }

    /// A credential the operator embedded in a fetched URL is authentication,
    /// not content identity: it is stripped before the URL can reach the spool
    /// or the wire, while the rest of the URL — query string included — is
    /// projected exactly as witnessed.
    #[test]
    fn embedded_credentials_are_stripped_from_the_projected_url() {
        let batches = project_session(
            None,
            "s",
            &[crossing(
                "crossing_observed",
                "https://user:hunter2@api.example.com/v1/data?api_key=sk-live-SECRET123",
                true,
            )],
            &[],
            &|_| true,
        )
        .batches;
        let urls: Vec<&str> = batches[0]
            .events
            .iter()
            .map(|event| event.content_url.as_str())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://api.example.com/v1/data?api_key=sk-live-SECRET123",
                "https://api.example.com/v1/data?api_key=sk-live-SECRET123"
            ]
        );
    }

    #[test]
    fn only_the_authority_is_touched_when_stripping_userinfo() {
        for (raw, expected) in [
            // A password-less userinfo, and an `@` that belongs to the path or
            // the query rather than the authority.
            ("https://token@a.example/x", "https://a.example/x"),
            ("https://a.example/u@b?q=c@d", "https://a.example/u@b?q=c@d"),
            ("https://a.example", "https://a.example"),
            (
                "https://u:p@a.example:8443/x#f@g",
                "https://a.example:8443/x#f@g",
            ),
        ] {
            assert_eq!(wire_url(raw), expected, "projecting {raw}");
        }
    }

    /// A fetch the origin refused, or that nothing answered, retrieved
    /// nothing and is never reported as a retrieval; a search result, which
    /// carries no status, still is.
    #[test]
    fn a_non_2xx_answer_or_a_transport_failure_is_never_projected_as_a_retrieval() {
        let mut forbidden = crossing("crossing_mediated", "https://a.example/paywalled", false);
        forbidden["payload"]["http_status"] = json!(403);
        let mut failed = crossing("crossing_mediated", "https://a.example/down", false);
        failed["payload"]["failure"] = json!("connect a.example: connection refused");
        let mut served = crossing("crossing_mediated", "https://a.example/served", true);
        served["payload"]["http_status"] = json!(200);
        let listed = crossing("crossing_mediated", "https://a.example/listed", false);
        let batches = project_session(
            None,
            "s",
            &[forbidden, failed, served, listed],
            &[],
            &|_| true,
        )
        .batches;
        let urls: Vec<&str> = batches[0]
            .events
            .iter()
            .map(|event| event.content_url.as_str())
            .collect();
        assert_eq!(
            urls,
            [
                "https://a.example/served",
                "https://a.example/served",
                "https://a.example/listed"
            ]
        );
    }

    /// The correlation id travels from the crossing to the retrieval event
    /// and to nothing else.
    #[test]
    fn the_content_telemetry_id_is_projected_on_the_retrieval_event_alone() {
        let mut record = crossing("crossing_mediated", "https://a.example/x", true);
        record["payload"]["http_status"] = json!(200);
        record["payload"]["content_telemetry_id"] = json!("550e8400-e29b-41d4-a716-446655440000");
        let batches = project_session(None, "s", &[record], &[], &|_| true).batches;
        let events = &batches[0].events;
        assert_eq!(events[0].kind, WireEventKind::ContentRetrieved);
        assert_eq!(
            events[0]
                .content_telemetry_id
                .map(|id| id.to_string())
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(events[1].kind, WireEventKind::ContentGrounded);
        assert!(events[1].content_telemetry_id.is_none());
    }

    /// The supplier that served a search result travels on both event kinds
    /// of that crossing and on nothing a fetch produced; a run's plan
    /// provider does the same for every source it admitted.
    #[test]
    fn a_supplied_crossing_names_its_supplier_and_a_fetch_names_none() {
        let mut supplied = crossing("crossing_mediated", "https://a.example/served", false);
        supplied["payload"]["supplier"] = json!("ozone");
        let mut fetched = crossing("crossing_mediated", "https://a.example/fetched", true);
        fetched["payload"]["http_status"] = json!(200);
        let batches = project_session(None, "s", &[supplied, fetched], &[], &|_| true).batches;
        let events = &batches[0].events;
        assert_eq!(events[0].content_url, "https://a.example/served");
        assert_eq!(events[0].data[SUPPLIER_FIELD], json!("ozone"));
        assert!(
            events[1..]
                .iter()
                .all(|event| !event.data.contains_key(SUPPLIER_FIELD)),
            "a fetch this runtime made has no supplier"
        );

        let summary = json!({
            "run": {"id": "6e0f9b3a-4c1d-4f2e-8a5b-9d7c2e1f0a3b",
                    "started_at": "2026-08-20T10:00:00Z"},
            "plans": [{"id": "ozone-only", "provider": "ozone", "capability": "search",
                       "sources": [{"admitted": true, "url": "https://a.example/x",
                                    "retrieval_rank": 1, "content_hash": "sha256:00"}]}],
        });
        let batches = project_run(&summary, &[]).expect("project run");
        let kinds: Vec<(WireEventKind, Value)> = batches[0]
            .events
            .iter()
            .map(|event| (event.kind, event.data[SUPPLIER_FIELD].clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (WireEventKind::ContentRetrieved, json!("ozone")),
                (WireEventKind::ContentGrounded, json!("ozone")),
            ]
        );
    }

    /// The operator's own corpus and a catalogued skill are not suppliers:
    /// their names stay home even where their sources project.
    #[test]
    fn the_internal_corpus_and_a_skill_are_never_named_as_suppliers() {
        for provider in ["internal", "skill:validate-brief", ""] {
            let mut supplied = crossing("crossing_mediated", "https://a.example/x", false);
            supplied["payload"]["supplier"] = json!(provider);
            let batches = project_session(None, "s", &[supplied], &[], &|_| true).batches;
            assert!(
                !batches[0].events[0].data.contains_key(SUPPLIER_FIELD),
                "{provider:?} must not be named on the wire"
            );
            let summary = json!({
                "run": {"id": "6e0f9b3a-4c1d-4f2e-8a5b-9d7c2e1f0a3b",
                        "started_at": "2026-08-20T10:00:00Z"},
                "plans": [{"id": "p", "provider": provider,
                           "sources": [{"admitted": true, "url": "https://a.example/x",
                                        "retrieval_rank": 1}]}],
            });
            let batches = project_run(&summary, &[]).expect("project run");
            assert!(!batches[0].events[0].data.contains_key(SUPPLIER_FIELD));
        }
    }

    fn registered(mut record: Value, issuer: &str, revision: i64) -> Value {
        record["payload"]["instance"] = json!({
            "issuer": issuer,
            "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b",
            "revision": revision,
        });
        record
    }

    /// The reference crosses on the content events of a registered record,
    /// to its issuer alone, at the revision the record was written under.
    ///
    /// Catches: the member on a session that never registered or on a turn
    /// boundary; a renewal rewriting the revision of earlier events; and the
    /// reference reaching a receiver that did not issue it, which is how it
    /// would reach a publisher.
    #[test]
    fn the_instance_reference_crosses_on_content_events_to_its_issuer_alone() {
        let hub = "https://hub.example";
        let boundary = json!({"event": "turn_started", "payload": {
            "host": "claude-code", "timestamp": "2026-08-02T09:59:00Z",
            "privacy_level": "minimal", "turn_id": "t1"}});
        let records = [
            crossing("crossing_mediated", "https://a.example/before", true),
            registered(boundary, hub, 1),
            registered(
                crossing("crossing_mediated", "https://a.example/first", true),
                hub,
                1,
            ),
            // A renewal between two records: the later one holds revision 2.
            registered(
                crossing("crossing_observed", "https://a.example/renewed", false),
                "https://hub.example/",
                2,
            ),
        ];
        let members = |receiver: Option<&str>| -> Vec<(WireEventKind, String, Option<i64>)> {
            project_session(receiver, "s", &records, &[], &|_| true).batches[0]
                .events
                .iter()
                .map(|event| {
                    (
                        event.kind,
                        event.content_url.clone(),
                        event.instance.as_ref().map(|instance| instance.revision),
                    )
                })
                .collect()
        };
        use WireEventKind::{ContentGrounded, ContentRetrieved, TurnStarted};
        let before = "https://a.example/before".to_owned();
        let first = "https://a.example/first".to_owned();
        let renewed = "https://a.example/renewed".to_owned();
        assert_eq!(
            members(Some("https://hub.example/api/v1/telemetry")),
            vec![
                (ContentRetrieved, before.clone(), None),
                (ContentGrounded, before, None),
                (TurnStarted, String::new(), None),
                (ContentRetrieved, first.clone(), Some(1)),
                (ContentGrounded, first, Some(1)),
                (ContentRetrieved, renewed, Some(2)),
            ]
        );
        let projected = project_session(Some(hub), "s", &records, &[], &|_| true);
        let wire = serde_json::to_value(&projected.batches[0]).unwrap();
        assert_eq!(
            wire["events"][5]["instance"],
            json!({"issuer": hub, "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b", "revision": 2}),
            "the issuer crosses without its trailing slash"
        );
        assert!(wire["events"][0].get("instance").is_none());

        // Same host on another scheme or port, another host, and no receiver.
        for other in [
            Some("http://hub.example/api/v1/telemetry"),
            Some("https://hub.example:8443"),
            Some("https://receiver.example"),
            Some("not a url"),
            None,
        ] {
            assert!(
                members(other)
                    .iter()
                    .all(|(_, _, revision)| revision.is_none()),
                "{other:?} did not issue the instance"
            );
            // The member is the only thing the receiver decides.
            let mut elsewhere = project_session(other, "s", &records, &[], &|_| true);
            let mut at_hub = project_session(Some(hub), "s", &records, &[], &|_| true);
            for batch in elsewhere.batches.iter_mut().chain(&mut at_hub.batches) {
                batch
                    .events
                    .iter_mut()
                    .for_each(|event| event.instance = None);
            }
            assert_eq!(elsewhere.batches, at_hub.batches);
        }
    }

    /// A reference that is not an issuer, an identifier and an integer
    /// revision projects no member and leaves the event in place.
    #[test]
    fn a_malformed_instance_reference_projects_no_member() {
        let hub = "https://hub.example";
        for reference in [
            json!({"issuer": hub, "id": "i", "revision": "2"}),
            json!({"issuer": hub, "id": "", "revision": 2}),
            json!({"issuer": hub, "revision": 2}),
            json!({"id": "i", "revision": 2}),
            json!({"issuer": "hub.example", "id": "i", "revision": 2}),
            json!("0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b"),
        ] {
            let mut record = crossing("crossing_mediated", "https://a.example/x", true);
            record["payload"]["instance"] = reference.clone();
            let batches = project_session(Some(hub), "s", &[record], &[], &|_| true).batches;
            assert_eq!(batches[0].events.len(), 2, "{reference}");
            assert!(
                batches[0]
                    .events
                    .iter()
                    .all(|event| event.instance.is_none()),
                "{reference}"
            );
        }
    }

    /// A host-observed grounding event takes the reference of the observation
    /// that is its source record, which a renewal may have moved past the
    /// acquisition's revision.
    #[test]
    fn a_context_entry_event_carries_its_own_records_reference() {
        let hub = "https://hub.example";
        let mut records = host_observation_session();
        records[0] = registered(records[0].clone(), hub, 1);
        records[2] = registered(records[2].clone(), hub, 2);
        let projected = project_session(Some(hub), "s", &records, &[], &|_| true);
        let revisions: Vec<(WireEventKind, Option<i64>)> = projected.batches[0]
            .events
            .iter()
            .map(|event| {
                (
                    event.kind,
                    event.instance.as_ref().map(|instance| instance.revision),
                )
            })
            .collect();
        assert_eq!(
            revisions,
            vec![
                (WireEventKind::ContentRetrieved, Some(1)),
                (WireEventKind::ContentGrounded, Some(2)),
                (WireEventKind::ContentRetrieved, None),
                (WireEventKind::TurnCompleted, None),
            ]
        );
    }

    #[test]
    fn oversized_projections_split_under_the_receiver_cap() {
        let records: Vec<Value> = (0..600)
            .map(|n| {
                crossing(
                    "crossing_observed",
                    &format!("https://a.example/{n}"),
                    false,
                )
            })
            .collect();
        let batches = project_session(None, "s", &records, &[], &|_| true).batches;
        assert_eq!(batches.len(), 2);
        assert!(
            batches
                .iter()
                .all(|batch| batch.events.len() <= MAX_EVENTS_PER_BATCH)
        );
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.events.len())
                .sum::<usize>(),
            600
        );
    }

    /// Refusals cross as a count on the session's batches and nothing else:
    /// no event carries a refused URL, the count honours the same clearance
    /// filter the crossings do, and a run's batch carries no count.
    #[test]
    fn refusals_cross_as_a_count_and_nothing_else() {
        let refused = |url: &str| {
            json!({"event": "crossing_refused", "payload": {
                "timestamp": "2026-08-02T10:01:00.000Z", "mode": "mediated",
                "host": "claude-code", "url": url, "grounded": false,
                "refusal": "access rule 1 (*) refuses host paywall.example.",
            }})
        };
        let records = [
            crossing("crossing_mediated", "https://a.example/x", true),
            refused("https://paywall.example/one"),
            refused("http://127.0.0.1:8080/private"),
            refused("https://paywall.example/three"),
        ];
        let projection = project_session(None, "s", &records, &[], &|_| true);
        assert_eq!(projection.refused, 3);
        assert_eq!(projection.batches.len(), 1);
        assert_eq!(projection.batches[0].refused, Some(3));
        let text = serde_json::to_string(&projection.batches[0]).unwrap();
        assert!(!text.contains("paywall.example"), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains("access rule"), "{text}");

        // The same clearance filter: a refusal at a position the caller did
        // not clear is not counted.
        let projection = project_session(None, "s", &records, &[], &|position| position != 3);
        assert_eq!(projection.refused, 2);
        assert_eq!(projection.batches[0].refused, Some(2));

        // Nothing admitted: no batch, so the count does not cross.
        let projection = project_session(None, "s", &records[1..], &[], &|_| true);
        assert_eq!(projection.refused, 3);
        assert!(projection.batches.is_empty());

        // A run has no refused crossings to count.
        let summary = json!({
            "run": {"id": "6e0f9b3a-4c1d-4f2e-8a5b-9d7c2e1f0a3b",
                    "started_at": "2026-08-20T10:00:00Z"},
            "plans": [{"id": "p", "sources": [
                {"admitted": true, "url": "https://a.example/x", "retrieval_rank": 1},
                {"admitted": false, "url": "https://b.example/y", "retrieval_rank": 2}]}],
        });
        let batches = project_run(&summary, &[]).expect("project run");
        assert_eq!(batches[0].refused, None);
    }

    /// The relay queues only documents whose `events` is an array. Anything
    /// else is left byte for byte as found, and the members removed from an
    /// array are counted.
    #[test]
    fn withholding_leaves_a_document_without_an_events_array_as_found() {
        let reference = json!({"issuer": "https://hub.example", "id": "i", "revision": 1});
        for found in [json!({}), json!({"events": {"a": {"instance": reference}}})] {
            let mut document = found.clone();
            assert_eq!(
                withhold_unissued_instances(&mut document, "https://other.example"),
                0
            );
            assert_eq!(document, found);
        }

        let mut document = json!({"events": [
            {"id": "1", "instance": reference},
            {"id": "2"},
            {"id": "3", "instance": reference},
        ]});
        assert_eq!(
            withhold_unissued_instances(&mut document, "https://hub.example/api/v1/telemetry"),
            0,
            "the issuer keeps the member"
        );
        assert_eq!(
            withhold_unissued_instances(&mut document, "https://other.example"),
            2
        );
        assert!(!document.to_string().contains("instance"));
    }
}
