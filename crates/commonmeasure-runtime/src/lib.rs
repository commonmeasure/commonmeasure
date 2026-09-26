//! The Common Measure runtime: the one place a job becomes a run.
//!
//! It holds the three things that must not be spread across a CLI and a
//! console — deterministic policy, the orchestration that calls real
//! adapters and a real inference gateway, and the append-only evidence trail —
//! so the binary above it is a thin argument parser and the artefacts below it
//! are the whole story.

pub mod allowance;
pub mod artifact;
pub mod benchmark;
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

#[cfg(all(test, unix))]
pub(crate) mod test_umask {
    /// Runs `body` in a child of this test binary whose umask is 022, set
    /// between fork and exec, and asserts that the child ran the one test
    /// `name` (its path from the crate root) and passed. An owner-only
    /// assertion passes whatever mode the writer asks for under a umask that
    /// already masks 066, such as 077; the child makes it hold under any
    /// umask the suite runs with, without touching this process's umask.
    pub(crate) fn under_umask_022(name: &str, body: impl FnOnce()) {
        use std::os::unix::process::CommandExt as _;
        const CHILD: &str = "COMMONMEASURE_TEST_UMASK_CHILD";
        if std::env::var_os(CHILD).is_some_and(|test| test == name) {
            body();
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", name, "--test-threads=1"])
            .env(CHILD, name);
        // SAFETY: `umask` is async-signal-safe and changes only the child's
        // own process state, between fork and exec.
        unsafe {
            child.pre_exec(|| {
                libc::umask(0o022);
                Ok(())
            });
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        assert!(
            stdout.contains("test result: ok. 1 passed"),
            "the child did not run exactly one test: {stdout}"
        );
    }
}
