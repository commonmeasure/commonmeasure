//! Processors: replaceable units of work around a ContextJob.
//!
//! The in-process implementation of `docs/contracts/processor.md`. A processor
//! declares what it is, where it runs and what it may touch in a
//! [`ProcessorManifest`], and every invocation yields one [`Invocation`]
//! record: an append-only account of what crossed the stage, what the
//! processor decided and by what method, and what it cannot see. Scores and
//! findings stay inside the invocation record, named by the processor that
//! produced them — the runtime never lifts them onto one shared scale.
//!
//! These implementations run inside this binary. The optional Encypher
//! provenance route calls its fixed remote service only with a job's explicit
//! disclosure permission and operator credentials. The generic external
//! processor protocol and supervised sidecars remain planned.

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_types::{AssuranceBasis, Decision, Gap};
use serde::Serialize;
use serde_json::{Value, json};

pub mod correctness;
mod embedded;
pub mod extract;
pub mod fidelity;
pub mod injection;
pub mod judge;
pub mod optimise;
pub mod pii;
pub mod provenance;
pub mod support;

/// The invocation-record contract version. Any change to the record shape
/// changes this.
pub const INVOCATION_VERSION: &str = "contextops-processor-invocation/v1";

/// Where in the life of a ContextJob a processor runs
/// (`docs/contracts/processor.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Discover,
    Admit,
    Transform,
    Infer,
    Verify,
    Attest,
    Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    /// Same input, same output, no sampling and no clock in the decision.
    Deterministic,
    Seeded,
    NonDeterministic,
}

/// What happens when the processor itself cannot run to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailBehaviour {
    /// The crossing proceeds without the processor's judgement, with a gap.
    FailOpen,
    /// The crossing is refused: an unexamined input is not admitted.
    FailClosed,
}

/// Failure behaviour by policy mode. Declared per mode because the same
/// failure means different things to an operator who asked for enforcement
/// and one who asked only for a record.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FailureByMode {
    pub strict: FailBehaviour,
    pub prefer: FailBehaviour,
    pub observe: FailBehaviour,
}

/// What the processor may touch. All false means the processor computes over
/// the bytes it is handed and nothing else.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Permissions {
    pub network: bool,
    pub filesystem: bool,
    pub model: bool,
    pub credentials: bool,
    /// Whether the processor sees raw content, prompts or responses.
    pub raw_content: bool,
}

/// How the processor is implemented and where its code lives.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Implementation {
    /// `in-process` today, including the Encypher HTTP adapter. The generic
    /// external processor protocol remains planned (`docs/contracts/processor.md`).
    pub kind: &'static str,
    pub crate_name: &'static str,
    pub crate_version: &'static str,
}

/// What a processor declares about itself before it is allowed to run.
///
/// For an in-process processor the implementation identity is the crate
/// version it was compiled from, and the configuration digest covers the
/// canonical rule text that determines its behaviour — the two facts a reader
/// needs to re-derive any invocation record it produces.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessorManifest {
    pub name: &'static str,
    pub version: &'static str,
    pub stage: Stage,
    pub capability: &'static str,
    pub implementation: Implementation,
    /// SHA-256 of the processor's canonical rule text. Behaviour cannot change
    /// without this changing, which is what makes two runs comparable.
    pub configuration_digest: String,
    pub permissions: Permissions,
    pub determinism: Determinism,
    /// Resource limits, stated honestly: in-process processors have no
    /// supervisor to enforce a timeout, and claiming one would claim
    /// enforcement that does not exist.
    pub limits: &'static str,
    pub failure_by_mode: FailureByMode,
    pub evidence_format: &'static str,
}

impl ProcessorManifest {
    /// The identity fields an invocation record carries, so a record is
    /// attributable without restating the whole manifest.
    fn identity(&self) -> Value {
        json!({
            "name": self.name,
            "version": self.version,
            "configuration_digest": self.configuration_digest,
        })
    }
}

const IN_PROCESS: Implementation = Implementation {
    kind: "in-process",
    crate_name: "commonmeasure-runtime",
    crate_version: env!("CARGO_PKG_VERSION"),
};

const NO_AMBIENT_AUTHORITY: Permissions = Permissions {
    network: false,
    filesystem: false,
    model: false,
    credentials: false,
    raw_content: true,
};

/// Every processor compiled into this binary. In-process processors are
/// installed by being built in; there is no separate installation step to
/// drift from what the binary actually does.
pub fn installed() -> [&'static ProcessorManifest; 8] {
    [
        pii::manifest(),
        injection::manifest(),
        support::manifest(),
        extract::manifest(),
        optimise::manifest(),
        fidelity::manifest(),
        judge::manifest(),
        provenance::manifest(),
    ]
}

/// One artefact an invocation read or produced, by reference and hash rather
/// than by copy.
#[derive(Debug, Clone, Serialize)]
pub struct ArtefactRef {
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}

/// One processor invocation, as it goes into the evidence trail.
#[derive(Debug)]
pub struct Invocation {
    manifest: &'static ProcessorManifest,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    pub decision: Decision,
    method: String,
    inputs: Vec<ArtefactRef>,
    outputs: Vec<ArtefactRef>,
    /// Processor-specific findings and measures. Namespaced by construction:
    /// they live inside a record that names the processor, never beside
    /// another processor's numbers on a shared scale.
    detail: Value,
    gaps: Vec<Gap>,
    blind_spots: Vec<&'static str>,
    /// How strongly the record's findings are evidenced. Deterministic
    /// processors compute over bytes this runtime handed them, so their
    /// findings are observed; a model judge's verdict is the model's
    /// statement about those bytes, so it is declared.
    assurance: AssuranceBasis,
}

impl Invocation {
    #[allow(clippy::too_many_arguments)]
    fn new(
        manifest: &'static ProcessorManifest,
        started_at: DateTime<Utc>,
        decision: Decision,
        method: impl Into<String>,
        inputs: Vec<ArtefactRef>,
        outputs: Vec<ArtefactRef>,
        detail: Value,
        gaps: Vec<Gap>,
        blind_spots: Vec<&'static str>,
    ) -> Self {
        Self {
            manifest,
            started_at,
            finished_at: Utc::now(),
            decision,
            method: method.into(),
            inputs,
            outputs,
            detail,
            gaps,
            blind_spots,
            assurance: AssuranceBasis::Observed,
        }
    }

    /// Mark the record's findings as a model's or supplier's statement rather
    /// than an observation of the bytes.
    fn declared(mut self) -> Self {
        self.assurance = AssuranceBasis::Declared;
        self
    }

    /// The record as it is appended to an evidence log and carried in a plan.
    ///
    /// `timestamp` duplicates `finished_at` so every evidence reader that
    /// orders records by their common timestamp field can order these too.
    pub fn to_value(&self) -> Value {
        let latency = (self.finished_at - self.started_at)
            .num_milliseconds()
            .max(0);
        json!({
            "record_version": INVOCATION_VERSION,
            "processor": self.manifest.identity(),
            "stage": self.manifest.stage,
            "timestamp": stamp(self.finished_at),
            "started_at": stamp(self.started_at),
            "finished_at": stamp(self.finished_at),
            "latency_ms": latency,
            "decision": self.decision,
            "method": self.method,
            "inputs": self.inputs,
            "outputs": self.outputs,
            "detail": self.detail,
            "gaps": self.gaps,
            "assurance": self.assurance,
            "blind_spots": self.blind_spots,
        })
    }
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}
