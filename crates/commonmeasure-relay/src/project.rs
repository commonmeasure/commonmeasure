//! The purpose-limited projection from the evidence logs to the wire.
//!
//! What crosses: retrieval and grounding facts with observed evidence behind
//! them, from witnessed crossings (`observed`, `mediated`) in session logs and
//! from admitted sources in published runs. What stays home, by construction:
//! prompts, answers, evaluator output, refused crossings, sources the run
//! rejected, policy detail, every reconstructed crossing, and every crossing
//! to a private address or a named internal prefix. A transcript claim must
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

use crate::wire::{WireBatch, WireEvent, WireEventKind};

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
/// identifier in sealed evidence uses (`PRODUCT.md` §Naming); the hyphen
/// cannot collide with a standard field, which are all snake_case.
pub const HOST_TOOL_FIELD: &str = "contextops-host-tool";

/// The extension field naming the supplier that served an acquisition: the
/// provider name a plan and a mediated search use (`exa`, `ozone`). Present on
/// the retrieval and grounding events of a supplied source and on nothing
/// else. It takes the product's current name rather than the `contextops-`
/// namespace of [`HOST_TOOL_FIELD`]: that namespace is kept only for
/// identifiers already inside sealed evidence, and this field was born after
/// the rename (`PRODUCT.md` §Naming).
///
/// The supplier is on the wire so a content owner, or the network reporting
/// for it, can tell a retrieval served through a licensed supplier from one
/// fetched from the open web. The operator's own corpus (`internal`) and a
/// catalogued skill (`skill:<name>`) are not suppliers and are never named:
/// the first would disclose operator layout, the second a program that
/// published nothing (`wire_supplier`).
pub const SUPPLIER_FIELD: &str = "commonmeasure-supplier";

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

/// One session's projection: the batches to deliver, and where each of their
/// events came from.
pub struct SessionProjection {
    pub batches: Vec<WireBatch>,
    /// The log position each projected event was derived from, in projection
    /// order. A caller that resolved a per-crossing property — the relay
    /// resolves each crossing's governing engagement before asking
    /// `may_project` — can attribute the events back to it through this,
    /// instead of re-deriving an event id from the naming scheme above and
    /// owning a second copy of it.
    pub event_positions: Vec<(Uuid, usize)>,
}

/// Project one session evidence log. Returns no batch when nothing in the log
/// is eligible, which is the correct projection of a session that only ever
/// searched, was refused, was internal-only, or is known only from a
/// transcript. `internal_prefixes` is the operator's `record_internal_prefixes`
/// list; crossings matching it are operator-record only and never projected.
///
/// `may_project` is asked about each witnessed crossing by its position in
/// `records`, and it is a filter rather than a shorter list on purpose: the
/// event ids below are derived from that position, and the relay treats them
/// as delivery identity across runs. Hand this function a compacted list and
/// the same crossing changes id whenever anything before it is excluded — an
/// already-delivered event redelivers under a new id, and a newly included one
/// inherits an id already burned and is silently never sent. `records` must
/// always be the whole log, exactly as it was read.
pub fn project_session(
    session_id: &str,
    records: &[Value],
    internal_prefixes: &[String],
    may_project: &dyn Fn(usize) -> bool,
) -> SessionProjection {
    let mut events = Vec::new();
    let mut event_positions = Vec::new();
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
        if !is_witnessed(record) {
            continue;
        }
        if !may_project(position) {
            continue;
        }
        let payload = &record["payload"];
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
            license_ref: license_ref(payload),
            data: with_supplier(host_tool_data(host_tool.as_deref()), supplier),
        });

        if payload["grounded"] == json!(true) {
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
            if let Some(tokens) = payload["estimated_tokens"].as_u64() {
                data.insert("tokens_ingested".to_owned(), json!(tokens));
                // The number is an estimate; its basis travels with it, as it
                // does in the evidence log, so nobody downstream mistakes it for a
                // tokeniser's output.
                if let Some(basis) = payload["token_basis"].as_str() {
                    data.insert("token_basis".to_owned(), json!(basis));
                }
            }
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
                license_ref: license_ref(payload),
                data,
            });
        }
    }
    SessionProjection {
        batches: into_batches(session_uuid(session_id), agent_id, events),
        event_positions,
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
    let mut data = Map::new();
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
/// signed request for an owner to match a key id against.
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
                license_ref: license_ref(source),
                data: with_supplier(Map::new(), supplier),
            });
            if let Some(hash) = source["content_hash"].as_str() {
                let mut data = with_supplier(Map::new(), supplier);
                data.insert("scope".to_owned(), json!("session"));
                data.insert("content_hash".to_owned(), json!(hash));
                if let Some(tokens) = source["tokens"].as_u64() {
                    data.insert("tokens_ingested".to_owned(), json!(tokens));
                }
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
                    license_ref: license_ref(source),
                    data,
                });
            }
        }
    }
    Ok(into_batches(run_id, EMITTER_ID, events))
}

/// Chunk into standard-conformant batches sharing one session envelope. The
/// envelope's `started_at` is the earliest event in the projection, which for
/// a run is the run's own start.
fn into_batches(session_id: Uuid, agent_id: &str, events: Vec<WireEvent>) -> Vec<WireBatch> {
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

        let batches = project_session("s", &[enrolled, fetch.clone()], &[], &|_| true).batches;
        assert_eq!(batches[0].agent_id, "key-1");

        // The same crossings from an edge that holds no key.
        let batches = project_session("s", &[fetch], &[], &|_| true).batches;
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
        let first = project_session("s", &records, &[], &|_| true).batches;
        let second = project_session("s", &records, &[], &|_| true).batches;
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
            "file:///home/alex/notes.md",
        ] {
            assert!(
                project_session(
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
        let batches = project_session("s", &records, &prefixes, &|_| true).batches;
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
            project_session("s", &[record], &[], &|_| true)
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
        let batches =
            project_session("s", &[forbidden, failed, served, listed], &[], &|_| true).batches;
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
        let batches = project_session("s", &[record], &[], &|_| true).batches;
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
        let batches = project_session("s", &[supplied, fetched], &[], &|_| true).batches;
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
            let batches = project_session("s", &[supplied], &[], &|_| true).batches;
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
        let batches = project_session("s", &records, &[], &|_| true).batches;
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
}
