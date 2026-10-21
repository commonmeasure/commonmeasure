//! The Common Measure domain: what a job declares, what supply looks like once it
//! is normalised, and what the runtime is obliged to record about a decision.
//!
//! Pure values only. Nothing here opens a socket, reads a file or knows a
//! provider's wire format, which is what lets the same types describe a
//! recorded replay and a live run without either becoming a special case.

pub mod address;
mod allowance;
pub mod canonical;
pub mod display;
mod envelope;
mod evidence;
pub mod finding;
mod job;
mod money;
mod plan;
pub mod release;

pub use allowance::{AllowanceDeclaration, AllowancePeriod};
pub use envelope::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, LicenceState, NativeCharge,
};
pub use evidence::{AssuranceBasis, Decision, DecisionRecord, EvidenceError, Gap, GapReason};
pub use finding::{Finding, Standing};
pub use job::{
    AccessAction, Constraint, ContextJob, EvidenceRequirement, HostPattern, JobError,
    MAX_PAGE_READINGS, Objective, PolicyMode, SignerKey, TooManyReadings, canonical_url,
    matching_pattern, matching_target, matching_target_readings, matching_url,
    matching_url_readings, normalised_host, release_signer_for,
};
pub use money::Money;
pub use plan::{ModelPlan, PlanError, ProviderCapability, ProviderRef, SupplyPlan, SupplyStep};
