//! Host integration: how Common Measure attaches to a coding harness.
//!
//! Two ways in, and they are not equal. Both record; only one can refuse.
//!
//! - **Observed** — lifecycle hooks. `PostToolUse` fires *after* the crossing,
//!   so a hook can record what the agent read but cannot stop it. It needs no
//!   credentials and no configuration, so it starts recording the moment the
//!   user works.
//! - **Mediated** — MCP tools the agent calls instead of its own. These run
//!   *before* the crossing, so policy can refuse.
//!
//! Every record carries which of the two produced it. Recording them as the
//! same grade of evidence would overstate what the observed path can promise,
//! and it is carried as an explicit field rather than inferred from which code
//! path wrote the row.
//!

/// The declared-artefact write primitives now live in `commonmeasure-runtime`, beside
/// the allowance ledger that shares them; re-exported here because this
/// crate's policy file and `commonmeasure-console`'s attribution rules are the artefacts
/// they were written for.
pub use commonmeasure_runtime::declaration;
pub mod browser;
pub mod declarations;
pub mod directory;
pub mod discovery;
pub mod enrolment;
pub mod fleet;
pub mod grounding;
pub mod hook;
pub mod identity;
pub mod import;
pub mod managed;
pub mod manifest;
pub mod mcp;
pub mod nudge;
pub mod policy;
pub mod prompt;
pub mod registration;
pub mod session;
pub mod snapshot;

pub use enrolment::{EnrolmentRecord, enrolled_key_id};
pub use hook::{HookInput, HostSurface, capture};
pub use identity::{EdgeKey, Identity, PresentedIdentity};
pub use import::known_hosts;
pub use session::{
    Crossing, CrossingMode, SessionLog, SessionSummary, boundary_policy, home_dir, safe_session,
    summarise,
};
