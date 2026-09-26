//! The Common Measure domain: what a job declares, what supply looks like once it
//! is normalised, and what the runtime is obliged to record about a decision.
//!
//! Pure values only. Nothing here opens a socket, reads a file or knows a
//! provider's wire format, which is what lets the same types describe a
//! recorded replay and a live run without either becoming a special case.

pub mod address;
mod allowance;
pub mod canonical;
mod envelope;
mod evidence;
mod job;
mod money;
mod plan;

pub use allowance::{AllowanceDeclaration, AllowancePeriod};
pub use envelope::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, LicenceState, NativeCharge,
};
pub use evidence::{AssuranceBasis, Decision, DecisionRecord, EvidenceError, Gap, GapReason};
pub use job::{
    AccessAction, Constraint, ContextJob, EvidenceRequirement, HostPattern, JobError, Objective,
    PolicyMode, canonical_url, normalised_host,
};
pub use money::Money;
pub use plan::{ModelPlan, PlanError, ProviderCapability, ProviderRef, SupplyPlan, SupplyStep};
