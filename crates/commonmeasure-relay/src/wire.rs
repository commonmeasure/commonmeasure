//! The Content Telemetry v1.0 wire shapes this relay emits.
//!
//! These are projection types, not the operator record: a [`WireEvent`] carries
//! what the standard defines, the Content Telemetry boundary in
//! `ARCHITECTURE.md` permits, and the custom members
//! `docs/contracts/telemetry-projection.md` §Custom fields lists. The pinned
//! schemas in `schema/` are the contract; the conformance test in
//! `tests/conformance.rs` keeps these types honest against them, so the
//! standard is reused verbatim rather than forked.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The one schema version this relay speaks, matching the pin in
/// `schema/SOURCE.md`.
pub const SCHEMA_VERSION: &str = "1.0";

/// An `event_batch` delivery document: events sharing one session context.
/// The standard requires events of different sessions to travel in separate
/// batches, and this type cannot express anything else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireBatch {
    pub document_type: String,
    pub schema_version: String,
    /// The projected session identifier, a UUID as the standard requires.
    pub session_id: Uuid,
    /// Responding agent identifier; required at Grounding conformance when
    /// delivering by batch.
    pub agent_id: String,
    /// Session start, mirrored on the envelope as the standard requires for
    /// batch delivery at Grounding conformance.
    pub started_at: String,
    /// How many crossings the operator's policy refused in this session,
    /// counted at projection over the refusals under scopes cleared for
    /// egress: an integer and nothing else about them, so an organisation
    /// can see policy enforced without any refused source leaving the
    /// machine. Present on every batch of a session, absent on a run's
    /// batches, where the run's own rejected sources are not crossings.
    /// A receiver keeps the highest value it has seen for a session rather
    /// than summing, so every batch of a session and every redelivery
    /// carry the same fact (`docs/contracts/telemetry-projection.md`
    /// §The refused count on the wire).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refused: Option<u64>,
    pub events: Vec<WireEvent>,
}

/// One projected event. Only the fields the projection actually carries are
/// present; everything else the standard defines is deliberately absent rather
/// than null-filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEvent {
    /// Client-supplied identity, derived deterministically from the evidence
    /// record this event projects, so redelivery after a crash carries the
    /// same id and the receiver's dedup makes it count once.
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: WireEventKind,
    pub timestamp: String,
    pub source_role: String,
    /// The `Content-Telemetry-ID` the fetch carried, on the retrieval event
    /// alone (standard section 7.2), so an owner that logged the header can
    /// match this report to its own line. Absent where the fetch carried
    /// none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_telemetry_id: Option<Uuid>,
    /// Absent on turn boundaries, which identify no source.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub content_url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub turn: Option<WireTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_ref: Option<String>,
    /// The registered instance the source record was written under, on a
    /// content event projected for the registration service that issued it
    /// and on nothing else. Not a standard member: the operator's hub reads
    /// it to join the event to a reporting duty and removes it before onward
    /// delivery (`docs/contracts/telemetry-projection.md` §Instance reference).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub instance: Option<WireInstance>,
    #[serde(skip_serializing_if = "Map::is_empty", default)]
    pub data: Map<String, Value>,
}

/// The `instance` member of a session record as it crosses to its issuer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireInstance {
    /// The registration service's origin, without a trailing slash.
    pub issuer: String,
    pub id: String,
    /// The binding revision held when the source record was written.
    pub revision: i64,
}

/// Turn data is restricted to a privacy declaration. Query and response text,
/// intent, summaries and private boundary detail cannot enter this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTurn {
    pub privacy_level: TurnPrivacy,
}

/// The projection carries no conversation content, so only minimal is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPrivacy {
    Minimal,
}

/// The Grounding-level event types this projection emits. Boundaries,
/// retrieval and grounding are witnessed; citation, presentation and engagement
/// have no observed evidence behind them here, so there is no variant to emit
/// them with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireEventKind {
    TurnStarted,
    TurnCompleted,
    ContentRetrieved,
    ContentGrounded,
}

impl WireBatch {
    pub fn new(session_id: Uuid, agent_id: &str, started_at: &str) -> Self {
        Self {
            document_type: "event_batch".to_owned(),
            schema_version: SCHEMA_VERSION.to_owned(),
            session_id,
            agent_id: agent_id.to_owned(),
            started_at: started_at.to_owned(),
            refused: None,
            events: Vec::new(),
        }
    }
}
