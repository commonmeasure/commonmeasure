//! The Content Telemetry v1.0 wire shapes this relay emits.
//!
//! These are projection types, not the operator record: a [`WireEvent`] carries
//! only what the standard defines and the Content Telemetry boundary in
//! `ARCHITECTURE.md` permits. The pinned schemas in `schema/` are the contract;
//! the conformance test in `tests/conformance.rs` keeps these types honest
//! against them, so the standard is reused verbatim rather than forked.

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
    pub content_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_ref: Option<String>,
    #[serde(skip_serializing_if = "Map::is_empty", default)]
    pub data: Map<String, Value>,
}

/// The two event types this projection emits. Retrieval and grounding are the
/// facts the evidence logs witness; citation, presentation and engagement
/// have no observed evidence behind them here, so there is no variant to emit
/// them with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireEventKind {
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
            events: Vec::new(),
        }
    }
}
