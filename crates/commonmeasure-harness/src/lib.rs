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
pub mod compare;
pub mod compare_export;
pub mod crawl_delay;
pub mod declarations;
pub mod delivery;
pub mod directory;
pub mod discovery;
pub mod enrolment;
pub mod fleet;
pub mod grounding;
pub mod hook;
pub mod host_process;
pub mod identity;
pub mod import;
pub mod instance;
pub mod managed;
pub mod manifest;
pub mod mcp;
pub mod nudge;
pub mod policy;
pub mod prompt;
pub mod registration;
pub mod relay_config;
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
