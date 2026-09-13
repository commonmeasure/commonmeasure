//! The Common Measure runtime: the one place a job becomes a run.
//!
//! It holds the three things that must not be spread across a CLI and a
//! console — deterministic policy, the orchestration that calls real
//! adapters and a real inference gateway, and the append-only evidence trail —
//! so the binary above it is a thin argument parser and the artefacts below it
//! are the whole story.

pub mod allowance;
pub mod coverage;
pub mod declaration;
pub mod evaluate;
pub mod evidence;
pub mod freshness;
pub mod governance;
pub mod policy;
pub mod processor;
pub mod replay;
pub mod run;
pub mod selection;

pub use coverage::{CoverageRubric, RubricItem};
pub use evidence::{EvidenceLog, RunDirectory};
pub use freshness::AsOf;
pub use replay::{ReplayContext, ReplayError, ReplaySupply};
pub use run::{
    EvidenceOutcome, RunError, RunOptions, RunReport, SCHEMA_VERSION, Suite, execute, finalise_log,
    load_suite,
};
pub use selection::{Candidate, MeasuredCoverage, Selection};

/// The verb ending for "carr{ies,y}" over a list of `count` plan names, so
/// one plan reads "exa-only carries no measured coverage" and two read
/// "exa-only, tavily-only carry no measured coverage".
///
/// Written once because a gap's prose is read as evidence: two records of the
/// same absence must not differ in how they word it.
#[must_use]
pub(crate) fn carries(count: usize) -> &'static str {
    if count == 1 { "ies" } else { "y" }
}
