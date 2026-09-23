use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Admit,
    Refuse,
    Fallback,
    Abstain,
}

/// How strongly a recorded fact is evidenced. A supplier's own statement is
/// `Declared`; bytes this runtime saw are `Observed`. The two are never merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssuranceBasis {
    Declared,
    Observed,
    Corroborated,
    CryptographicallyVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    ProviderUnavailable,
    CapabilityUnavailable,
    PolicyRefused,
    BudgetExhausted,
    /// A measurement, licence or provenance fact the run needed and did not
    /// obtain. Never filled in with a plausible value.
    EvidenceMissing,
    /// Supply could not cover what the plan asked of it, from a source whose
    /// shortfall is knowable: a bounded corpus holding fewer matching
    /// documents than requested, or a provider whose published search page is
    /// smaller than the job's result limit. An open-web search that returned
    /// little claims no gap — what the web holds is not knowable from it. It
    /// is the record "where did our supply fall short of the job" is built
    /// from.
    CoverageGap,
    /// No inference backend was configured or reachable, so no answer exists.
    InferenceUnavailable,
    /// The evidence log could not record something that happened. Written by
    /// the log's own gap record (`commonmeasure_runtime::evidence`), which is
    /// why nothing on a plan carries it.
    WriteFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub reason: GapReason,
    pub detail: String,
}

impl Gap {
    pub fn new(reason: GapReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// One policy decision, with the reason and the evidence it rests on.
///
/// A refusal without a gap is rejected by [`DecisionRecord::validate`]: a run
/// that refuses something has, by construction, left a hole in its own
/// evidence, and the hole must be recorded where a reader will find it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub id: Uuid,
    pub job_id: Uuid,
    pub run_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub decision: Decision,
    pub reason: String,
    pub assurance: AssuranceBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_cost: Option<Money>,
    #[serde(default)]
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    EmptyReason,
    RefusalWithoutGap,
}

impl DecisionRecord {
    pub fn validate(&self) -> Result<(), EvidenceError> {
        if self.reason.trim().is_empty() {
            return Err(EvidenceError::EmptyReason);
        }
        if self.decision == Decision::Refuse && self.gaps.is_empty() {
            return Err(EvidenceError::RefusalWithoutGap);
        }
        Ok(())
    }
}
