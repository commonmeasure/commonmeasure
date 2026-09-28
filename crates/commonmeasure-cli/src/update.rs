//! `commonmeasure update`: replace this binary with a newer release.
//!
//! The download, checksum and version checks are the installer's own:
//! `install.sh` is compiled into the binary and run with `sh`, so there is
//! one implementation of the release format, and the script that runs is
//! the one this binary shipped with, never one fetched at update time. The
//! installer downloads with `curl`, which has no size ceiling; the product's
//! own HTTP client refuses bodies over 32 MiB and the binary is close to it.
//!
//! The checksum proves that the downloaded bytes match the `SHA256SUMS`
//! published beside them at the same origin. It does not prove who published
//! either: releases are not signed, and `update` trusts the release origin
//! exactly as the installer run by hand does.
//!
//! Nothing here runs on a timer. The operator runs `update`, and only then
//! does the binary contact the release location.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::processes::{Home, Process, Runs};
use crate::service::{self, BeforeUpdate, Commands, Context, Installed, ServiceName, StopFailed};

const INSTALLER: &str = include_str!("../../../install.sh");

const RELEASES: &str = "https://github.com/commonmeasure/commonmeasure/releases";

#[derive(clap::Args)]
pub struct Update {
    /// Report the installed and latest versions and change nothing.
    #[arg(long)]
    check: bool,
    /// Install this release (`vX.Y.Z`) instead of the latest, including an
    /// older one.
    #[arg(long)]
    tag: Option<String>,
}

pub fn run(args: Update) -> Result<(), String> {
    if !cfg!(unix) {
        return Err(
            "commonmeasure update runs on macOS and Linux. On Windows, download the \
                    release binary again: https://github.com/commonmeasure/commonmeasure/releases"
                .to_string(),
        );
    }
    let releases = origin(
        cfg!(debug_assertions),
        std::env::var("COMMONMEASURE_RELEASE_URL").ok(),
    );
    println!("origin     {releases}");
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or("this binary's own version is not of the form X.Y.Z")?;
    let explicit = args.tag.is_some();
    let version = match &args.tag {
        Some(tag) => Version::tag(tag).ok_or_else(|| {
            format!("--tag {tag} is not a release tag of the form vX.Y.Z; nothing was changed")
        })?,
        None => latest(&releases, current)?,
    };

    if args.check {
        println!("installed  {current}");
        println!("release    {version} ({releases}/tag/v{version})");
        if current < version {
            println!("\nUpdate with: commonmeasure update");
        } else {
            println!("\nNo newer release.");
        }
        return Ok(());
    }
    if !explicit && version <= current {
        println!("commonmeasure {current} is installed; the latest release is {version}.");
        return Ok(());
    }

    let target = target()?;
    let contexts = if cfg!(target_os = "macos") {
        vec![
            (ServiceName::Console, service::context()?),
            (ServiceName::Relay, service::relay_context()?),
        ]
    } else {
        Vec::new()
    };
    let replacement = Replacement {
        releases,
        version,
        current,
        target,
        shell: PathBuf::from("sh"),
        installer: INSTALLER,
        reported_version: &reported_version,
    };
    let services: Vec<(ServiceName, &Context)> = contexts
        .iter()
        .map(|(service, context)| (*service, context))
        .collect();
    replacement.run(&services, &service::System)
}

/// The release location. A debug build honours `COMMONMEASURE_RELEASE_URL`,
/// so the tests serve a release from loopback through the ordinary binary. A
/// release build uses the public repository whatever the environment says,
/// so an inherited variable cannot redirect an update. `install.sh` run by
/// hand still honours the variable.
fn origin(honour_override: bool, value: Option<String>) -> String {
    value
        .filter(|url| honour_override && !url.trim().is_empty())
        .unwrap_or_else(|| RELEASES.to_string())
}

/// A release version: three decimal numbers, each without a sign or a
/// leading zero and within `u64`. Release tags are this with a `v` in front;
/// anything else, including a prerelease or build suffix, is not a release
/// this binary installs or compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split('.').map(|part| {
            let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            let canonical = part == "0" || !part.starts_with('0');
            (digits && canonical).then(|| part.parse::<u64>().ok())?
        });
        let version = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    fn tag(tag: &str) -> Option<Version> {
        Version::parse(tag.strip_prefix('v')?)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version the release location's `latest` redirects to, read without
/// following the redirect, as the installer reads it.
fn latest(releases: &str, current: Version) -> Result<Version, String> {
    let url = format!("{releases}/latest");
    let path = url::Url::parse(&url)
        .map_err(|error| format!("{url}: {error}"))?
        .path()
        .to_string();
    let response = commonmeasure_http::send(&url, commonmeasure_http::Request::get(&path))
        .map_err(|error| format!("find the latest release at {url}: {error:#}"))?;
    let location = response
        .headers
        .get("Location")
        .filter(|_| (300..400).contains(&response.status))
        .ok_or_else(|| {
            format!(
                "find the latest release: {url} answered {} with no redirect to a release",
                response.status
            )
        })?;
    let tag = location
        .rsplit_once("/releases/tag/")
        .map(|(_, tag)| tag)
        .filter(|tag| !tag.contains('/'))
        .ok_or_else(|| {
            format!(
                "find the latest release: {url} redirected to {location}, not a release tag. \
                 Pass --tag vX.Y.Z to install a named release"
            )
        })?;
    Version::tag(tag).ok_or_else(|| {
        format!(
            "the latest release at {url} is tagged {tag}, which is not of the form vX.Y.Z, so it \
             cannot be compared with {current}; nothing was changed. Pass --tag vX.Y.Z to \
             install a named release"
        )
    })
}

/// The file to replace: this binary. A binary reached through a symbolic
/// link belongs to whatever made the link (a package manager), and replacing
/// the link with a file would take it out of that manager's hands. Only
/// macOS reports the invocation path here; Linux's `current_exe` has already
/// resolved the link, so there the check never fires.
fn target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if std::fs::symlink_metadata(&exe)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        let resolved = exe.canonicalize().unwrap_or_else(|_| exe.clone());
        return Err(format!(
            "{} is a link to {}. Update it with whatever installed it.",
            exe.display(),
            resolved.display()
        ));
    }
    if exe.file_name().and_then(|name| name.to_str()) != Some("commonmeasure") {
        return Err(format!(
            "{} is not named commonmeasure, so it was not placed by the installer. Update it \
             the way it was built.",
            exe.display()
        ));
    }
    Ok(exe)
}

/// One replacement of `target` by release `version`.
struct Replacement<'a> {
    releases: String,
    version: Version,
    current: Version,
    target: PathBuf,
    /// The shell the installer is passed to on standard input.
    shell: PathBuf,
    installer: &'a str,
    /// Names the replacement the installer did not confirm:
    /// [`reported_version`], or in tests a callback that sees whether the
    /// console restart was attempted first.
    reported_version: &'a dyn Fn(&Path) -> String,
}

impl Replacement<'_> {
    /// In order: find the managed services (console, then background relay)
    /// and refuse what cannot be stopped and started again as it was; stop
    /// each that runs the target; list the other processes that run it; run
    /// the installer; start each stopped service again on the binary then in
    /// place, whatever the installer's outcome; after every successful
    /// install, the current release's included, name the processes listed,
    /// which keep the release they started with.
    /// Every refusal comes before anything is stopped or downloaded. Other
    /// processes are never a reason to refuse: the installer renames the new
    /// file over the target, and a running process keeps the inode it
    /// started from.
    fn run(
        &self,
        services: &[(ServiceName, &Context)],
        commands: &dyn Commands,
    ) -> Result<(), String> {
        let target = self.target.as_path();
        let mut found = Vec::new();
        for &(service, context) in services {
            let before = service::inspect_for_update(context, commands, service, target)
                .map_err(|error| format!("{error}. Nothing was stopped or installed."))?;
            found.push((service, context, before));
        }

        let old = format!("commonmeasure {}", self.current);
        let mut stopped: Vec<(ServiceName, &Context, Installed)> = Vec::new();
        for (service, context, before) in found {
            let noun = service.noun();
            match before {
                BeforeUpdate::Runs(installed) => {
                    match service::stop_for_update(context, commands, service, &installed) {
                        Ok(()) => {
                            let on = match service {
                                ServiceName::Console => format!(" on {}", installed.listen),
                                ServiceName::Relay => String::new(),
                            };
                            println!("Stopped the {noun}{on} for the update.");
                            stopped.push((service, context, installed));
                        }
                        Err(StopFailed::NotStopped(error)) => {
                            let restarts = start_again(&stopped, commands, &old);
                            return Err(with_restarts(
                                format!(
                                    "stop the {noun} before updating: {error}. Nothing was \
                                     installed. Check it with: commonmeasure service status {}",
                                    service.word()
                                ),
                                &restarts,
                            ));
                        }
                        Err(StopFailed::StillHeld(error)) => {
                            let remedy = match service {
                                // Something else may hold the console's
                                // port; the console is started again at once.
                                ServiceName::Console => {
                                    stopped.push((service, context, installed));
                                    String::new()
                                }
                                // The loop is finishing a run; another loop
                                // would be refused until it exits.
                                ServiceName::Relay => format!(
                                    " Once it has exited, start it again with: {}",
                                    service::reinstall_command(service, &installed)
                                ),
                            };
                            let restarts = start_again(&stopped, commands, &old);
                            return Err(with_restarts(
                                format!(
                                    "stop the {noun} before updating: {error}. Nothing was \
                                     installed.{remedy}"
                                ),
                                &restarts,
                            ));
                        }
                    }
                }
                BeforeUpdate::RunsOther(program) => println!(
                    "The {noun} runs {}, not {}, and is left running.",
                    program.display(),
                    target.display()
                ),
                BeforeUpdate::Absent => {}
            }
        }

        // Listed after the services are stopped, so they are not among the
        // processes left on the old release.
        let listed = listed(target);
        let outcome = self.install();
        let running = match &outcome {
            Outcome::Installed => format!("commonmeasure {}", self.version),
            Outcome::Unchanged(_) => old.clone(),
            Outcome::Replaced(_) => format!("the replacement now at {}", target.display()),
            Outcome::Uncertain(_) => format!("the binary now at {}", target.display()),
        };
        let restarts = start_again(&stopped, commands, &running);
        match outcome {
            Outcome::Installed => {
                if !restarts.is_empty() {
                    return Err(format!(
                        "{} was replaced by commonmeasure {}, but {}",
                        target.display(),
                        self.version,
                        restarts.join("\n")
                    ));
                }
            }
            Outcome::Replaced(error) => {
                // Only now, after the restart was attempted: recovery never
                // waits on the replacement answering, and the answer is
                // bounded in time and size.
                let mut text = format!(
                    "{error}, but {} had already been replaced by {}",
                    target.display(),
                    (self.reported_version)(target),
                );
                if !restarts.is_empty() {
                    text.push_str(&format!(", and {}", restarts.join("\n")));
                }
                return Err(text);
            }
            Outcome::Unchanged(error) | Outcome::Uncertain(error) => {
                return Err(with_restarts(error, &restarts));
            }
        }

        // Printed after a reinstall of the current release too: processes
        // that outlived an earlier update still run the file it replaced.
        print!("{}", still_running(target, self.current, &listed));
        if self.version != self.current {
            println!(
                "\nHosts start the MCP server and hooks from this binary: sessions opened from \
                 now on run {}.",
                self.version
            );
        }
        Ok(())
    }

    /// Run the embedded installer and collect it. The child is waited for on
    /// every path, killed first when it could not be given the script or
    /// waiting for it failed, so no installer is left running when the
    /// service is started again.
    ///
    /// The installer can fail after it has moved the new binary into place,
    /// for instance when it cannot write its last message, so a non-zero
    /// exit is compared with the target's identity taken before it ran.
    fn install(&self) -> Outcome {
        let tag = format!("v{}", self.version);
        let target = self.target.display();
        let Some(dir) = self.target.parent() else {
            return Outcome::Unchanged(format!("{target} has no directory"));
        };
        let before = identity(&self.target);
        let spawned = std::process::Command::new(&self.shell)
            .args(["-s", "--", "--tag", &tag, "--update", "--dir"])
            .arg(dir)
            // The origin printed above, and no other: in a release build this
            // replaces whatever the environment inherited.
            .env("COMMONMEASURE_RELEASE_URL", &self.releases)
            .stdin(std::process::Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                return Outcome::Unchanged(format!(
                    "update to {tag} failed: run {}: {error}; {target} is unchanged",
                    self.shell.display()
                ));
            }
        };
        let written = {
            use std::io::Write as _;
            let mut stdin = child.stdin.take().expect("stdin is piped");
            stdin.write_all(self.installer.as_bytes())
        };
        if let Err(error) = written {
            let _ = child.kill();
            let _ = child.wait();
            return Outcome::Uncertain(format!(
                "update to {tag} failed: pass the installer to {}: {error}. The installer was \
                 stopped; check {target} with: {target} --version",
                self.shell.display()
            ));
        }
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome::Uncertain(format!(
                    "update to {tag} failed: wait for the installer: {error}. Check {target} \
                     with: {target} --version"
                ));
            }
        };
        if status.success() {
            return Outcome::Installed;
        }
        // The installer has already said why on stderr.
        let failed = format!("update to {tag} failed");
        match (before, identity(&self.target)) {
            (Ok(before), Ok(after)) if before == after => {
                Outcome::Unchanged(format!("{failed}; {target} is unchanged"))
            }
            (Ok(_), Ok(_)) => Outcome::Replaced(format!(
                "{failed} (the installer exited {})",
                status
                    .code()
                    .map_or("on a signal".to_string(), |code| format!(
                        "with status {code}"
                    ))
            )),
            (Err(error), _) | (_, Err(error)) => Outcome::Uncertain(format!(
                "{failed}, and update cannot tell whether {target} was replaced ({error}). Check \
                 it with: {target} --version"
            )),
        }
    }
}

/// How a run of the installer left the target.
enum Outcome {
    Installed,
    /// It failed, and the target is the file it was before.
    Unchanged(String),
    /// It failed after the target was replaced. The new binary's version
    /// is asked for only after the console service's restart was attempted.
    Replaced(String),
    /// It failed, and whether the target was replaced is not known.
    Uncertain(String),
}

/// What identifies the file at `path`: device, inode, size and SHA-256.
/// The installer replaces the target by renaming a new file over it, which
/// changes the inode; the size and digest also catch a rewrite in place.
///
/// This is a sequential check, not an atomic snapshot. After hashing, the
/// open file is described again and the path looked up again: when the
/// file's metadata moved during the read, or the path names another file
/// than the one read, the identity is an error, which the caller reports
/// as not known. A writer that changes the file and restores it between
/// those reads is not seen; nothing here locks the file against other
/// writers (`ARCHITECTURE.md` §What runs where).
#[cfg(unix)]
fn identity(path: &Path) -> Result<(u64, u64, u64, [u8; 32]), String> {
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;
    let read = |error: std::io::Error| format!("{}: {error}", path.display());
    // Everything that changes when the file is written, replaced or renamed.
    let stamp = |metadata: &std::fs::Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            (metadata.mtime(), metadata.mtime_nsec()),
            (metadata.ctime(), metadata.ctime_nsec()),
        )
    };
    let mut file = std::fs::File::open(path).map_err(read)?;
    let metadata = file.metadata().map_err(read)?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0; 1 << 16];
    loop {
        match file.read(&mut buffer).map_err(read)? {
            0 => break,
            count => digest.update(&buffer[..count]),
        }
        #[cfg(test)]
        if let Some(meanwhile) = WHILE_HASHING.take() {
            meanwhile();
        }
    }
    let after = file.metadata().map_err(read)?;
    let named = std::fs::metadata(path).map_err(read)?;
    if stamp(&after) != stamp(&metadata) {
        return Err(format!("{}: it changed while it was read", path.display()));
    }
    if (named.dev(), named.ino()) != (after.dev(), after.ino()) {
        return Err(format!(
            "{}: it was replaced while it was read",
            path.display()
        ));
    }
    let sha256 = digest
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes");
    Ok((metadata.dev(), metadata.ino(), metadata.size(), sha256))
}

#[cfg(test)]
thread_local! {
    /// Run once by [`identity`] on this thread after its first read, so a
    /// test can change the file or its path at a known point of the hash.
    static WHILE_HASHING: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(not(unix))]
fn identity(path: &Path) -> Result<(u64, u64, u64, [u8; 32]), String> {
    Err(format!(
        "{}: no file identity on this system",
        path.display()
    ))
}

/// How long `--version` may take, and how much of its output is kept, when
/// another binary is asked for its version: the replacement the installer
/// did not confirm, or the `commonmeasure` on PATH that `service status`
/// names. Such a binary may not answer, or may not stop printing.
///
/// The binary asked may be freshly placed, and macOS checks a file on its
/// first launch. On an idle Mac a debug `commonmeasure` copied to a new path took
/// 0.6 s to answer its first `--version` and 0.01 s its second; under heavy
/// load (load average 40 on 10 cores) the first took up to 5.7 s, and a
/// two-line shell script up to 6.7 s. Five seconds would report such a
/// binary's version as unreadable, so the bound is 15 s. A binary that
/// hangs holds `service status`, or the error of an `update` whose
/// installer failed, for up to that long; one that answers is not waited
/// for.
pub(crate) const VERSION_WAIT: Duration = Duration::from_secs(15);
const VERSION_BYTES: usize = 4096;

/// The version the binary at `path` reports, for naming a replacement the
/// installer did not confirm, asked within [`VERSION_WAIT`].
fn reported_version(path: &Path) -> String {
    reported_version_within(path, VERSION_WAIT)
}

/// [`reported_version`] asked within `wait`. When the version cannot be
/// read the message says why and gives the command that shows it.
fn reported_version_within(path: &Path, wait: Duration) -> String {
    version_within(path, wait).unwrap_or_else(|why| {
        format!(
            "a binary whose version could not be read ({why}); check it with: {} --version",
            path.display()
        )
    })
}

/// What `path --version` prints, trimmed, or why it could not be read.
/// Bounded by `wait` ([`VERSION_WAIT`] outside tests) and [`VERSION_BYTES`],
/// with standard error discarded (see [`bounded`]).
pub(crate) fn version_within(path: &Path, wait: Duration) -> Result<String, String> {
    let mut command = std::process::Command::new(path);
    command.arg("--version");
    match bounded(&mut command, wait, VERSION_BYTES) {
        Ok(Bounded::Exited(status, stdout)) if status.success() => {
            match String::from_utf8_lossy(&stdout).trim() {
                "" => Err("it printed nothing".to_string()),
                version => Ok(version.to_string()),
            }
        }
        Ok(Bounded::Exited(status, _)) => Err(format!("--version exited with {status}")),
        Ok(Bounded::TimedOut) => Err(format!(
            "--version did not finish within {} s and was stopped",
            wait.as_secs()
        )),
        Ok(Bounded::TooLong) => Err(format!(
            "--version printed more than {VERSION_BYTES} bytes and was stopped"
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// How a [`bounded`] run ended.
#[derive(Debug)]
enum Bounded {
    /// It exited within the time, having printed at most the limit.
    Exited(std::process::ExitStatus, Vec<u8>),
    /// It was stopped at the time limit.
    TimedOut,
    /// It was stopped for printing more than the limit.
    TooLong,
}

/// Run `command` with no input, keeping at most `limit` bytes of standard
/// output and discarding standard error. Past `wait`, or past `limit`, the
/// child and the processes it started in its group are killed, and the
/// child alone is waited for. An exit within both, zero or not, kills
/// nothing. The thread reading the output is not joined: a process that left
/// the group could hold the pipe open, and the thread ends when it closes.
/// Such a process is not killed either: cleanup reaches the child's process
/// group only. `wait` runs from before the spawn and bounds waiting for the
/// output and the exit; the spawn and the final wait for the killed child
/// are system calls it does not bound.
fn bounded(
    command: &mut std::process::Command,
    wait: Duration,
    limit: usize,
) -> std::io::Result<Bounded> {
    use std::io::Read as _;
    use std::process::Stdio;
    let deadline = Instant::now() + wait;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(read);
    });
    let ended = match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) if bytes.len() > limit => Ok(Bounded::TooLong),
        Ok(Ok(bytes)) => loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Bounded::Exited(status, bytes)),
                Ok(None) if Instant::now() >= deadline => break Ok(Bounded::TimedOut),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => break Err(error),
            }
        },
        Ok(Err(error)) => Err(error),
        Err(_) => Ok(Bounded::TimedOut),
    };
    stop(&mut child);
    ended
}

/// Kill `child`, and the processes it started in the group it leads, and
/// wait for it.
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill takes no pointers. The group is the one the child was
        // spawned to lead, and the child has not been waited for, so its id
        // still names that group and no other process.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The processes of this user, other than this one, that run `target`,
/// each with its subcommand and `COMMONMEASURE_HOME` and never its other
/// arguments or environment, which can carry credentials; or why the
/// process table could not be read.
fn listed(target: &Path) -> Result<Vec<Process>, String> {
    use clap::CommandFactory as _;
    let subcommands: Vec<String> = crate::Cli::command()
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .collect();
    crate::processes::running(target, &subcommands)
}

/// What `update` prints after replacing `target`, which ran release
/// `current`, about the processes `listed` found running it: they keep the
/// binary they started with until their host starts them again. That is
/// `current`, or for a process that outlived an earlier update, the release
/// installed before it; the listing does not tell them apart. Empty when
/// none was found.
fn still_running(target: &Path, current: Version, listed: &Result<Vec<Process>, String>) -> String {
    let processes = match listed {
        Ok(processes) if processes.is_empty() => return String::new(),
        Ok(processes) => processes,
        Err(error) => {
            return format!(
                "\nThe processes running {} could not be listed ({error}); any that run it keep \
                 commonmeasure {current}, or a release installed before it, until their host \
                 restarts them.\n",
                target.display()
            );
        }
    };
    let mut text = format!(
        "\nThese processes run commonmeasure {current}, or a release installed before it, until \
         their host restarts them:\n"
    );
    for process in processes {
        let name = match &process.runs {
            Runs::Unidentified { name } => {
                format!("  process name {name}, its executable could not be read")
            }
            Runs::File => String::new(),
        };
        let subcommand = process
            .subcommand
            .as_deref()
            .map(|name| format!("  {name}"))
            .unwrap_or_default();
        let home = match &process.home {
            Home::Set(home) => format!("COMMONMEASURE_HOME={home}"),
            Home::Default => "COMMONMEASURE_HOME not set or empty".to_string(),
            Home::Unknown => "COMMONMEASURE_HOME unknown".to_string(),
        };
        text.push_str(&format!(
            "  pid {}{name}{subcommand}  {home}\n",
            process.pid
        ));
    }
    text
}

/// Start again each service in `stopped`, as it was installed, now that
/// the target holds `running`. Returns an error for each that did not
/// start, naming the command that starts it with the same Edge home.
fn start_again(
    stopped: &[(ServiceName, &Context, Installed)],
    commands: &dyn Commands,
    running: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (service, context, installed) in stopped {
        let noun = service.noun();
        match service::start_after_update(context, commands, *service, installed) {
            Ok(text) => {
                println!("\nThe {noun} was started again on {running}:");
                print!("{text}");
            }
            Err(error) => errors.push(format!(
                "the {noun} did not start again: {error}\nStart it with: {}",
                service::reinstall_command(*service, installed)
            )),
        }
    }
    errors
}

/// `error`, then each restart that failed, one to a line.
fn with_restarts(error: String, restarts: &[String]) -> String {
    std::iter::once(error)
        .chain(restarts.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::testing::{Recorder, context, printed, printed_relay, with_home};
    use commonmeasure_http::{Request, Response, Server, ServerHandle};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    #[test]
    fn versions_parse_strictly_and_compare_by_number() {
        let v = |text| Version::parse(text);
        assert!(v("0.4.0") < v("0.4.1"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert_eq!(v("0.4.0"), Some(Version(0, 4, 0)));
        assert_eq!(v("18446744073709551615.0.0"), Some(Version(u64::MAX, 0, 0)));
        for bad in [
            "0.4.1-rc.1",
            "0.4.1+build",
            "0.4",
            "0.4.1.2",
            "01.4.1",
            "0.4.",
            "-1.0.0",
            "+1.0.0",
            " 0.4.1",
            "18446744073709551616.0.0",
            "banana",
        ] {
            assert_eq!(v(bad), None, "{bad}");
        }
        assert_eq!(Version::tag("v0.4.1"), Some(Version(0, 4, 1)));
        assert_eq!(Version::tag("0.4.1"), None);
        assert_eq!(Version::tag("vbanana"), None);
        assert_eq!(Version(0, 4, 1).to_string(), "0.4.1");
    }

    #[test]
    fn only_a_debug_build_honours_the_release_url_override() {
        let mirror = Some("http://127.0.0.1:9/r".to_string());
        assert_eq!(origin(true, mirror.clone()), "http://127.0.0.1:9/r");
        assert_eq!(origin(false, mirror), RELEASES);
        assert_eq!(origin(true, Some("  ".into())), RELEASES);
        assert_eq!(origin(true, None), RELEASES);
    }

    #[test]
    fn the_embedded_installer_takes_the_update_flag() {
        assert!(INSTALLER.contains("--update) update=1"));
    }

    // The updater run end to end on a temporary home: the real installer
    // passed to the real shell, downloading with curl from a loopback origin
    // shaped like a release. Only launchd is replaced, by the recorder, which
    // stands in for the service manager and for the console it would start.

    const LAUNCHCTL_PRINT: &str = "launchctl print gui/501/ai.commonmeasure.console";

    /// How long a test that expects a binary to answer lets it take. The
    /// first launch of a freshly written script on a loaded host has taken
    /// longer than [`VERSION_WAIT`]; a test of the bound passes its own.
    const ANSWER_WAIT: Duration = Duration::from_secs(30);

    /// [`reported_version`] as the tests ask it, within [`ANSWER_WAIT`].
    fn reported_generously(path: &Path) -> String {
        reported_version_within(path, ANSWER_WAIT)
    }

    fn asset() -> &'static str {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "commonmeasure-linux-x64",
            ("linux", "aarch64") => "commonmeasure-linux-arm64",
            ("macos", "aarch64") => "commonmeasure-darwin-arm64",
            ("macos", "x86_64") => "commonmeasure-darwin-x64",
            other => panic!("the release has no binary for {other:?}"),
        }
    }

    /// Each request's path, with the launchd calls made by the time it
    /// arrived.
    type Requests = Arc<Mutex<Vec<(String, Vec<String>)>>>;

    /// A release origin serving `tag`, whose binary is a script reporting
    /// `reports`.
    struct Origin {
        _server: ServerHandle,
        releases: String,
        requests: Requests,
    }

    impl Origin {
        fn new(
            tag: &str,
            reports: &str,
            sum_matches: bool,
            launchd: Arc<Mutex<Vec<String>>>,
        ) -> Self {
            let binary = format!("#!/bin/sh\necho \"commonmeasure {reports}\"\n").into_bytes();
            Origin::serving(tag, binary, sum_matches, launchd)
        }

        /// A release origin serving `tag`, whose binary is `binary`.
        fn serving(
            tag: &str,
            binary: Vec<u8>,
            sum_matches: bool,
            launchd: Arc<Mutex<Vec<String>>>,
        ) -> Self {
            let sum: String = Sha256::digest(if sum_matches { &binary[..] } else { b"other" })
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let sums = format!("{sum}  {}\n", asset());
            let server = Server::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", server.local_addr().unwrap());
            let requests = Requests::default();
            let log = Arc::clone(&requests);
            let tag = tag.to_string();
            let origin = url.clone();
            let server = server
                .spawn(move |request: Request| {
                    let path = request
                        .target
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    log.lock()
                        .unwrap()
                        .push((path.clone(), launchd.lock().unwrap().clone()));
                    if path == "/r/latest" {
                        let mut response = Response::new(302, Vec::new());
                        response
                            .headers
                            .set("Location", &format!("{origin}/r/tag/{tag}"));
                        response
                    } else if path == format!("/r/download/{tag}/SHA256SUMS") {
                        Response::new(200, sums.clone().into_bytes())
                    } else if path == format!("/r/download/{tag}/{}", asset()) {
                        Response::new(200, binary.clone())
                    } else {
                        Response::text(404, "no such route")
                    }
                })
                .unwrap();
            Origin {
                _server: server,
                releases: format!("{url}/r"),
                requests,
            }
        }

        fn downloads(&self) -> Vec<(String, Vec<String>)> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(path, _)| path.contains("/download/"))
                .cloned()
                .collect()
        }
    }

    /// A home with a binary at `bin/commonmeasure` and a console service
    /// installed for Edge home `edge-a`, loaded and running it on a free
    /// port. Holds the contexts of a shell that selects `edge-b`.
    struct Home {
        dir: tempfile::TempDir,
        shell: Context,
        /// The background relay's context in the same shell.
        relay_shell: Context,
        installed_plist: String,
        print: String,
        port: u16,
    }

    /// The background relay service [`Home::with_relay`] installs: for Edge
    /// home `edge-a`, every 300 s, loaded and running.
    struct Relay {
        installed_plist: String,
        print: String,
        /// The Edge home it relays, with a receiver in `relay.json`.
        home: PathBuf,
    }

    const RELAY_PRINT: &str = "launchctl print gui/501/ai.commonmeasure.relay";

    impl Home {
        fn new() -> Home {
            let dir = tempfile::tempdir().unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let listen = format!("127.0.0.1:{port}");
            let installer = with_home(context(dir.path()), &dir.path().join("edge-a"));
            std::fs::write(&installer.exe, "#!/bin/sh\necho \"commonmeasure 0.0.0\"\n").unwrap();
            let plist_path = crate::service::plist_path(dir.path(), crate::service::LABEL);
            std::fs::create_dir_all(plist_path.parent().unwrap()).unwrap();
            let installed_plist = crate::service::plist(&installer, &listen);
            std::fs::write(&plist_path, &installed_plist).unwrap();
            let print = printed(&installer, &listen, 7);
            let shell = with_home(context(dir.path()), &dir.path().join("edge-b"));
            std::fs::write(&shell.exe, "#!/bin/sh\necho \"commonmeasure 0.0.0\"\n").unwrap();
            let relay_shell = Context {
                label: crate::service::RELAY_LABEL.to_string(),
                log: dir.path().join("edge-b/logs/relay.log"),
                ..with_home(context(dir.path()), &dir.path().join("edge-b"))
            };
            std::fs::write(
                &relay_shell.exe,
                "#!/bin/sh\necho \"commonmeasure 0.0.0\"\n",
            )
            .unwrap();
            Home {
                dir,
                shell,
                relay_shell,
                installed_plist,
                print,
                port,
            }
        }

        /// Both managed services as `update` is given them. A relay that is
        /// not installed is not loaded, as the recorder answers.
        fn services(&self) -> [(ServiceName, &Context); 2] {
            [
                (ServiceName::Console, &self.shell),
                (ServiceName::Relay, &self.relay_shell),
            ]
        }

        /// Install the background relay service too.
        fn with_relay(&self) -> Relay {
            let edge = self.dir.path().join("edge-a");
            let installer = Context {
                label: crate::service::RELAY_LABEL.to_string(),
                log: edge.join("logs/relay.log"),
                ..with_home(context(self.dir.path()), &edge)
            };
            std::fs::write(&installer.exe, "#!/bin/sh\necho \"commonmeasure 0.0.0\"\n").unwrap();
            std::fs::create_dir_all(&edge).unwrap();
            std::fs::write(
                edge.join("relay.json"),
                r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
            )
            .unwrap();
            let installed_plist = crate::service::relay_plist(&installer, 300);
            std::fs::write(self.relay_plist_path(), &installed_plist).unwrap();
            Relay {
                installed_plist,
                print: printed_relay(&installer, 300, 8),
                home: edge,
            }
        }

        fn relay_plist_path(&self) -> PathBuf {
            crate::service::plist_path(self.dir.path(), crate::service::RELAY_LABEL)
        }

        /// [`Home::launchd`] with the relay loaded too: its print, bootout,
        /// bootstrap and kickstart succeed, and kickstart starts a loop that
        /// holds the relay's Edge home.
        fn launchd_with(&self, relay: &Relay) -> Recorder {
            let label = crate::service::RELAY_LABEL;
            self.launchd()
                .answer(RELAY_PRINT, true, &relay.print)
                .answer(&format!("launchctl bootout gui/501/{label}"), true, "")
                .answer(
                    &format!(
                        "launchctl bootstrap gui/501 {}",
                        self.relay_plist_path().display()
                    ),
                    true,
                    "",
                )
                .answer(&format!("launchctl kickstart gui/501/{label}"), true, "")
                .relay_on(&format!("launchctl kickstart gui/501/{label}"), &relay.home)
        }

        fn relay_plist(&self) -> String {
            std::fs::read_to_string(self.relay_plist_path()).unwrap()
        }

        /// The launchd a loaded service meets: print, bootout, bootstrap and
        /// kickstart succeed, and kickstart brings a console up.
        fn launchd(&self) -> Recorder {
            let plist = crate::service::plist_path(self.dir.path(), crate::service::LABEL);
            Recorder::default()
                .answer(LAUNCHCTL_PRINT, true, &self.print)
                .answer(
                    "launchctl bootout gui/501/ai.commonmeasure.console",
                    true,
                    "",
                )
                .answer(
                    &format!("launchctl bootstrap gui/501 {}", plist.display()),
                    true,
                    "",
                )
                .answer(
                    "launchctl kickstart gui/501/ai.commonmeasure.console",
                    true,
                    "",
                )
                .console_on(
                    "launchctl kickstart gui/501/ai.commonmeasure.console",
                    self.port,
                )
        }

        fn replacement(&self, origin: &Origin, version: Version) -> Replacement<'static> {
            Replacement {
                releases: origin.releases.clone(),
                version,
                current: Version(0, 0, 0),
                target: self.shell.exe.clone(),
                shell: PathBuf::from("sh"),
                installer: INSTALLER,
                reported_version: &reported_generously,
            }
        }

        fn plist(&self) -> String {
            std::fs::read_to_string(crate::service::plist_path(
                self.dir.path(),
                crate::service::LABEL,
            ))
            .unwrap()
        }

        fn binary(&self) -> String {
            std::fs::read_to_string(&self.shell.exe).unwrap()
        }
    }

    fn restarted(calls: &[String]) -> bool {
        restarted_as(calls, crate::service::LABEL)
    }

    /// Whether the service at `label` was booted out, bootstrapped and
    /// kickstarted, in that order.
    fn restarted_as(calls: &[String], label: &str) -> bool {
        let calls: Vec<&String> = calls.iter().filter(|call| call.contains(label)).collect();
        let at = |prefix: &str| calls.iter().position(|call| call.starts_with(prefix));
        matches!(
            (at("launchctl bootout"), at("launchctl bootstrap"), at("launchctl kickstart")),
            (Some(stop), Some(load), Some(start)) if stop < load && load < start
        )
    }

    #[test]
    fn the_service_is_stopped_before_the_first_download_and_started_again_as_installed() {
        let home = Home::new();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        home.replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap();

        let downloads = origin.downloads();
        let (first, calls_then) = downloads.first().expect("the release was downloaded");
        assert!(first.ends_with("/SHA256SUMS"), "{first}");
        assert_eq!(
            calls_then.as_slice(),
            [
                LAUNCHCTL_PRINT.to_string(),
                RELAY_PRINT.to_string(),
                "launchctl bootout gui/501/ai.commonmeasure.console".to_string()
            ],
            "the service was stopped, and not yet started, when the origin first served bytes"
        );
        assert!(restarted(&launchd.calls()), "{:?}", launchd.calls());
        assert!(home.binary().contains("commonmeasure 9.9.9"));
        // Started again with Edge home A, though the shell selected B.
        assert_eq!(home.plist(), home.installed_plist);
        assert!(home.plist().contains("edge-a"));
    }

    #[test]
    fn a_stop_that_fails_downloads_nothing_and_replaces_nothing() {
        let home = Home::new();
        let launchd = Recorder::default()
            .answer(LAUNCHCTL_PRINT, true, &home.print)
            .fail(
                "launchctl bootout gui/501/ai.commonmeasure.console",
                Some(5),
                "Boot-out failed: 5: Input/output error",
            );
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        assert!(error.contains("Nothing was installed"), "{error}");
        assert!(error.contains("Input/output error"), "{error}");
        assert!(origin.requests.lock().unwrap().is_empty());
        assert!(home.binary().contains("commonmeasure 0.0.0"));
        assert!(
            !launchd
                .calls()
                .iter()
                .any(|call| call.contains("bootstrap"))
        );
    }

    #[test]
    fn a_service_that_cannot_be_started_again_as_installed_stops_nothing() {
        let home = Home::new();
        std::fs::remove_file(crate::service::plist_path(
            home.dir.path(),
            crate::service::LABEL,
        ))
        .unwrap();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        assert!(
            error.contains("Nothing was stopped or installed"),
            "{error}"
        );
        assert_eq!(launchd.calls(), vec![LAUNCHCTL_PRINT.to_string()]);
        assert!(origin.requests.lock().unwrap().is_empty());
        assert!(home.binary().contains("commonmeasure 0.0.0"));
    }

    #[test]
    fn an_installer_failure_starts_the_service_again_on_the_old_binary_with_its_home() {
        let home = Home::new();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", false, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        assert!(error.contains("is unchanged"), "{error}");
        assert!(restarted(&launchd.calls()), "{:?}", launchd.calls());
        assert!(home.binary().contains("commonmeasure 0.0.0"));
        assert_eq!(home.plist(), home.installed_plist);
    }

    #[test]
    fn the_service_is_started_again_when_the_installer_cannot_run_to_the_end() {
        // Fault injection on the shell the installer is passed to: it does
        // not exist; it exits at once without reading; it exits without
        // reading while the updater is still writing a script larger than
        // the pipe holds.
        let large = "#".repeat(1 << 20);
        let cases: [(&str, &str, &str); 3] = [
            ("/nonexistent/sh", INSTALLER, "run /nonexistent/sh"),
            ("/usr/bin/false", INSTALLER, "update to v9.9.9 failed"),
            (
                "/usr/bin/false",
                &large,
                "pass the installer to /usr/bin/false",
            ),
        ];
        for (shell, script, expected) in cases {
            let home = Home::new();
            let launchd = home.launchd();
            let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
            let replacement = Replacement {
                shell: PathBuf::from(shell),
                installer: script,
                ..home.replacement(&origin, Version(9, 9, 9))
            };
            let error = replacement.run(&home.services(), &launchd).unwrap_err();
            assert!(error.contains(expected), "{shell}: {error}");
            assert!(
                restarted(&launchd.calls()),
                "{shell}: {:?}",
                launchd.calls()
            );
            assert!(home.binary().contains("commonmeasure 0.0.0"), "{shell}");
            assert_eq!(home.plist(), home.installed_plist, "{shell}");
            assert!(origin.downloads().is_empty(), "{shell}");
        }
    }

    #[test]
    fn a_restart_that_fails_fails_the_update_and_names_the_command_with_the_home() {
        let home = Home::new();
        let plist = crate::service::plist_path(home.dir.path(), crate::service::LABEL);
        let launchd = Recorder::default()
            .answer(LAUNCHCTL_PRINT, true, &home.print)
            .answer(
                "launchctl bootout gui/501/ai.commonmeasure.console",
                true,
                "",
            )
            .fail(
                &format!("launchctl bootstrap gui/501 {}", plist.display()),
                Some(5),
                "Bootstrap failed: 5: Input/output error",
            );
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        // Both outcomes: the binary was replaced, the console was not
        // started again.
        assert!(
            error.contains(&format!(
                "{} was replaced by commonmeasure 9.9.9, but the console service did not start \
                 again",
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(!error.contains("unchanged"), "{error}");
        assert!(
            error.contains(&format!(
                "Start it with: env COMMONMEASURE_HOME={}",
                home.dir.path().join("edge-a").display()
            )),
            "{error}"
        );
        assert!(home.binary().contains("commonmeasure 9.9.9"));
    }

    #[test]
    fn the_relay_service_is_stopped_before_the_first_download_and_started_again_as_installed() {
        let home = Home::new();
        let relay = home.with_relay();
        let launchd = home.launchd_with(&relay);
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        home.replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap();

        let downloads = origin.downloads();
        let (_, calls_then) = downloads.first().expect("the release was downloaded");
        assert_eq!(
            calls_then.as_slice(),
            [
                LAUNCHCTL_PRINT.to_string(),
                RELAY_PRINT.to_string(),
                "launchctl bootout gui/501/ai.commonmeasure.console".to_string(),
                "launchctl bootout gui/501/ai.commonmeasure.relay".to_string(),
            ],
            "both services were stopped, and neither started, when the origin first served bytes"
        );
        let calls = launchd.calls();
        assert!(restarted(&calls), "{calls:?}");
        assert!(
            restarted_as(&calls, crate::service::RELAY_LABEL),
            "{calls:?}"
        );
        assert!(home.binary().contains("commonmeasure 9.9.9"));
        // Started again with Edge home A, though the shell selected B.
        assert_eq!(home.relay_plist(), relay.installed_plist);
        assert!(home.relay_plist().contains("edge-a"));
        assert_eq!(
            commonmeasure_harness::delivery::relay_loop_state(&relay.home),
            commonmeasure_harness::delivery::LockState::Running
        );
    }

    #[test]
    fn an_installer_failure_starts_the_relay_service_again_on_the_old_binary_with_its_home() {
        let home = Home::new();
        let relay = home.with_relay();
        let launchd = home.launchd_with(&relay);
        let origin = Origin::new("v9.9.9", "9.9.9", false, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        assert!(error.contains("is unchanged"), "{error}");
        let calls = launchd.calls();
        assert!(restarted(&calls), "{calls:?}");
        assert!(
            restarted_as(&calls, crate::service::RELAY_LABEL),
            "{calls:?}"
        );
        assert!(home.binary().contains("commonmeasure 0.0.0"));
        assert_eq!(home.relay_plist(), relay.installed_plist);
    }

    #[test]
    fn a_relay_restart_that_fails_fails_the_update_and_names_the_command_with_the_home() {
        let home = Home::new();
        let relay = home.with_relay();
        let label = crate::service::RELAY_LABEL;
        let launchd = home
            .launchd()
            .answer(RELAY_PRINT, true, &relay.print)
            .answer(&format!("launchctl bootout gui/501/{label}"), true, "")
            .fail(
                &format!(
                    "launchctl bootstrap gui/501 {}",
                    home.relay_plist_path().display()
                ),
                Some(5),
                "Bootstrap failed: 5: Input/output error",
            );
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        // The binary was replaced and the console started again; the relay
        // was not.
        assert!(
            error.contains(&format!(
                "{} was replaced by commonmeasure 9.9.9, but the background relay service did \
                 not start again",
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(!error.contains("console service did not"), "{error}");
        assert!(
            error.contains(&format!(
                "Start it with: env COMMONMEASURE_HOME={} {} service install relay --every 300",
                relay.home.display(),
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(restarted(&launchd.calls()), "{:?}", launchd.calls());
        assert!(home.binary().contains("commonmeasure 9.9.9"));
    }

    #[test]
    fn a_relay_loop_that_keeps_its_lock_stops_the_update_and_the_console_starts_again() {
        // The loop finishes the run in progress after SIGTERM; one still
        // holding the lock when the wait ends leaves nothing installed.
        let home = Home::new();
        let relay = home.with_relay();
        let launchd = home.launchd_with(&relay);
        let held = commonmeasure_runtime::declaration::open_lock(&crate::relay_loop::lock_path(
            &relay.home,
        ))
        .unwrap();
        held.lock().unwrap();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let error = home
            .replacement(&origin, Version(9, 9, 9))
            .run(&home.services(), &launchd)
            .unwrap_err();
        drop(held);
        assert!(
            error.contains("the background relay service was stopped but its loop still holds"),
            "{error}"
        );
        assert!(error.contains("Nothing was installed"), "{error}");
        assert!(
            error.contains(&format!(
                "Once it has exited, start it again with: env COMMONMEASURE_HOME={} {} service \
                 install relay --every 300",
                relay.home.display(),
                home.shell.exe.display()
            )),
            "{error}"
        );
        let calls = launchd.calls();
        assert!(
            restarted(&calls),
            "the console was started again: {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|call| call.contains("bootstrap") && call.contains("relay")),
            "{calls:?}"
        );
        assert!(origin.requests.lock().unwrap().is_empty());
        assert!(home.binary().contains("commonmeasure 0.0.0"));
    }

    /// A shell that runs the installer, then runs `after` with `$target` set
    /// to the file being replaced.
    fn wrapper(home: &Home, name: &str, run: &str, after: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = home.dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor last; do :; done\ntarget=\"$last/commonmeasure\"\n{run}\n\
                 status=$?\n{after}\nexit $status\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn an_installer_that_fails_after_placing_the_binary_names_the_replacement() {
        // The reviewer's probe: the installer's standard output closed, so it
        // moves the new binary into place and then fails on its last echo.
        let home = Home::new();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let replacement = Replacement {
            shell: wrapper(&home, "closed-stdout", "/bin/sh \"$@\" >&-", ""),
            ..home.replacement(&origin, Version(9, 9, 9))
        };
        let error = replacement.run(&home.services(), &launchd).unwrap_err();
        assert!(!error.contains("unchanged"), "{error}");
        assert!(
            error.contains(&format!(
                "update to v9.9.9 failed (the installer exited with status 1), but {} had already \
                 been replaced by commonmeasure 9.9.9",
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(home.binary().contains("commonmeasure 9.9.9"));
        assert!(restarted(&launchd.calls()), "{:?}", launchd.calls());
        assert_eq!(home.plist(), home.installed_plist);
    }

    #[test]
    fn an_installer_failure_that_leaves_the_binary_unreadable_is_uncertain() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = Home::new();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let replacement = Replacement {
            shell: wrapper(
                &home,
                "unreadable",
                "/bin/sh \"$@\"",
                "chmod 000 \"$target\"; status=1",
            ),
            ..home.replacement(&origin, Version(9, 9, 9))
        };
        let error = replacement.run(&home.services(), &launchd).unwrap_err();
        std::fs::set_permissions(&home.shell.exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!error.contains("unchanged"), "{error}");
        assert!(!error.contains("was replaced by"), "{error}");
        assert!(
            error.contains(&format!(
                "update cannot tell whether {} was replaced",
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(
            error.contains(&format!(
                "Check it with: {} --version",
                home.shell.exe.display()
            )),
            "{error}"
        );
        assert!(restarted(&launchd.calls()), "{:?}", launchd.calls());
    }

    #[test]
    fn the_replacement_is_asked_for_its_version_only_after_the_restart_was_attempted() {
        // The installer fails after placing the new binary (standard output
        // closed), so update names the replacement. An injected callback
        // stands in for asking it and records what launchd had been asked by
        // then: the restart, whether it succeeds or fails.
        for name in ["starts", "fails"] {
            let home = Home::new();
            let plist = crate::service::plist_path(home.dir.path(), crate::service::LABEL);
            let launchd = match name {
                "starts" => home.launchd(),
                _ => Recorder::default()
                    .answer(LAUNCHCTL_PRINT, true, &home.print)
                    .answer(
                        "launchctl bootout gui/501/ai.commonmeasure.console",
                        true,
                        "",
                    )
                    .fail(
                        &format!("launchctl bootstrap gui/501 {}", plist.display()),
                        Some(5),
                        "Bootstrap failed: 5: Input/output error",
                    ),
            };
            let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
            let seen = Mutex::new(Vec::new());
            let ask = |_: &Path| {
                seen.lock().unwrap().push(launchd.calls());
                "commonmeasure 9.9.9 (as the callback reports it)".to_string()
            };
            let replacement = Replacement {
                shell: wrapper(&home, "closed-stdout", "/bin/sh \"$@\" >&-", ""),
                reported_version: &ask,
                ..home.replacement(&origin, Version(9, 9, 9))
            };
            let error = replacement.run(&home.services(), &launchd).unwrap_err();
            assert!(
                error.contains("had already been replaced by commonmeasure 9.9.9 (as the callback"),
                "{name}: {error}"
            );
            let seen = seen.into_inner().unwrap();
            assert_eq!(seen.len(), 1, "{name}: asked once");
            let bootstrap = seen[0]
                .iter()
                .position(|call| call.starts_with("launchctl bootstrap"));
            let bootout = seen[0]
                .iter()
                .position(|call| call.starts_with("launchctl bootout"));
            assert!(
                matches!((bootout, bootstrap), (Some(stop), Some(start)) if stop < start),
                "{name}: the restart was attempted before the version was asked: {:?}",
                seen[0]
            );
            if name == "starts" {
                assert!(restarted(&seen[0]), "{name}: {:?}", seen[0]);
            } else {
                assert!(error.contains("did not start again"), "{name}: {error}");
            }
        }
    }

    /// A script at `dir/name` that runs `body` when asked for its version.
    fn misbehaving(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_version_that_floods_or_fails_is_not_read() {
        // Real processes through the bounded helper. The budget is long so
        // that a slow start on a busy host cannot turn either case into a
        // timeout; neither case waits for it.
        let dir = tempfile::tempdir().unwrap();
        let budget = Duration::from_secs(30);
        let exactly = misbehaving(
            dir.path(),
            "exactly",
            "head -c 4096 /dev/zero | tr '\\0' x; echo x >&2",
        );
        assert_eq!(version_within(&exactly, budget), Ok("x".repeat(4096)));
        let floods = misbehaving(dir.path(), "floods", "head -c 8192 /dev/zero | tr '\\0' x");
        assert_eq!(
            version_within(&floods, budget),
            Err("--version printed more than 4096 bytes and was stopped".to_string())
        );
        let fails = misbehaving(dir.path(), "fails", "echo commonmeasure 9.9.9; exit 3");
        assert_eq!(
            version_within(&fails, budget),
            Err("--version exited with exit status: 3".to_string())
        );
        let silent = misbehaving(dir.path(), "silent", "exit 0");
        assert_eq!(
            version_within(&silent, budget),
            Err("it printed nothing".to_string())
        );
    }

    #[test]
    fn a_version_that_hangs_times_out_and_its_group_is_stopped() {
        // A child keeps the output open, to show the group is stopped and
        // not only the process that was started. A slow host may time the
        // script out before it writes its pids; the timeout is the result
        // either way, and the pids are checked when they were written.
        let dir = tempfile::tempdir().unwrap();
        let pids = dir.path().join("pids");
        let hangs = misbehaving(
            dir.path(),
            "hangs",
            &format!(
                "echo $$ > '{pids}'; sleep 60 & echo $! >> '{pids}'; wait",
                pids = pids.display()
            ),
        );
        // Asked on a thread, so a wait that ignores its bound fails here at
        // 10 s rather than when `sleep 60` ends.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(version_within(&hangs, Duration::from_secs(1))));
        let read = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("--version was still being waited for after 10 s");
        assert_eq!(
            read,
            Err("--version did not finish within 1 s and was stopped".to_string())
        );
        let written = std::fs::read_to_string(&pids).unwrap_or_default();
        for pid in written.lines() {
            let pid: libc::pid_t = pid.parse().unwrap();
            // A killed process whose parent has not reaped it yet still
            // answers kill 0 for a moment.
            let deadline = Instant::now() + Duration::from_secs(5);
            // SAFETY: kill with signal 0 only checks the pid.
            while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            // SAFETY: as above.
            assert!(
                unsafe { libc::kill(pid, 0) } != 0,
                "pid {pid} was left running"
            );
        }
    }

    #[test]
    fn a_rewrite_in_place_with_the_same_size_is_a_replacement() {
        // Same inode, same size, other bytes: only the digest tells them
        // apart. The "installer" rewrites the target and fails.
        let home = Home::new();
        let launchd = home.launchd();
        let origin = Origin::new("v9.9.9", "9.9.9", true, launchd.log());
        let replacement = Replacement {
            shell: wrapper(
                &home,
                "rewrite",
                "printf '#!/bin/sh\\necho \"commonmeasure 0.0.1\"\\n' > \"$target\"; \
                 chmod 755 \"$target\"; false",
                "",
            ),
            ..home.replacement(&origin, Version(9, 9, 9))
        };
        let before = std::fs::metadata(&home.shell.exe).unwrap();
        let error = replacement.run(&home.services(), &launchd).unwrap_err();
        let after = std::fs::metadata(&home.shell.exe).unwrap();
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(
            (before.ino(), before.size()),
            (after.ino(), after.size()),
            "rewritten in place"
        );
        assert!(!error.contains("unchanged"), "{error}");
        assert!(
            error.contains("had already been replaced by commonmeasure 0.0.1"),
            "{error}"
        );
    }

    #[test]
    fn the_identity_holds_the_digest_and_the_file_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commonmeasure");
        std::fs::write(&path, "same size, first").unwrap();
        let first = identity(&path).unwrap();
        std::fs::write(&path, "same size, other").unwrap();
        let second = identity(&path).unwrap();
        assert_eq!((first.0, first.1, first.2), (second.0, second.1, second.2));
        assert_ne!(first, second);
        assert!(identity(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn a_file_replaced_or_written_while_it_is_hashed_has_no_identity() {
        // The review's race: a writer renames a small file over the path
        // while the identity of a 1 GiB sparse file is being read. The
        // descriptor still names the original, so without the second look
        // the original's identity would be returned for a path that now
        // names another file. Then a writer changes the same file in place,
        // which only the descriptor's second description shows.
        let race = |write: fn(&Path, &Path)| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("commonmeasure");
            std::fs::File::create(&path)
                .unwrap()
                .set_len(1 << 30)
                .unwrap();
            let replacement = dir.path().join("replacement");
            std::fs::write(&replacement, "small").unwrap();
            let writer = {
                let path = path.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    write(&path, &replacement);
                })
            };
            let read = identity(&path);
            writer.join().unwrap();
            read
        };
        let renamed = race(|path, replacement| std::fs::rename(replacement, path).unwrap());
        assert!(renamed.is_err(), "{renamed:?}");
        let written = race(|path, _| {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            file.write_all(b"other bytes").unwrap();
        });
        assert!(written.is_err(), "{written:?}");
    }

    #[test]
    fn a_file_whose_directory_is_renamed_while_it_is_hashed_has_no_identity() {
        // The round-5 review's case: the directory holding the file is
        // renamed away during the hash and a fresh one put in its place, with
        // another file at the same path. The file that was read keeps its
        // device, inode, size, mtime and ctime, so only the path's second
        // look shows the path now names another file.
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live");
        std::fs::create_dir(&live).unwrap();
        let path = live.join("commonmeasure");
        std::fs::write(&path, vec![b'x'; 1 << 17]).unwrap();
        let old = dir.path().join("old");
        WHILE_HASHING.set(Some(Box::new({
            let (live, old) = (live.clone(), old.clone());
            move || {
                std::fs::rename(&live, &old).unwrap();
                std::fs::create_dir(&live).unwrap();
                std::fs::write(live.join("commonmeasure"), "another file").unwrap();
            }
        })));
        let before = std::fs::metadata(&path).unwrap();
        let read = identity(&path);
        let after = std::fs::metadata(old.join("commonmeasure")).unwrap();
        use std::os::unix::fs::MetadataExt as _;
        let stamp = |metadata: &std::fs::Metadata| {
            (
                metadata.dev(),
                metadata.ino(),
                metadata.size(),
                (metadata.mtime(), metadata.mtime_nsec()),
                (metadata.ctime(), metadata.ctime_nsec()),
            )
        };
        assert_eq!(
            stamp(&before),
            stamp(&after),
            "the file read did not change"
        );
        assert_eq!(
            read,
            Err(format!(
                "{}: it was replaced while it was read",
                path.display()
            ))
        );
    }

    #[test]
    fn the_processes_left_on_the_old_release_are_named_by_pid_subcommand_and_home() {
        use crate::processes::Home as Env;
        let target = Path::new("/u/bin/commonmeasure");
        let process = |pid, runs, subcommand: Option<&str>, home| Process {
            pid,
            runs,
            subcommand: subcommand.map(str::to_string),
            home,
        };
        let listed = Ok(vec![
            process(12293, Runs::File, Some("mcp"), Env::Default),
            process(61846, Runs::File, Some("serve"), Env::Set("/edge/a".into())),
            process(400, Runs::File, Some("mcp"), Env::Unknown),
            process(
                401,
                Runs::Unidentified {
                    name: "commonmeasure".into(),
                },
                None,
                Env::Default,
            ),
        ]);
        assert_eq!(
            still_running(target, Version(0, 4, 4), &listed),
            "\nThese processes run commonmeasure 0.4.4, or a release installed before it, until \
             their host restarts them:\n  pid 12293  mcp  COMMONMEASURE_HOME not set or empty\n  \
             pid 61846  serve  COMMONMEASURE_HOME=/edge/a\n  pid 400  mcp  COMMONMEASURE_HOME \
             unknown\n  pid 401  process name commonmeasure, its executable could not be read  \
             COMMONMEASURE_HOME not set or empty\n"
        );
        assert_eq!(still_running(target, Version(0, 4, 4), &Ok(Vec::new())), "");
        let failed = still_running(target, Version(0, 4, 4), &Err("read /proc: denied".into()));
        assert!(
            failed.contains("could not be listed (read /proc: denied)"),
            "{failed}"
        );
    }
}
