//! The host process a hook or an MCP server runs under, read from the
//! operating system's process table, and whether a process named that way is
//! still there (`docs/contracts/session-evidence.md` §Host process).
//!
//! On the hosts that pass no session identifier to the MCP server, a hook
//! writes under the host's identifier and the server under one it mints.
//! Both were started by one host process, and that process is the only join
//! between the two logs. Each writer walks up from its own parent to the
//! first ancestor that is not a shell or a launcher wrapper and records that
//! process as `pid`, its start time to the second and its executable name.
//! The pair `(pid, started_at)` is the identity: the system reuses numbers,
//! so a pid alone is not one. Arguments are never read, because a host's
//! command line can carry a prompt.
//!
//! The process table is `sysinfo` (MIT), asked for one pid at a time with
//! nothing but the basic fields, so a walk costs a few lookups and reads no
//! command line.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Executable names the walk steps over: the shells a host runs a hook
/// command through, this product's own plugin shim (which `exec`s the
/// binary, so it is rarely seen, but a `sh -c` around it is), and the
/// `disclaimer` helper Claude Desktop wraps each server in.
pub const WRAPPERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "ksh",
    "commonmeasure-launch",
    "disclaimer",
];

/// How the recorded process was chosen, written on every record so a reader
/// of the log alone knows what the pair names.
pub const BASIS: &str = "the first ancestor of the writing process that is not a shell or \
                         launcher wrapper, read from the operating system's process table when \
                         the record was written";

/// The longest walk attempted before the table is declared unreadable, so a
/// cycle in what the table reports cannot spin a hook.
const MAX_DEPTH: usize = 64;

/// One process in the table: the pair that identifies it and the name of
/// what it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProcess {
    pub pid: u32,
    /// Start time as the operating system reports it, to the second.
    pub started_at: DateTime<Utc>,
    /// The executable's file name only, never its arguments.
    pub command: String,
}

impl HostProcess {
    /// Two records name one host process when both pid and start time
    /// agree. A reused pid has a different start time and joins nothing.
    pub fn same(&self, other: &Self) -> bool {
        self.pid == other.pid && self.started_at == other.started_at
    }

    /// The process a `host_process` record's payload names, or `None` where
    /// the record says it was unavailable.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let process = payload.get("host_process")?;
        if process.is_null() {
            return None;
        }
        serde_json::from_value(process.clone()).ok()
    }
}

/// What a writer found when it looked for its host process: the process, or
/// the reason the table gave none. Recorded either way, so a log written
/// where the table could not be read says so rather than joining nothing
/// silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The process that wrote the record.
    pub writer_pid: u32,
    pub host_process: Result<HostProcess, String>,
}

impl Finding {
    /// The fields the `host_process` record carries beside the session
    /// identity, host and timestamp: `path` is `hook` or `mcp`.
    pub fn to_payload(&self, path: &str) -> Value {
        let mut payload = json!({
            "path": path,
            "writer_pid": self.writer_pid,
            "basis": BASIS,
        });
        match &self.host_process {
            Ok(process) => {
                payload["host_process"] = json!({
                    "pid": process.pid,
                    "started_at": process.started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "command": process.command,
                });
            }
            Err(reason) => {
                payload["host_process"] = Value::Null;
                payload["unavailable"] = json!(reason);
            }
        }
        payload
    }
}

/// Find the host process of the current process: its first ancestor that is
/// not a shell or launcher wrapper.
pub fn find() -> Finding {
    let writer_pid = std::process::id();
    Finding {
        writer_pid,
        host_process: find_from(writer_pid),
    }
}

/// Find the host process of `pid`: walk from its parent upwards. The walk
/// itself is what a hook and a server share, so a test can start it from a
/// process it spawned and know what the answer must be.
pub fn find_from(pid: u32) -> Result<HostProcess, String> {
    table::walk(pid)
}

/// Whether a process with this pid and this start time is in the table now.
/// `None` where the table cannot be read on this platform, which a reader
/// shows as liveness unavailable rather than as either answer.
pub fn is_present(pid: u32, started_at: DateTime<Utc>) -> Option<bool> {
    table::is_present(pid, started_at)
}

/// The `host_process` record of the log at `path`, read line by line and
/// stopped at the first one, so scanning every log on an edge costs a few
/// lines each. A log without the record joins nothing.
pub fn recorded_in(path: &Path) -> Option<HostProcess> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).ok()? == 0 {
            return None;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if record["event"] == "host_process" {
            return HostProcess::from_payload(&record["payload"]);
        }
    }
}

/// The logs that join the session at `path`: every other log under the
/// same sessions directory whose `host_process` record names the same
/// process. Empty where this log carries no record, or no other log agrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// The process the logs agree on, or `None` where this log has no record
    /// and so joins nothing.
    pub host_process: Option<HostProcess>,
    /// The other logs, most recently modified first.
    pub others: Vec<PathBuf>,
}

pub fn joined_logs(home: &Path, path: &Path) -> std::io::Result<Join> {
    let Some(process) = recorded_in(path) else {
        return Ok(Join {
            host_process: None,
            others: Vec::new(),
        });
    };
    let own = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let others = crate::session::SessionLog::list(home)?
        .into_iter()
        .filter(|other| other.canonicalize().unwrap_or_else(|_| other.clone()) != own)
        .filter(|other| recorded_in(other).is_some_and(|found| found.same(&process)))
        .collect();
    Ok(Join {
        host_process: Some(process),
        others,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod table {
    use super::{HostProcess, MAX_DEPTH, WRAPPERS};
    use chrono::{DateTime, Utc};
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    /// The basic fields only: parent, start time and name. On Linux the
    /// name in the table is truncated to fifteen characters and the exe
    /// link is not, so the link is read there; on macOS the name is the
    /// executable's file name already, and asking for the exe would read
    /// the argument area, which this module never does.
    fn refresh_kind() -> ProcessRefreshKind {
        let kind = ProcessRefreshKind::nothing();
        #[cfg(target_os = "linux")]
        let kind = kind.with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
        kind
    }

    /// One process, read now.
    fn lookup(system: &mut System, pid: u32) -> Option<HostProcess> {
        let id = Pid::from_u32(pid);
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[id]), true, refresh_kind());
        let process = system.process(id)?;
        let command = process
            .exe()
            .and_then(|exe| exe.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| process.name().to_string_lossy().into_owned());
        let started_at =
            DateTime::<Utc>::from_timestamp(i64::try_from(process.start_time()).ok()?, 0)?;
        Some(HostProcess {
            pid,
            started_at,
            command,
        })
    }

    fn parent_of(system: &System, pid: u32) -> Option<u32> {
        system
            .process(Pid::from_u32(pid))
            .and_then(|process| process.parent())
            .map(|parent| parent.as_u32())
    }

    pub(super) fn walk(from: u32) -> Result<HostProcess, String> {
        let mut system = System::new();
        let mut pid = from;
        for _ in 0..MAX_DEPTH {
            if lookup(&mut system, pid).is_none() {
                return Err(format!("process {pid} is not in the process table"));
            }
            let Some(parent) = parent_of(&system, pid).filter(|parent| *parent != pid) else {
                return Err(format!(
                    "the walk reached process {pid}, which has no parent, without leaving \
                     shells and launcher wrappers"
                ));
            };
            let Some(process) = lookup(&mut system, parent) else {
                return Err(format!(
                    "the parent of process {pid}, process {parent}, left the table during the \
                     walk"
                ));
            };
            if parent <= 1 || process.command.is_empty() {
                return Err(format!(
                    "the walk reached the root of the process tree (process {parent}) without \
                     leaving shells and launcher wrappers"
                ));
            }
            if !WRAPPERS.contains(&process.command.as_str()) {
                return Ok(process);
            }
            pid = parent;
        }
        Err(format!(
            "the walk did not leave shells and launcher wrappers within {MAX_DEPTH} ancestors"
        ))
    }

    pub(super) fn is_present(pid: u32, started_at: DateTime<Utc>) -> Option<bool> {
        let mut system = System::new();
        Some(lookup(&mut system, pid).is_some_and(|process| process.started_at == started_at))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod table {
    use super::HostProcess;
    use chrono::{DateTime, Utc};

    pub(super) fn walk(_from: u32) -> Result<HostProcess, String> {
        Err(format!(
            "the process table is not read on {}; only macOS and Linux are",
            std::env::consts::OS
        ))
    }

    pub(super) fn is_present(_pid: u32, _started_at: DateTime<Utc>) -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionLog;

    fn process(pid: u32, started_at: &str, command: &str) -> HostProcess {
        HostProcess {
            pid,
            started_at: started_at.parse().unwrap(),
            command: command.to_owned(),
        }
    }

    /// The record names the writer, the process found and the basis; a
    /// reader gets the pair back as it was written, to the second.
    #[test]
    fn the_record_carries_the_pair_the_command_and_the_basis() {
        let finding = Finding {
            writer_pid: 99796,
            host_process: Ok(process(94522, "2026-09-17T22:55:48Z", "claude")),
        };
        let payload = finding.to_payload("hook");
        assert_eq!(payload["path"], "hook");
        assert_eq!(payload["writer_pid"], 99796);
        assert_eq!(payload["host_process"]["pid"], 94522);
        assert_eq!(
            payload["host_process"]["started_at"],
            "2026-09-17T22:55:48Z"
        );
        assert_eq!(payload["host_process"]["command"], "claude");
        assert_eq!(payload["basis"], BASIS);
        assert!(payload.get("unavailable").is_none());
        assert_eq!(
            HostProcess::from_payload(&payload),
            Some(process(94522, "2026-09-17T22:55:48Z", "claude"))
        );
    }

    /// Where the table gave nothing, `host_process` is null and
    /// `unavailable` says why; nothing is guessed and the reader sees no
    /// process.
    #[test]
    fn an_unreadable_table_records_null_and_the_reason() {
        let finding = Finding {
            writer_pid: 7,
            host_process: Err("process 7 is not in the process table".to_owned()),
        };
        let payload = finding.to_payload("mcp");
        assert!(payload["host_process"].is_null());
        assert_eq!(
            payload["unavailable"],
            "process 7 is not in the process table"
        );
        assert_eq!(payload["basis"], BASIS);
        assert_eq!(HostProcess::from_payload(&payload), None);
    }

    /// The same pid with a different start time is a reused number, not the
    /// same process: the two logs stay apart.
    #[test]
    fn a_reused_pid_joins_nothing() {
        let first = process(94522, "2026-09-17T22:55:48Z", "claude");
        let reused = process(94522, "2026-09-18T09:01:02Z", "claude");
        let again = process(94522, "2026-09-17T22:55:48Z", "claude");
        assert!(!first.same(&reused));
        assert!(first.same(&again));
    }

    /// Over real logs in a sessions directory: the hook log and the MCP log
    /// that name one process join; a log naming the pid with another start
    /// time does not; a log without the record joins nothing and is joined
    /// by nothing.
    #[test]
    fn joined_logs_agree_on_pid_and_start_time_and_on_nothing_else() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let started = "2026-09-17T22:55:48Z";
        let record = |session: &str, path: &str, process: Result<HostProcess, String>| {
            let mut log = SessionLog::open(&home, session).expect("open");
            log.record_host_process(
                "claude-code",
                path,
                &Finding {
                    writer_pid: 1,
                    host_process: process,
                },
            )
            .expect("append");
            log.path().to_path_buf()
        };
        let hook = record("hook-log", "hook", Ok(process(94522, started, "claude")));
        let mcp = record("local-1-2", "mcp", Ok(process(94522, started, "claude")));
        let reused = record(
            "local-3-4",
            "mcp",
            Ok(process(94522, "2026-09-18T09:01:02Z", "claude")),
        );
        let unavailable = record("local-5-6", "mcp", Err("no table".to_owned()));
        let mut bare = SessionLog::open(&home, "bare").expect("open");
        bare.record_context_snapshot(json!({})).expect("append");

        let join = joined_logs(&home, &hook).expect("join");
        assert_eq!(join.host_process, Some(process(94522, started, "claude")));
        assert_eq!(join.others, vec![mcp.clone()]);
        let join = joined_logs(&home, &mcp).expect("join");
        assert_eq!(join.others, vec![hook.clone()]);
        for alone in [&reused, &unavailable, bare.path()] {
            let join = joined_logs(&home, alone).expect("join");
            assert!(join.others.is_empty(), "{}", alone.display());
        }
        assert_eq!(joined_logs(&home, &unavailable).unwrap().host_process, None);
    }

    /// On this platform the walk from a child this test spawned finds the
    /// test process itself, because the child is a shell and the test
    /// binary is not; the pair it names is present now, and the same pid
    /// with another start time is not.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_walk_steps_over_a_shell_and_the_probe_knows_the_pair() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 5"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("sh");
        let found = find_from(child.id()).expect("the walk finds a process");
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(found.pid, std::process::id(), "{found:?}");
        assert!(!WRAPPERS.contains(&found.command.as_str()), "{found:?}");
        assert_eq!(is_present(found.pid, found.started_at), Some(true));
        assert_eq!(
            is_present(found.pid, found.started_at + chrono::Duration::hours(1)),
            Some(false)
        );
        let own = find();
        assert_eq!(own.writer_pid, std::process::id());
        // The test binary's own parent is the cargo test runner, or
        // whatever started it; only that it is not a wrapper is known.
        if let Ok(parent) = own.host_process {
            assert!(!WRAPPERS.contains(&parent.command.as_str()), "{parent:?}");
        }
    }
}
