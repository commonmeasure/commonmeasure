//! The operator console: an SQLite index over the evidence logs and the
//! loopback HTTP server that renders it.
//!
//! The NDJSON evidence logs remain the source of truth; this crate derives an
//! SQLite index from them and serves the operator console over loopback HTTP.
//! Deleting the database loses nothing: it is rebuilt from the logs, and every
//! aggregate it serves can be re-derived by reading the same files by hand.
//!
//! Two disciplines carry over from the rest of the runtime. Ingestion is
//! lossless — every log line lands as a row, and a line that cannot be parsed
//! is counted and kept rather than skipped, so the index can never quietly
//! hold less than the log. And the three grades of evidence never merge: every
//! aggregate that involves grounding or crossings reports witnessed (observed
//! and mediated) apart from reconstructed.
//!

mod attribution;
pub mod compare_export;
mod console;
pub mod guide;
mod serve;
mod store;

pub use attribution::{Attribution, UNATTRIBUTED};
pub use serve::{LivenessProbe, SearchRunner, ServeOptions, serve, start};
pub use store::{CrossingFact, CrossingFacts, IngestReport, Store};
