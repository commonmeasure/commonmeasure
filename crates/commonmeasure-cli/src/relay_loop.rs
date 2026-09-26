//! `commonmeasure relay --every <seconds>`: the background relay, for hosts
//! that send no session-end event (Claude Desktop, Codex, Cursor, the Copilot
//! CLI, VS Code, Pi).
//!
//! Each tick is the run a session end starts (`start_session_end_relay`):
//! `commonmeasure relay` with no options, so the receiver is the one in
//! `relay.json`, and clearance, the supplier scope, reporting approvals,
//! backoff and the spool lock apply as to any run. The loop adds no egress
//! of its own. It skips a tick while the `relay/manual` marker is present or
//! `relay.json` names no receiver, and says so in its journal.
//!
//! For as long as it runs it holds `relay-loop.lock` in the home
//! ([`commonmeasure_harness::delivery::RELAY_LOCK_FILE`]), which is how the
//! licence ruling, `status` and `doctor` tell that the home has a carrier for
//! every host. The spool's own lock is taken only during a tick, so a
//! session-end run or a typed `commonmeasure relay` between ticks runs as
//! usual, and one that meets a tick loses the spool lock and leaves its work
//! to the next run. A session still being written is relayed as far as its
//! log reads: event ids are derived from the records, and `delivered.idx`
//! and the spool keep an event from leaving twice.
//!
//! SIGTERM (what launchd sends at `bootout`) and SIGINT end the loop once the
//! tick in progress has finished; the lock is released when the process
//! exits.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use commonmeasure_harness::delivery::RELAY_LOCK_FILE;
use serde::{Deserialize, Serialize};

/// The interval `service install relay` writes unless told otherwise: the
/// hosted service's default.
pub(crate) const DEFAULT_EVERY_SECS: u64 = 300;

/// The longest interval `--every` takes: a day. A loop holding the lock
/// counts as a carrier for every reporting demand in the home, so an
/// interval of a year would admit a source whose report then waits a year.
pub(crate) const MAX_EVERY_SECS: u64 = 86_400;

/// Whether the loop can run every `every` seconds.
pub(crate) fn check_every(every: u64) -> Result<(), String> {
    if (1..=MAX_EVERY_SECS).contains(&every) {
        Ok(())
    } else {
        Err(format!(
            "--every must be from 1 to {MAX_EVERY_SECS} seconds (a day); {every} is out of range"
        ))
    }
}

/// What the holder writes into its lock file, so that a second loop's
/// refusal and `doctor` can name it. The lock, not this record, says whether
/// a loop runs: a record left by a loop that has exited is read only while
/// the lock is held again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Holder {
    pub(crate) pid: u32,
    pub(crate) every_seconds: u64,
}

impl Holder {
    /// The record in `home`'s lock file, where it reads.
    pub(crate) fn read(home: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(lock_path(home)).ok()?).ok()
    }

    fn describe(&self) -> String {
        format!("pid {}, every {}s", self.pid, self.every_seconds)
    }
}

pub(crate) fn lock_path(home: &Path) -> PathBuf {
    home.join(RELAY_LOCK_FILE)
}

/// Why a loop did not start. The remedy differs: a configuration refusal
/// is fixed in `relay.json`, a lock that cannot be taken is fixed on the
/// home, and a home another loop holds is already relayed, so nothing needs
/// starting.
#[derive(Debug)]
enum Refusal {
    /// `relay.json` names no receiver, or does not load.
    Configuration(String),
    /// Another loop holds the home.
    Held(String),
    /// The lock could not be opened, taken or written.
    Lock(String),
}

impl Refusal {
    /// The journal line: the reason and, where starting the loop again is
    /// part of the remedy, how.
    fn line(&self, home: &Path) -> String {
        const AGAIN: &str = "`commonmeasure service install relay` starts the LaunchAgent again";
        match self {
            Self::Configuration(reason) => format!(
                "commonmeasure: background relay not started: {reason}. It stays stopped until \
                 {} names a receiver the relay loads and it is started again; {AGAIN}",
                home.join("relay.json").display()
            ),
            Self::Held(reason) => format!("commonmeasure: background relay not started: {reason}"),
            Self::Lock(reason) => format!(
                "commonmeasure: background relay not started: {reason}. It stays stopped until \
                 it is started again; {AGAIN}"
            ),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(reason) | Self::Held(reason) | Self::Lock(reason) => {
                f.write_str(reason)
            }
        }
    }
}

/// The loop's hold on the home.
struct LoopLock {
    _file: std::fs::File,
}

/// How long a starting loop keeps trying for a lock another process holds.
/// `doctor`, `status`, the licence ruling and `service install relay`'s own
/// polling each take a free lock for an instant to see whether it is free;
/// a loop starting in that instant would otherwise be refused, and under
/// launchd stay stopped.
const TAKE_FOR: Duration = Duration::from_secs(1);

impl LoopLock {
    fn take(home: &Path, every: u64) -> Result<Self, Refusal> {
        let path = lock_path(home);
        let mut file = commonmeasure_runtime::declaration::open_lock(&path).map_err(|error| {
            Refusal::Lock(format!("cannot open the lock {}: {error}", path.display()))
        })?;
        let until = std::time::Instant::now() + TAKE_FOR;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(Refusal::Held(format!(
                        "another background relay holds {}{}; one background relay runs per home",
                        path.display(),
                        Holder::read(home)
                            .filter(|holder| alive(holder.pid))
                            .map(|holder| format!(" ({})", holder.describe()))
                            .unwrap_or_default()
                    )));
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(Refusal::Lock(format!(
                        "cannot lock {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        let record = serde_json::to_vec(&Holder {
            pid: std::process::id(),
            every_seconds: every,
        })
        .map_err(|error| Refusal::Lock(error.to_string()))?;
        file.set_len(0)
            .and_then(|()| file.write_all(&record))
            .map_err(|error| Refusal::Lock(format!("cannot write {}: {error}", path.display())))?;
        Ok(Self { _file: file })
    }
}

/// Whether a process with `pid` exists. The record in the lock file is the
/// last loop's; the lock may be held by a process that has not yet written
/// its own (or a probe), so a pid is named only when it is alive.
#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 checks for the process without sending anything.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(crate) fn alive(_pid: u32) -> bool {
    false
}

/// Run the background relay over `home` until SIGTERM or SIGINT.
///
/// A refusal at start that the configuration causes (no receiver, a
/// `relay.json` that does not load, another loop holding the home) is
/// reported and returns `Ok`, so the process exits 0: the LaunchAgent
/// restarts the loop only after an unsuccessful exit
/// ([`crate::service::relay_plist`]), and a refused loop left to exit 1 would
/// be started and refused again every five minutes for as long as the cause
/// lasts. An interval it cannot run with is an error, as for any command.
pub(crate) fn run(home: &Path, every: u64) -> Result<(), String> {
    check_every(every)?;
    let lock = match start(home, every) {
        Ok(lock) => lock,
        Err(refusal) => {
            eprintln!("{}", refusal.line(home));
            return Ok(());
        }
    };
    stop::install();
    eprintln!("{}", start_line(home, every));
    let interval = Duration::from_secs(every);
    loop {
        tick(home);
        if let Some(signal) = stop::wait(interval) {
            eprintln!("commonmeasure: background relay stopped by {signal}");
            break;
        }
    }
    drop(lock);
    Ok(())
}

/// The journal's first line. A receiver scoped to suppliers is named, with
/// what it means here: the loop still runs for every session, but only the
/// listed suppliers' events leave, and a fetched page names no supplier.
fn start_line(home: &Path, every: u64) -> String {
    format!(
        "commonmeasure: background relay for {} every {every}s, pid {}{}",
        home.display(),
        std::process::id(),
        commonmeasure_relay::config::RelayConfig::load(home)
            .ok()
            .flatten()
            .as_ref()
            .and_then(scope_consequence)
            .map(|consequence| format!("; {consequence}"))
            .unwrap_or_default()
    )
}

/// What a receiver scoped to suppliers means for this home, where
/// `relay.json` is scoped: said by the loop, `service install relay` and
/// `doctor`.
pub(crate) fn scope_consequence(
    config: &commonmeasure_relay::config::RelayConfig,
) -> Option<String> {
    let scope = config.scope()?;
    let leaves = if config.suppliers.as_ref().is_some_and(Vec::is_empty) {
        "no event leaves"
    } else {
        "only their events leave"
    };
    Some(format!(
        "the receiver is scoped to {scope}, so {leaves} and a source whose licence demands usage \
         reporting is refused on this home"
    ))
}

/// The checks before the first tick, and the hold on the home.
fn start(home: &Path, every: u64) -> Result<LoopLock, Refusal> {
    // Refused at start rather than skipped at every tick: a loop with
    // nowhere to send would otherwise hold the lock, and so count as a
    // carrier, while sending nothing.
    if commonmeasure_relay::config::RelayConfig::load(home)
        .map_err(Refusal::Configuration)?
        .is_none()
    {
        return Err(Refusal::Configuration(format!(
            "no telemetry receiver is configured in {}; the background relay sends only to the \
             receiver named there",
            home.join("relay.json").display()
        )));
    }
    LoopLock::take(home, every)
}

/// One run, reported to the journal. A failed run is reported and the loop
/// goes on: what it did not send stays in the session logs and the spool.
fn tick(home: &Path) {
    if let Some(reason) = commonmeasure_harness::delivery::withheld_reason(home) {
        eprintln!("commonmeasure: relay skipped: {reason}");
        return;
    }
    match commonmeasure_relay::config::RelayConfig::load(home) {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!(
                "commonmeasure: relay skipped: {} names no receiver",
                home.join("relay.json").display()
            );
            return;
        }
        Err(error) => {
            eprintln!("commonmeasure: relay skipped: {error}");
            return;
        }
    }
    eprintln!(
        "commonmeasure: relay run at {}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if let Err(error) = crate::relay(None, None, Vec::new(), Vec::new(), false, None) {
        eprintln!("commonmeasure: relay: {error}");
    }
}

/// What `doctor` and `status` say about the background relay, with this
/// user's relay LaunchAgent where one is installed.
pub(crate) fn line(home: &Path) -> String {
    line_for(
        home,
        crate::service::relay_context()
            .ok()
            .and_then(|context| crate::service::relay_agent(&context)),
    )
}

/// [`line`], given the relay LaunchAgent on disk. A free lock with the agent
/// installed for this home means the agent is not holding it: it was refused
/// at start, stopped, or cannot run its binary, and the reason is in its log
/// where it has written one, so "start it" would be the wrong remedy.
pub(crate) fn line_for(home: &Path, agent: Option<crate::service::RelayAgent>) -> String {
    use commonmeasure_harness::delivery::LockState;
    match commonmeasure_harness::delivery::relay_loop_state(home) {
        LockState::Running => format!(
            "running (lock held{})",
            Holder::read(home)
                .map(|holder| format!(", {}", holder.describe()))
                .unwrap_or_default()
        ),
        LockState::NotRunning => match agent {
            Some(agent) if agent.may_serve(home) => format!(
                "not running; installed ({}) but not holding the home; {}. `commonmeasure \
                 service install relay` starts it again",
                agent.plist.display(),
                match &agent.log {
                    Some(log) if log.exists() => format!("the reason is in {}", log.display()),
                    Some(log) => format!(
                        "it has not written its log {}, so it has not run since it was installed",
                        log.display()
                    ),
                    None => "the plist is not in the form install writes, so its log is not \
                             known"
                        .to_owned(),
                }
            ),
            Some(agent) => format!(
                "not running; the LaunchAgent {} relays {}, and `commonmeasure service install \
                 relay` from this shell moves it to this home; or run `commonmeasure relay \
                 --every {DEFAULT_EVERY_SECS}` under another service manager",
                agent.plist.display(),
                agent
                    .home
                    .as_deref()
                    .map(Path::display)
                    .map(|home| home.to_string())
                    .unwrap_or_default()
            ),
            None => format!(
                "not running; start it with `commonmeasure service install relay` on macOS, or \
                 run `commonmeasure relay --every {DEFAULT_EVERY_SECS}` under another service \
                 manager"
            ),
        },
        LockState::Unknown(reason) => format!("whether one is running cannot be read ({reason})"),
    }
}

/// The signal that ends the loop, recorded by the handler and read between
/// ticks. A handler that only stores to an atomic is async-signal-safe.
mod stop {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::{Duration, Instant};

    static SIGNAL: AtomicI32 = AtomicI32::new(0);

    #[cfg(unix)]
    extern "C" fn record(signal: libc::c_int) {
        SIGNAL.store(signal, Ordering::SeqCst);
    }

    pub(super) fn install() {
        #[cfg(unix)]
        for signal in [libc::SIGTERM, libc::SIGINT] {
            // SAFETY: `record` is an `extern "C"` function that only stores
            // to an atomic, which is async-signal-safe; nothing else in this
            // process installs a handler for these signals.
            unsafe {
                libc::signal(signal, record as *const () as libc::sighandler_t);
            }
        }
    }

    /// Sleep for `interval`, or less if a signal arrives; the signal's name
    /// when one has.
    pub(super) fn wait(interval: Duration) -> Option<&'static str> {
        let until = deadline(interval);
        loop {
            if let Some(name) = received() {
                return Some(name);
            }
            // With no deadline the clock cannot represent, the wait lasts
            // until a signal, which is what an interval that long means.
            let left = until.map_or(Duration::MAX, |until| {
                until.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return None;
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }

    /// When a wait of `interval` from now ends; `None` past what `Instant`
    /// can hold, where `Instant + Duration` would panic.
    pub(super) fn deadline(interval: Duration) -> Option<Instant> {
        Instant::now().checked_add(interval)
    }

    fn received() -> Option<&'static str> {
        #[cfg(unix)]
        match SIGNAL.load(Ordering::SeqCst) {
            0 => None,
            libc::SIGTERM => Some("SIGTERM"),
            libc::SIGINT => Some("SIGINT"),
            _ => Some("a signal"),
        }
        #[cfg(not(unix))]
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn with_receiver(home: &Path) {
        std::fs::write(
            home.join("relay.json"),
            r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
        )
        .unwrap();
    }

    #[test]
    fn a_second_loop_on_the_same_home_is_refused_naming_the_holder() {
        let home = tempfile::tempdir().expect("tempdir");
        with_receiver(home.path());
        let first = LoopLock::take(home.path(), 60).expect("the first takes the lock");
        let refused = match LoopLock::take(home.path(), 30) {
            Err(refused) => refused.to_string(),
            Ok(_) => panic!("a second loop took the lock"),
        };
        assert!(
            refused.contains("another background relay holds")
                && refused.contains(RELAY_LOCK_FILE)
                && refused.contains(&format!("pid {}, every 60s", std::process::id())),
            "{refused}"
        );
        assert!(line(home.path()).starts_with("running (lock held, pid "));
        drop(first);
        assert!(line(home.path()).starts_with("not running; "));
        drop(LoopLock::take(home.path(), 30).expect("released with the first"));
    }

    // Review P3-7: a probe's momentary hold (`lock_state` takes a free lock
    // for an instant) does not refuse a starting loop.
    #[test]
    fn a_starting_loop_outlasts_a_momentary_hold_on_its_lock() {
        let home = tempfile::tempdir().expect("tempdir");
        let probe = commonmeasure_runtime::declaration::open_lock(&lock_path(home.path())).unwrap();
        probe.lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(probe);
        });
        let taken = LoopLock::take(home.path(), 60);
        release.join().unwrap();
        assert!(taken.is_ok(), "{:?}", taken.err());
    }

    // Review P3-7: the record a dead loop left is not named as the holder.
    #[cfg(unix)]
    #[test]
    fn the_refusal_names_the_holder_s_pid_only_while_it_is_alive() {
        let home = tempfile::tempdir().expect("tempdir");
        let held = commonmeasure_runtime::declaration::open_lock(&lock_path(home.path())).unwrap();
        held.lock().unwrap();
        let mut exited = std::process::Command::new("true").spawn().unwrap();
        exited.wait().unwrap();
        std::fs::write(
            lock_path(home.path()),
            format!(r#"{{"pid":{},"every_seconds":60}}"#, exited.id()),
        )
        .unwrap();
        let refused = match LoopLock::take(home.path(), 30) {
            Err(refused) => refused.to_string(),
            Ok(_) => panic!("took a held lock"),
        };
        assert!(
            refused.contains("another background relay holds") && !refused.contains("pid"),
            "{refused}"
        );
        drop(held);
    }

    #[test]
    fn a_loop_without_a_receiver_does_not_start_or_take_the_lock() {
        let home = tempfile::tempdir().expect("tempdir");
        let refused = match start(home.path(), 60) {
            Err(refused) => refused.to_string(),
            Ok(_) => panic!("started without a receiver"),
        };
        assert!(refused.contains("no telemetry receiver"), "{refused}");
        assert_eq!(run(home.path(), 60), Ok(()), "refused, and exits 0");
        assert!(!lock_path(home.path()).exists());
        assert!(run(home.path(), 0).is_err());
    }

    // Review P3-D: `install relay` is offered only where starting the loop
    // again is the remedy, and a missing receiver names `relay.json` first.
    #[test]
    fn a_refusal_offers_install_relay_only_where_starting_again_helps() {
        let home = tempfile::tempdir().expect("tempdir");
        let held = commonmeasure_runtime::declaration::open_lock(&lock_path(home.path())).unwrap();
        held.lock().unwrap();
        with_receiver(home.path());
        let line = match start(home.path(), 30) {
            Err(refused) => refused.line(home.path()),
            Ok(_) => panic!("took a held lock"),
        };
        assert!(line.contains("another background relay holds"), "{line}");
        assert!(!line.contains("install relay"), "{line}");
        drop(held);

        std::fs::remove_file(home.path().join("relay.json")).unwrap();
        let line = match start(home.path(), 30) {
            Err(refused) => refused.line(home.path()),
            Ok(_) => panic!("started without a receiver"),
        };
        let relay_json = home.path().join("relay.json").display().to_string();
        let named = line.rfind(&relay_json).expect("names relay.json");
        let offered = line.find("install relay").expect("offers install relay");
        assert!(named < offered, "{line}");
        assert!(
            line.contains(&format!(
                "stays stopped until {relay_json} names a receiver"
            )),
            "{line}"
        );
    }

    // Review P2-4: the journal's first line names a scoped receiver.
    #[test]
    fn the_start_line_names_a_receiver_scoped_to_suppliers() {
        let home = tempfile::tempdir().expect("tempdir");
        with_receiver(home.path());
        assert!(!start_line(home.path(), 60).contains("scoped"));
        std::fs::write(
            home.path().join("relay.json"),
            r#"{"receiver":"http://127.0.0.1:9/telemetry","suppliers":["ozone"]}"#,
        )
        .unwrap();
        let said = start_line(home.path(), 60);
        assert!(
            said.ends_with(
                "; the receiver is scoped to suppliers (ozone), so only their events leave and a \
                 source whose licence demands usage reporting is refused on this home"
            ),
            "{said}"
        );
    }

    // Review P3-1: an interval past a day is refused before anything
    // starts, and no interval makes the wait's deadline overflow.
    #[test]
    fn an_interval_is_bounded_and_its_deadline_never_overflows() {
        assert_eq!(check_every(1), Ok(()));
        assert_eq!(check_every(MAX_EVERY_SECS), Ok(()));
        for every in [0, MAX_EVERY_SECS + 1, u64::MAX] {
            let refused = check_every(every).expect_err("out of range");
            assert!(refused.contains("--every"), "{refused}");
        }
        let home = tempfile::tempdir().expect("tempdir");
        with_receiver(home.path());
        assert!(run(home.path(), u64::MAX).is_err());
        assert!(!lock_path(home.path()).exists());
        assert_eq!(stop::deadline(Duration::MAX), None);
        assert!(stop::deadline(Duration::from_secs(MAX_EVERY_SECS)).is_some());
    }

    #[test]
    fn the_wait_lasts_the_interval_without_a_signal() {
        let started = Instant::now();
        assert_eq!(stop::wait(Duration::from_millis(150)), None);
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_this_user_cannot_open_reads_as_unknown() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let home = tempfile::tempdir().expect("tempdir");
        if std::fs::metadata(home.path()).unwrap().uid() == 0 {
            return;
        }
        let path = lock_path(home.path());
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let said = line(home.path());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            said.starts_with("whether one is running cannot be read (cannot open "),
            "{said}"
        );
    }
}
