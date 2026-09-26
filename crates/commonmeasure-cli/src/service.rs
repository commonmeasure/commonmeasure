//! `commonmeasure service`: the operator console as a login service.
//!
//! On macOS the console runs as a LaunchAgent, `ai.commonmeasure.console`,
//! loaded into the user's GUI domain with `launchctl bootstrap` and removed
//! with `launchctl bootout`. Other platforms get an explicit refusal naming
//! the command to run the console under their own service manager: only the
//! LaunchAgent has been exercised against a real service manager.
//!
//! A console started by hand and forgotten is the failure this exists for,
//! so `status` reports what holds the port whether or not it is the service,
//! and reports a service still running a binary that has since been replaced
//! on disk. Launchd's `KeepAlive` restarts a process that exits; it does not
//! restart one whose executable was upgraded underneath it.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The launchd label, the plist's file stem and the service's name in
/// `launchctl print`.
pub const LABEL: &str = "ai.commonmeasure.console";

/// Where `serve` listens unless told otherwise.
const DEFAULT_LISTEN: &str = "127.0.0.1:4173";

/// How long install waits for the previous process to release the port and
/// for the new one to answer.
const SETTLE: Duration = Duration::from_secs(10);

#[derive(clap::Subcommand)]
pub enum ServiceCommand {
    /// Install the console as a login service and start it: on macOS a
    /// LaunchAgent at ~/Library/LaunchAgents/ai.commonmeasure.console.plist
    /// that runs this binary's `serve` at login, restarts it if it exits, and
    /// logs to logs/console.log in the Edge home (~/.commonmeasure, or
    /// COMMONMEASURE_HOME, which the plist then sets for the service).
    /// Installing again rewrites the plist from this shell and restarts the
    /// console. Refuses a non-loopback --listen, and refuses when another
    /// process already listens on the address.
    Install {
        service: ServiceName,
        /// Address the console listens on. Loopback only.
        #[arg(long, default_value = DEFAULT_LISTEN)]
        listen: String,
    },
    /// Whether the console service is installed, loaded and listening; the
    /// binary, version, Edge home and log it runs with; the port; and
    /// whether the binary on PATH differs from the one running. Also names a
    /// console on the port that the service did not start.
    Status,
    /// Stop the console service and remove its plist. The log is kept.
    Uninstall { service: ServiceName },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ServiceName {
    /// The operator console, `commonmeasure serve`.
    Console,
}

/// What a command printed and how it exited.
pub struct Output {
    pub success: bool,
    /// The exit code; `None` when a signal ended the command.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `launchctl`, `lsof` and `ps`: the seam between this module and the
/// machine's service manager, so tests never reach the real one. The binary
/// on PATH is asked for its version directly, within bounds (`version_of`).
pub trait Commands {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, String>;
}

/// The machine's own commands.
pub struct System;

impl Commands for System {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("run {program}: {error}"))?;
        Ok(Output {
            success: output.status.success(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The facts about this user and binary the service is built from.
pub struct Context {
    /// The user's home directory: the plist goes under its `Library`.
    pub user_home: PathBuf,
    /// Set when `COMMONMEASURE_HOME` chose the Edge home, so the service is
    /// started with the same home the installing shell had.
    pub home_override: Option<PathBuf>,
    /// Where the service's output goes: `logs/console.log` in the Edge home.
    pub log: PathBuf,
    /// The service's working directory: the user's home directory.
    pub working_directory: PathBuf,
    /// The binary the plist runs: the one doing the installing.
    pub exe: PathBuf,
    /// The first `commonmeasure` on PATH, the binary an upgrade replaces.
    pub on_path: Option<PathBuf>,
    pub uid: u32,
    /// The launchd label: [`LABEL`], or in a debug build the value of
    /// `COMMONMEASURE_SERVICE_LABEL`, so that tests running the binary
    /// address a scratch label and never the owner's service.
    pub label: String,
    pub settle: Duration,
    /// How long another `commonmeasure` is given to answer `--version`:
    /// [`crate::update::VERSION_WAIT`] outside tests.
    pub version_wait: Duration,
}

impl Context {
    fn plist(&self) -> PathBuf {
        plist_path(&self.user_home, &self.label)
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.label)
    }

    /// This context with the installed service's configuration in place of
    /// the invoking shell's, so the service is started as it was installed.
    fn installed(&self, installed: &Installed) -> Context {
        Context {
            user_home: self.user_home.clone(),
            home_override: installed.home.clone(),
            log: installed.log.clone(),
            working_directory: installed.working_directory.clone(),
            exe: installed.program.clone(),
            on_path: self.on_path.clone(),
            uid: self.uid,
            label: self.label.clone(),
            settle: self.settle,
            version_wait: self.version_wait,
        }
    }
}

pub fn plist_path(user_home: &Path, label: &str) -> PathBuf {
    user_home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist"))
}

pub fn run(command: ServiceCommand) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(unsupported(&command));
    }
    let context = context()?;
    let text = match command {
        ServiceCommand::Install { listen, .. } => install(&context, &System, &listen)?,
        ServiceCommand::Status => status(&context, &System)?,
        ServiceCommand::Uninstall { .. } => uninstall(&context, &System)?,
    };
    print!("{text}");
    Ok(())
}

/// The refusal on a platform without a LaunchAgent, naming what to run
/// instead.
fn unsupported(command: &ServiceCommand) -> String {
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "commonmeasure".to_string());
    let listen = match command {
        ServiceCommand::Install { listen, .. } => listen.as_str(),
        _ => DEFAULT_LISTEN,
    };
    format!(
        "commonmeasure service is supported on macOS only. Under systemd, run the console as a \
         user service yourself: systemd-run --user --unit=commonmeasure-console {exe} serve \
         --listen {listen}"
    )
}

/// The console service as `update` finds it before anything is stopped.
#[derive(Debug, PartialEq)]
pub enum BeforeUpdate {
    /// Launchd has no such job loaded. A plist left on disk is neither
    /// stopped nor started.
    Absent,
    /// The loaded job runs the binary being replaced, and its plist holds
    /// the configuration launchd loaded, so the job can be started again as
    /// it was.
    Runs {
        installed: Installed,
        pid: Option<u32>,
        /// When launchd was asked for `pid`, noted just before asking.
        asked: SystemTime,
    },
    /// The loaded job runs another binary, which the update leaves alone.
    RunsOther(PathBuf),
}

/// Before `update` replaces `binary`: what the console service runs, asked
/// of launchd first and of the plist second, since a plist can be removed or
/// rewritten while the job it loaded keeps running. Changes nothing.
///
/// A loaded job running `binary` is an error unless its plist can be read
/// and matches the loaded job: the update would otherwise either replace the
/// binary under a console it cannot stop and start again, or start it again
/// with another Edge home.
pub fn inspect_for_update(
    context: &Context,
    commands: &dyn Commands,
    binary: &Path,
) -> Result<BeforeUpdate, String> {
    let label = &context.label;
    let target = context.target();
    let asked = SystemTime::now();
    let job = match loaded(context, commands) {
        Ok(None) => return Ok(BeforeUpdate::Absent),
        Ok(Some(job)) => job,
        Err(error) => {
            return Err(format!(
                "cannot tell whether the console service {label} is loaded: {error}. Check it \
                 with: launchctl print {target}"
            ));
        }
    };
    let Some(program) = job.program() else {
        return Err(format!(
            "the console service {label} is loaded, but launchctl print {target} names no \
             program, so update cannot tell whether it runs {}. Remove it with: commonmeasure \
             service uninstall console",
            binary.display()
        ));
    };
    if !same_path(&program, binary) {
        return Ok(BeforeUpdate::RunsOther(program));
    }
    let path = context.plist();
    let running = format!(
        "the console service {label} is loaded{} and runs {}",
        job.pid
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default(),
        program.display()
    );
    let listen = job.listen().unwrap_or("<address>");
    let home = match &job.home {
        JobHome::Set(home) => Some(home.as_path()),
        JobHome::Unset => None,
        JobHome::Unknown => {
            return Err(format!(
                "{running}, but launchctl print {target} shows no environment block update can \
                 read, so update cannot tell which Edge home the service runs with. Stop it with \
                 `commonmeasure service uninstall console`, then install it again from a shell \
                 that selects its Edge home: `{} service install console --listen {listen}`. \
                 Then run update again",
                shell_quote(&program)
            ));
        }
        JobHome::Empty => {
            return Err(format!(
                "{running}, but launchctl print {target} shows the service's own \
                 COMMONMEASURE_HOME set to \"\" and none inherited, so update cannot establish \
                 from that value which Edge home the service runs with. Stop it with \
                 `commonmeasure service uninstall console`, then install it again from a shell \
                 that selects its Edge home: `{} service install console --listen {listen}`. \
                 Then run update again",
                shell_quote(&program)
            ));
        }
        JobHome::Inherited { own, inherited } => {
            return Err(inherited_refusal(
                &running,
                own.as_deref(),
                inherited,
                &program,
                listen,
            ));
        }
    };
    let remedy = format!(
        "Stop it with `commonmeasure service uninstall console`, or install it again with the \
         Edge home launchd reports for it: `{}`. Then run update again",
        install_command(home, &program, listen)
    );
    let text = match std::fs::read_to_string(&path) {
        Err(error) => {
            return Err(format!(
                "{running}, but its plist {} cannot be read ({error}), so update could not \
                 start it again with the same Edge home. {remedy}",
                path.display()
            ));
        }
        Ok(text) => text,
    };
    let installed = read_plist(&text).ok_or_else(|| {
        format!(
            "{running}, but its plist {} is not in the form install writes, so update could not \
             start it again with the same Edge home. {remedy}",
            path.display()
        )
    })?;
    // Starting the service again writes the plist from `installed`, so any
    // setting outside it, such as a hand-edited KeepAlive, would be lost.
    if edited(context, &installed, &text) {
        return Err(format!(
            "{running}, but its plist {} has been edited since service install wrote it, and \
             update would start the service again from a plist written anew, losing the edits. \
             {remedy}",
            path.display()
        ));
    }
    let differences = job.differences(&installed, &path);
    if !differences.is_empty() {
        return Err(format!(
            "{running}, but its plist {} differs from the job launchd loaded ({}), and update \
             would start it again from the plist. {remedy}",
            path.display(),
            differences.join("; ")
        ));
    }
    Ok(BeforeUpdate::Runs {
        installed,
        pid: job.pid,
        asked,
    })
}

/// The pid launchd reports for the console service now, asked again after
/// `update` has scanned the process table. `None` when it reports none, or
/// cannot be asked, so that the process at the earlier pid is counted.
pub fn console_pid(context: &Context, commands: &dyn Commands) -> Option<u32> {
    loaded(context, commands).ok().flatten()?.pid
}

/// Why the service `inspect_for_update` found could not be stopped.
#[derive(Debug, PartialEq)]
pub enum StopFailed {
    /// Launchd refused to boot the job out; it may still be running.
    NotStopped(String),
    /// The job was booted out but the port did not free: something is down
    /// that should be started again.
    PortHeld(String),
}

/// Stop the service `inspect_for_update` found running the binary: boot it
/// out, so that `KeepAlive` cannot start the old binary again, and wait for
/// its port to free.
pub fn stop_for_update(
    context: &Context,
    commands: &dyn Commands,
    installed: &Installed,
) -> Result<(), StopFailed> {
    let address = loopback(&installed.listen).map_err(StopFailed::NotStopped)?;
    bootout(context, commands).map_err(StopFailed::NotStopped)?;
    if !wait_until(context.settle, || probe(address) == Answer::Nothing) {
        return Err(StopFailed::PortHeld(format!(
            "the console service was stopped but {} is still in use {} s later",
            installed.listen,
            context.settle.as_secs()
        )));
    }
    Ok(())
}

/// Start the service `stop_for_update` stopped, as it was installed: its
/// binary path, address, Edge home, log and working directory, whatever the
/// shell running `update` selects.
pub fn start_after_update(
    context: &Context,
    commands: &dyn Commands,
    installed: &Installed,
) -> Result<String, String> {
    install(&context.installed(installed), commands, &installed.listen)
}

/// Why `update` refuses a job whose launchd domain passes it
/// `COMMONMEASURE_HOME`, and how to install it again with the home it runs
/// with. The domain variable is removed first, or the next update would
/// refuse again. When the home it runs with is not known, no home is filled
/// in: the operator chooses one.
fn inherited_refusal(
    running: &str,
    own: Option<&Path>,
    inherited: &Path,
    program: &Path,
    listen: &str,
) -> String {
    let value = |home: &Path| match home.as_os_str().is_empty() {
        true => "\"\"".to_string(),
        false => home.display().to_string(),
    };
    let passed = format!(
        "{running}, and launchd passes it COMMONMEASURE_HOME={} through the domain's \
         environment (launchctl setenv), which service install never uses",
        value(inherited)
    );
    let unset = "Remove it with `launchctl unsetenv COMMONMEASURE_HOME`";
    match (own, effective_home(own, inherited)) {
        (Some(own), Some(_)) => format!(
            "{passed}. The job's own environment sets COMMONMEASURE_HOME={}, which launchd \
             applies over the inherited value, so the service runs with that home. {unset}, \
             then install the service again with it: `{}`. Then run update again",
            own.display(),
            install_command(Some(own), program, listen)
        ),
        (None, Some(_)) => format!(
            "{passed}. {unset}, then install the service again: `{}`. Then run update again",
            install_command(Some(inherited), program, listen)
        ),
        (own, None) => format!(
            "{passed}{}, so update cannot tell which Edge home the service runs with. {unset}, \
             choose the Edge home the service should use, and install the service again from a \
             shell that selects it: `{} service install console --listen {listen}`. Then run \
             update again",
            own.map(|own| format!(
                ", and its own environment sets COMMONMEASURE_HOME={}",
                value(own)
            ))
            .unwrap_or_default(),
            shell_quote(program)
        ),
    }
}

/// The Edge home a job runs with when its launchd domain passes it
/// `inherited`: launchd applies the job's own environment after the
/// inherited one, so `own` wins where it is set
/// ([`launchd` job environment setup](https://github.com/apple-oss-distributions/launchd/blob/main/src/core.c)).
/// `None` when the winning value is empty, which names no home.
fn effective_home<'a>(own: Option<&'a Path>, inherited: &'a Path) -> Option<&'a Path> {
    Some(own.unwrap_or(inherited)).filter(|home| !home.as_os_str().is_empty())
}

/// The command that installs the service again as `installed` describes
/// it, for an operator to run when `update` could not.
pub fn reinstall_command(installed: &Installed) -> String {
    install_command(
        installed.home.as_deref(),
        &installed.program,
        &installed.listen,
    )
}

/// `service install` run by `program` with `COMMONMEASURE_HOME` set to
/// `home`, or unset when the service uses the default Edge home.
fn install_command(home: Option<&Path>, program: &Path, listen: &str) -> String {
    let home = match home {
        Some(home) => format!("env COMMONMEASURE_HOME={}", shell_quote(home)),
        None => "env -u COMMONMEASURE_HOME".to_string(),
    };
    format!(
        "{home} {} service install console --listen {listen}",
        shell_quote(program)
    )
}

fn shell_quote(path: &Path) -> String {
    let text = path.display().to_string();
    if text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-+:@".contains(&byte))
    {
        text
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Whether two paths name the same file, or are the same text where either
/// cannot be resolved.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

pub fn context() -> Result<Context, String> {
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or("HOME is not set to an absolute path")?;
    let home = commonmeasure_harness::home_dir().map_err(|error| error.to_string())?;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let home = cwd.join(home);
    let home_override = std::env::var_os("COMMONMEASURE_HOME")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(|_| home.clone());
    // Not canonicalised: a symlinked install keeps pointing at the link, so
    // an upgrade that retargets the link is the binary the service runs.
    let exe = cwd.join(std::env::current_exe().map_err(|error| error.to_string())?);
    Ok(Context {
        log: home.join("logs").join("console.log"),
        working_directory: user_home.clone(),
        user_home,
        home_override,
        exe,
        on_path: on_path("commonmeasure"),
        uid: uid()?,
        label: label(
            cfg!(debug_assertions),
            std::env::var("COMMONMEASURE_SERVICE_LABEL").ok(),
        )?,
        settle: SETTLE,
        version_wait: crate::update::VERSION_WAIT,
    })
}

/// The launchd label. A debug build takes `COMMONMEASURE_SERVICE_LABEL`, so
/// the tests that run the binary with a scratch `HOME` also use a scratch
/// label; a release build always uses [`LABEL`], as it does the public
/// release origin.
fn label(honour_override: bool, value: Option<String>) -> Result<String, String> {
    match value.filter(|label| honour_override && !label.is_empty()) {
        None => Ok(LABEL.to_string()),
        Some(label)
            if label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte)) =>
        {
            Ok(label)
        }
        Some(label) => Err(format!(
            "COMMONMEASURE_SERVICE_LABEL={label:?} is not a launchd label of letters, digits, \
             dots, hyphens and underscores"
        )),
    }
}

#[cfg(unix)]
fn uid() -> Result<u32, String> {
    // SAFETY: getuid takes no arguments, cannot fail and touches no memory
    // of ours.
    Ok(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn uid() -> Result<u32, String> {
    Err("no user id on this platform".to_string())
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The address to probe for `listen`, refusing anything that is not
/// loopback, as `serve` does, and port 0, whose port status could not name.
pub fn loopback(listen: &str) -> Result<SocketAddr, String> {
    let exposed = commonmeasure_console::exposed_addresses(listen)
        .map_err(|error| format!("--listen {listen}: {error:#}"))?;
    if !exposed.is_empty() {
        return Err(format!(
            "--listen {listen} is reachable from outside this machine ({}). The console carries \
             no credential, so the service binds loopback only.",
            exposed
                .iter()
                .map(SocketAddr::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let address = listen
        .to_socket_addrs()
        .map_err(|error| format!("--listen {listen}: {error}"))?
        .next()
        .ok_or_else(|| format!("--listen {listen} resolved to no address"))?;
    if address.port() == 0 {
        return Err(format!("--listen {listen} needs a fixed port"));
    }
    Ok(address)
}

/// The LaunchAgent for `exe serve --listen <listen>`.
pub fn plist(context: &Context, listen: &str) -> String {
    let label = escape(&context.label);
    let mut arguments = String::new();
    for argument in [
        &context.exe.display().to_string(),
        "serve",
        "--listen",
        listen,
    ] {
        arguments.push_str(&format!("\t\t<string>{}</string>\n", escape(argument)));
    }
    let environment = match &context.home_override {
        Some(home) => format!(
            "\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>COMMONMEASURE_HOME</key>\n\
             \t\t<string>{}</string>\n\t</dict>\n",
            escape(&home.display().to_string())
        ),
        None => String::new(),
    };
    let log = escape(&context.log.display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\
         {environment}\
         \t<key>WorkingDirectory</key>\n\t<string>{}</string>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>KeepAlive</key>\n\t<true/>\n\
         \t<key>StandardOutPath</key>\n\t<string>{log}</string>\n\
         \t<key>StandardErrorPath</key>\n\t<string>{log}</string>\n\
         </dict>\n</plist>\n",
        escape(&context.working_directory.display().to_string()),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// What an installed plist runs and where the service keeps its state: the
/// configuration `update` starts the service again with.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub program: PathBuf,
    pub listen: String,
    /// `COMMONMEASURE_HOME` in the plist's environment; `None` when the
    /// service uses the default Edge home.
    pub home: Option<PathBuf>,
    /// The log, both standard output and standard error.
    pub log: PathBuf,
    pub working_directory: PathBuf,
}

/// Reads back a plist written by [`plist`]. Not a general plist parser: a
/// plist edited into another shape, including one with environment variables
/// other than `COMMONMEASURE_HOME` or with separate output and error logs,
/// reads as `None`, and status says so.
pub fn read_plist(text: &str) -> Option<Installed> {
    let (_, rest) = text.split_once("<key>ProgramArguments</key>")?;
    let (array, _) = rest.split_once("</array>")?;
    let arguments: Vec<String> = array
        .split("<string>")
        .skip(1)
        .filter_map(|piece| piece.split_once("</string>"))
        .map(|(value, _)| unescape(value))
        .collect();
    let [program, serve, flag, listen] = arguments.as_slice() else {
        return None;
    };
    if serve != "serve" || flag != "--listen" {
        return None;
    }
    let log = plist_string(text, "StandardOutPath")?;
    if plist_string(text, "StandardErrorPath")? != log {
        return None;
    }
    let home = match text.split_once("<key>EnvironmentVariables</key>") {
        None => None,
        Some((_, rest)) => {
            let (dict, _) = rest.split_once("</dict>")?;
            let keys: Vec<&str> = dict
                .split("<key>")
                .skip(1)
                .filter_map(|piece| piece.split_once("</key>"))
                .map(|(key, _)| key)
                .collect();
            if keys != ["COMMONMEASURE_HOME"] {
                return None;
            }
            Some(PathBuf::from(plist_string(dict, "COMMONMEASURE_HOME")?))
        }
    };
    Some(Installed {
        program: PathBuf::from(program),
        listen: listen.clone(),
        home,
        log: PathBuf::from(log),
        working_directory: PathBuf::from(plist_string(text, "WorkingDirectory")?),
    })
}

/// Whether `text`, read back as `installed`, is other than the bytes
/// [`plist`] writes for it: a plist edited by hand, whose other settings a
/// restart would drop.
fn edited(context: &Context, installed: &Installed, text: &str) -> bool {
    plist(&context.installed(installed), &installed.listen) != text
}

/// The `<string>` value that follows `<key>key</key>`.
fn plist_string(text: &str, key: &str) -> Option<String> {
    let (_, rest) = text.split_once(&format!("<key>{key}</key>"))?;
    let rest = rest.trim_start().strip_prefix("<string>")?;
    let (value, _) = rest.split_once("</string>")?;
    Some(unescape(value))
}

/// What `launchctl print` says of the loaded job: launchd's copy of the
/// configuration, which is what runs whatever the plist on disk now says.
#[derive(Debug, Default, PartialEq)]
struct Loaded {
    pid: Option<u32>,
    state: Option<String>,
    /// The plist launchd loaded the job from.
    path: Option<PathBuf>,
    program: Option<PathBuf>,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
    stdout: Option<PathBuf>,
    stderr: Option<PathBuf>,
    home: JobHome,
}

/// `COMMONMEASURE_HOME` as `launchctl print` reports the job's environment.
#[derive(Debug, Default, PartialEq)]
enum JobHome {
    /// The job's own environment, from its plist, sets it.
    Set(PathBuf),
    /// Its environment and the environment it inherits were read, and
    /// neither sets it: the service uses the default Edge home.
    Unset,
    /// No environment in the form launchd prints, so the Edge home is not
    /// known.
    #[default]
    Unknown,
    /// The job's own environment sets it empty and it inherits none, which
    /// does not establish the home the service runs with; `service install`
    /// never writes an empty value.
    Empty,
    /// Set in the environment the job inherits from its launchd domain
    /// (`launchctl setenv`), which `service install` never does. `own` is
    /// the job's own value when its environment sets one too; both are kept
    /// because they decide different recoveries.
    Inherited {
        own: Option<PathBuf>,
        inherited: PathBuf,
    },
}

impl JobHome {
    /// From the `environment` and `inherited environment` blocks of
    /// `launchctl print`, each of `KEY => value` lines, or `KEY =>` for an
    /// empty value. Launchd prints both
    /// for every job in the GUI domain (60 of 60 agents on the machine this
    /// was written on), so a missing or unreadable block means the format
    /// changed, not that the environment is empty.
    fn read(environment: Option<Vec<String>>, inherited: Option<Vec<String>>) -> JobHome {
        // Launchd prints an empty value as `KEY => `, whose trailing space
        // the block's lines have lost.
        let variables = |block: Option<Vec<String>>| -> Option<Vec<(String, String)>> {
            block?
                .iter()
                .map(|line| {
                    line.split_once(" => ")
                        .or_else(|| Some((line.strip_suffix(" =>")?, "")))
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                })
                .collect()
        };
        let home = |variables: &[(String, String)]| {
            variables
                .iter()
                .find(|(key, _)| key == "COMMONMEASURE_HOME")
                .map(|(_, value)| PathBuf::from(value))
        };
        let (Some(environment), Some(inherited)) = (variables(environment), variables(inherited))
        else {
            return JobHome::Unknown;
        };
        match (home(&environment), home(&inherited)) {
            (own, Some(inherited)) => JobHome::Inherited { own, inherited },
            (Some(home), None) if !home.as_os_str().is_empty() => JobHome::Set(home),
            (Some(_), None) => JobHome::Empty,
            (None, None) => JobHome::Unset,
        }
    }
}

impl Loaded {
    /// Reads the job's own fields, which sit one tab in; nested blocks
    /// repeat some names further in. The output is launchd's diagnostic
    /// text, not an interface, so a field it stops printing reads as absent
    /// and `update` refuses rather than guesses.
    fn parse(text: &str) -> Loaded {
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(&format!("\t{name} = ")))
                .map(|value| value.trim().to_string())
        };
        // The lines of a block, or `None` when it is absent or unterminated.
        let block = |name: &str| -> Option<Vec<String>> {
            let mut lines = text
                .lines()
                .skip_while(|line| *line != format!("\t{name} = {{"));
            lines.next()?;
            let mut out = Vec::new();
            for line in lines {
                if line == "\t}" {
                    return Some(out);
                }
                out.push(line.trim().to_string());
            }
            None
        };
        Loaded {
            pid: field("pid").and_then(|pid| pid.parse().ok()),
            state: field("state"),
            path: field("path").map(PathBuf::from),
            program: field("program").map(PathBuf::from),
            arguments: block("arguments").unwrap_or_default(),
            working_directory: field("working directory").map(PathBuf::from),
            stdout: field("stdout path").map(PathBuf::from),
            stderr: field("stderr path").map(PathBuf::from),
            home: JobHome::read(block("environment"), block("inherited environment")),
        }
    }

    fn program(&self) -> Option<PathBuf> {
        self.program
            .clone()
            .or_else(|| self.arguments.first().map(PathBuf::from))
    }

    /// The address from `serve --listen <address>`.
    fn listen(&self) -> Option<&str> {
        match self.arguments.as_slice() {
            [_, serve, flag, listen] if serve == "serve" && flag == "--listen" => Some(listen),
            _ => None,
        }
    }

    /// Where the plist at `path` says something other than the loaded job.
    fn differences(&self, installed: &Installed, path: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut path_field = |name: &str, loaded: Option<&Path>, plist: Option<&Path>| {
            let same = match (loaded, plist) {
                (Some(loaded), Some(plist)) => same_path(loaded, plist),
                (None, None) => true,
                _ => false,
            };
            if !same {
                let show = |value: Option<&Path>| {
                    value.map_or("none".to_string(), |value| value.display().to_string())
                };
                out.push(format!(
                    "{name}: loaded {}, plist {}",
                    show(loaded),
                    show(plist)
                ));
            }
        };
        path_field("plist", self.path.as_deref(), Some(path));
        path_field(
            "program",
            self.program().as_deref(),
            Some(&installed.program),
        );
        match &self.home {
            JobHome::Set(home) => {
                path_field("COMMONMEASURE_HOME", Some(home), installed.home.as_deref())
            }
            JobHome::Unset => path_field("COMMONMEASURE_HOME", None, installed.home.as_deref()),
            // `inspect_for_update` refuses these before comparing.
            JobHome::Unknown | JobHome::Empty | JobHome::Inherited { .. } => path_field(
                "COMMONMEASURE_HOME",
                Some(Path::new("not known from launchd's environment")),
                None,
            ),
        }
        path_field("stdout", self.stdout.as_deref(), Some(&installed.log));
        path_field("stderr", self.stderr.as_deref(), Some(&installed.log));
        path_field(
            "working directory",
            self.working_directory.as_deref(),
            Some(&installed.working_directory),
        );
        if self.listen() != Some(installed.listen.as_str()) {
            out.push(format!(
                "arguments: loaded {}, plist serve --listen {}",
                self.arguments.join(" "),
                installed.listen
            ));
        }
        out
    }
}

/// Whether launchd has the job loaded. `Ok(None)` only when launchd says it
/// has no such job; any other failure to ask is an error, because a job
/// that could not be inspected may still be running.
fn loaded(context: &Context, commands: &dyn Commands) -> Result<Option<Loaded>, String> {
    let target = context.target();
    let output = commands.run("launchctl", &["print", &target])?;
    if output.success {
        return Ok(Some(Loaded::parse(&output.stdout)));
    }
    // 113 with "Could not find service" is launchd's answer for a label it
    // has not loaded in the domain.
    if output.code == Some(113) && output.stderr.contains("Could not find service") {
        return Ok(None);
    }
    Err(format!(
        "launchctl print {target} failed ({}): {}",
        output
            .code
            .map_or("no exit code".to_string(), |code| format!("exit {code}")),
        output.stderr.trim()
    ))
}

/// A process found holding the port.
struct Holder {
    pid: u32,
    command: Option<String>,
}

fn holder(commands: &dyn Commands, port: u16) -> Option<Holder> {
    let port = format!("-iTCP:{port}");
    let output = commands
        .run("lsof", &["-tnP", &port, "-sTCP:LISTEN"])
        .ok()?;
    let pid: u32 = output.stdout.lines().next()?.trim().parse().ok()?;
    let command = commands
        .run("ps", &["-o", "args=", "-p", &pid.to_string()])
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout.trim().to_string())
        .filter(|command| !command.is_empty());
    Some(Holder { pid, command })
}

/// When a process started, from `ps -o etime=`, which is locale-free.
fn started(commands: &dyn Commands, pid: u32) -> Option<SystemTime> {
    let output = commands
        .run("ps", &["-o", "etime=", "-p", &pid.to_string()])
        .ok()
        .filter(|output| output.success)?;
    let elapsed = elapsed(output.stdout.trim())?;
    SystemTime::now().checked_sub(elapsed)
}

/// `[[dd-]hh:]mm:ss`.
fn elapsed(text: &str) -> Option<Duration> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let parts: Vec<u64> = clock
        .split(':')
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(Duration::from_secs(days * 86_400 + seconds))
}

/// When a file's contents or identity last changed: an upgrade that renames
/// a new file into place moves the change time even where it keeps the
/// modification time of the download.
fn changed(path: &Path) -> Option<SystemTime> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let ctime = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(u64::try_from(metadata.ctime()).ok()?))?;
        Some(modified.max(ctime))
    }
    #[cfg(not(unix))]
    Some(modified)
}

/// What answered on the address.
#[derive(Debug, PartialEq)]
enum Answer {
    Nothing,
    /// A Common Measure console reporting its version and process.
    Console {
        version: String,
        pid: Option<u32>,
    },
    /// Something accepted the connection but did not report a version: a
    /// console from before `/api/version`, or another program.
    Unreported,
}

fn probe(address: SocketAddr) -> Answer {
    let timeout = Duration::from_secs(2);
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return Answer::Nothing;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!(
        "GET /api/version HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    let mut response = Vec::new();
    if stream.write_all(request.as_bytes()).is_err()
        || stream.take(64 * 1024).read_to_end(&mut response).is_err() && response.is_empty()
    {
        return Answer::Unreported;
    }
    let response = String::from_utf8_lossy(&response);
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return Answer::Unreported;
    };
    if !head.starts_with("HTTP/1.1 200") {
        return Answer::Unreported;
    }
    // The console answers with a Content-Length body; decoding chunks is not
    // needed for it.
    match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(value) if value["product"] == "commonmeasure" => match value["version"].as_str() {
            Some(version) => Answer::Console {
                version: version.to_string(),
                pid: value["pid"]
                    .as_u64()
                    .and_then(|pid| u32::try_from(pid).ok()),
            },
            None => Answer::Unreported,
        },
        _ => Answer::Unreported,
    }
}

fn wait_until(settle: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + settle;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The version `binary --version` reports, answered without running the
/// binary when it is this one, or why it is not known. Another binary is
/// asked within the same bounds as a replacement `update` did not confirm
/// (`update::version_within`), so one that hangs or floods cannot hold
/// `status`.
fn version_of(context: &Context, binary: &Path) -> Result<String, String> {
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same(binary, &context.exe) {
        return Ok(env!("CARGO_PKG_VERSION").to_string());
    }
    let reported = crate::update::version_within(binary, context.version_wait)?;
    reported
        .strip_prefix("commonmeasure ")
        .map(str::to_string)
        .ok_or_else(|| "--version did not print a commonmeasure version".to_string())
}

pub fn install(context: &Context, commands: &dyn Commands, listen: &str) -> Result<String, String> {
    let label = &context.label;
    let address = loopback(listen)?;
    let loaded = loaded(context, commands)?;
    let service_pid = loaded.as_ref().and_then(|loaded| loaded.pid);
    if probe(address) != Answer::Nothing {
        match holder(commands, address.port()) {
            Some(holder) if Some(holder.pid) != service_pid => {
                return Err(format!(
                    "{listen} is already in use by pid {} ({}), which is not the service. Stop \
                     it first (kill {}), then install again.",
                    holder.pid,
                    holder.command.as_deref().unwrap_or("command not readable"),
                    holder.pid,
                ));
            }
            Some(_) => {}
            None if service_pid.is_none() => {
                return Err(format!(
                    "{listen} is already in use, and the process holding it could not be named \
                     (lsof -nP -iTCP:{} -sTCP:LISTEN). Stop it first, then install again.",
                    address.port()
                ));
            }
            None => {}
        }
    }

    let log = &context.log;
    std::fs::create_dir_all(log.parent().ok_or("the log path has no directory")?)
        .map_err(|error| format!("create {}: {error}", log.display()))?;
    let path = context.plist();
    std::fs::create_dir_all(path.parent().expect("the plist has a parent"))
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    // Under the umask, as the plists beside it: the plist holds no secret,
    // and launchd reads it as its owner wrote it. The temporary is this
    // writer's own, so a leftover `<label>.plist.tmp` is neither written
    // into nor renamed into place.
    commonmeasure_runtime::declaration::replace(&path, plist(context, listen).as_bytes())?;

    if loaded.is_some() {
        bootout(context, commands)?;
        if !wait_until(context.settle, || probe(address) == Answer::Nothing) {
            return Err(format!(
                "the previous console still holds {listen} {} s after it was stopped; the new \
                 plist is written but not loaded. Run install again once the port is free.",
                context.settle.as_secs()
            ));
        }
    }
    let output = commands.run(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{}", context.uid),
            &path.display().to_string(),
        ],
    )?;
    if !output.success {
        return Err(format!(
            "launchctl bootstrap gui/{} {} failed: {}",
            context.uid,
            path.display(),
            output.stderr.trim()
        ));
    }
    // Bootstrap alone can leave a RunAtLoad agent pending a "speculative"
    // spawn that launchd defers indefinitely (observed on macOS 26: `runs =
    // 0`, nothing listening). Kickstart starts it now.
    let output = commands.run("launchctl", &["kickstart", &context.target()])?;
    if !output.success {
        return Err(format!(
            "{label} is loaded but launchctl kickstart {} failed: {}",
            context.target(),
            output.stderr.trim()
        ));
    }

    let mut answer = Answer::Nothing;
    wait_until(context.settle, || {
        answer = probe(address);
        answer != Answer::Nothing
    });
    let verb = if loaded.is_some() {
        "reinstalled and restarted"
    } else {
        "installed"
    };
    match answer {
        Answer::Console { version, pid } => Ok(format!(
            "{label} {verb}\nplist      {}\nlistening  http://{address}, Common Measure \
             {version}{}\nlog        {}\n",
            path.display(),
            pid.map(|pid| format!(", pid {pid}")).unwrap_or_default(),
            log.display(),
        )),
        Answer::Unreported => Err(format!(
            "{label} {verb}, but what answers on {listen} does not report a Common Measure \
             version. Check with: commonmeasure service status. Log: {}",
            log.display()
        )),
        Answer::Nothing => Err(format!(
            "{label} {verb}, but nothing listens on {listen} after {} s. Launchd keeps \
             restarting it; the reason is in {}",
            context.settle.as_secs(),
            log.display()
        )),
    }
}

fn bootout(context: &Context, commands: &dyn Commands) -> Result<(), String> {
    let output = commands.run("launchctl", &["bootout", &context.target()])?;
    if output.success {
        Ok(())
    } else {
        Err(format!(
            "launchctl bootout {} failed: {}",
            context.target(),
            output.stderr.trim()
        ))
    }
}

pub fn uninstall(context: &Context, commands: &dyn Commands) -> Result<String, String> {
    let label = &context.label;
    let path = context.plist();
    let log = std::fs::read_to_string(&path)
        .ok()
        .as_deref()
        .and_then(read_plist)
        .map_or_else(|| context.log.clone(), |installed| installed.log);
    let was_loaded = loaded(context, commands)?.is_some();
    if was_loaded {
        bootout(context, commands)?;
    }
    let had_plist = path.exists();
    if had_plist {
        std::fs::remove_file(&path)
            .map_err(|error| format!("remove {}: {error}", path.display()))?;
    }
    Ok(match (was_loaded, had_plist) {
        (false, false) => format!("{label} is not installed; nothing to remove\n"),
        _ => format!(
            "{label} {}removed {}\nlog kept   {}\n",
            if was_loaded { "stopped and " } else { "" },
            path.display(),
            log.display()
        ),
    })
}

pub fn status(context: &Context, commands: &dyn Commands) -> Result<String, String> {
    let label = &context.label;
    let path = context.plist();
    let exists = path.exists();
    let text = std::fs::read_to_string(&path).ok();
    let installed = text.as_deref().and_then(read_plist);
    let (loaded, inspection) = match loaded(context, commands) {
        Ok(loaded) => (loaded, None),
        Err(error) => (None, Some(error)),
    };
    let listen = installed
        .as_ref()
        .map_or(DEFAULT_LISTEN, |installed| installed.listen.as_str());

    let mut out = String::new();
    let mut line = |name: &str, value: String| out.push_str(&format!("{name:<11}{value}\n"));
    line(
        "service",
        match (exists, &installed) {
            (false, _) => format!("{label}: not installed"),
            (true, Some(installed))
                if text
                    .as_deref()
                    .is_some_and(|text| edited(context, installed, text)) =>
            {
                format!(
                    "{label}: installed at {}, edited since install wrote it (update refuses \
                     until the service is installed again)",
                    path.display()
                )
            }
            (true, Some(_)) => format!("{label}: installed at {}", path.display()),
            (true, None) => format!(
                "{label}: {} exists but is not in the form install writes",
                path.display()
            ),
        },
    );
    if let Some(installed) = &installed {
        line(
            "binary",
            format!(
                "{}{}",
                installed.program.display(),
                if installed.program.is_file() {
                    ""
                } else {
                    " (missing)"
                }
            ),
        );
        line(
            "home",
            match &installed.home {
                Some(home) => home.display().to_string(),
                None => "the default Edge home (COMMONMEASURE_HOME is not set)".to_string(),
            },
        );
    }
    let service_pid = loaded.as_ref().and_then(|loaded| loaded.pid);
    line(
        "loaded",
        match (&loaded, &inspection) {
            (None, Some(error)) => format!("unknown: {error}"),
            (None, None) => "no".to_string(),
            (Some(loaded), _) => format!(
                "yes, {}{}",
                loaded.state.as_deref().unwrap_or("state not reported"),
                loaded
                    .pid
                    .map(|pid| format!(", pid {pid}"))
                    .unwrap_or_default()
            ),
        },
    );

    let mut notes = Vec::new();
    let address = match loopback(listen) {
        Ok(address) => Some(address),
        Err(error) => {
            line("listening", format!("unknown: {error}"));
            None
        }
    };
    let mut running_version = None;
    if let Some(address) = address {
        line("port", address.port().to_string());
        let answer = probe(address);
        let holder = match answer {
            Answer::Nothing => None,
            _ => holder(commands, address.port()),
        };
        let by_service = match (&holder, service_pid) {
            (Some(holder), Some(pid)) => holder.pid == pid,
            _ => false,
        };
        let who = match &holder {
            Some(holder) if by_service => format!("pid {}", holder.pid),
            Some(holder) => format!(
                "pid {} ({}), not the service",
                holder.pid,
                holder.command.as_deref().unwrap_or("command not readable")
            ),
            None => "process not named".to_string(),
        };
        line(
            "listening",
            match &answer {
                Answer::Nothing => format!("no, nothing on http://{address}"),
                Answer::Console { version, .. } => {
                    format!("http://{address}, Common Measure {version}, {who}")
                }
                Answer::Unreported => {
                    format!("http://{address}, {who}; the console does not report its version")
                }
            },
        );
        if let Answer::Console { version, .. } = &answer
            && (by_service || (holder.is_none() && loaded.is_some()))
        {
            running_version = Some(version.clone());
        }
        if let Some(holder) = holder.as_ref().filter(|_| !by_service) {
            notes.push(format!(
                "pid {} holds {listen} outside the service. Stop it with kill {}{}.",
                holder.pid,
                holder.pid,
                if installed.is_some() {
                    ""
                } else {
                    ", then run commonmeasure service install console"
                }
            ));
        }
    }

    let path_version = context
        .on_path
        .as_deref()
        .map(|binary| (binary, version_of(context, binary)));
    line(
        "on PATH",
        match &path_version {
            None => "no commonmeasure on PATH".to_string(),
            Some((binary, Ok(version))) => format!("{version} at {}", binary.display()),
            Some((binary, Err(why))) => {
                format!("{}, version unknown ({why})", binary.display())
            }
        },
    );
    line(
        "log",
        installed
            .as_ref()
            .map_or(&context.log, |installed| &installed.log)
            .display()
            .to_string(),
    );

    if let (Some(installed), Some(pid)) = (&installed, service_pid) {
        let restart = format!("commonmeasure service install console --listen {listen}");
        let path_version = path_version
            .as_ref()
            .and_then(|(_, version)| version.as_ref().ok());
        let replaced = match (started(commands, pid), changed(&installed.program)) {
            // A second either way is the clocks' resolution, not an upgrade.
            (Some(started), Some(changed)) => changed > started + Duration::from_secs(2),
            _ => false,
        };
        match (&running_version, path_version) {
            (Some(running), Some(on_path)) if running != on_path => notes.push(format!(
                "The service runs {running}; {on_path} is on PATH. Restart it: {restart}"
            )),
            _ if replaced => notes.push(format!(
                "{} changed after the service started, so the running process is an older \
                 binary{}. Restart it: {restart}",
                installed.program.display(),
                running_version
                    .as_ref()
                    .map(|version| format!(" ({version})"))
                    .unwrap_or_default()
            )),
            _ => {}
        }
        let on_path_file = context
            .on_path
            .as_deref()
            .and_then(|binary| binary.canonicalize().ok());
        if let Some(on_path_file) = on_path_file
            && installed.program.canonicalize().ok().as_ref() != Some(&on_path_file)
        {
            notes.push(format!(
                "The service runs {}, not the commonmeasure on PATH ({}). To run the one on \
                 PATH: {restart}",
                installed.program.display(),
                on_path_file.display()
            ));
        }
    }
    if loaded.as_ref().is_some_and(|loaded| loaded.pid.is_none()) {
        notes.push(format!(
            "The service is loaded but not running. Start it: commonmeasure service install \
             console --listen {listen}"
        ));
    }
    if loaded.is_some() && installed.is_none() {
        notes.push(format!(
            "The service is loaded, but its plist {} is missing or not in the form install \
             writes, so update refuses while it runs the binary being updated. Remove it with \
             commonmeasure service uninstall console, or install it again.",
            path.display()
        ));
    }
    if installed.is_some() && loaded.is_none() && inspection.is_none() {
        notes.push(format!(
            "The plist is installed but not loaded. Load it: commonmeasure service install \
             console --listen {listen}"
        ));
    }
    for note in notes {
        out.push('\n');
        out.push_str(&note);
        out.push('\n');
    }
    Ok(out)
}

/// The command seam and a context under a temporary home, shared by the
/// service and update tests.
#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Answers by command line and records every call, so a test sees what
    /// would have reached the service manager. A `launchctl print` it has no
    /// answer for is answered as launchd answers for a label it has not
    /// loaded; any other command without an answer fails.
    #[derive(Default)]
    pub struct Recorder {
        answers: HashMap<String, (Option<i32>, String, String)>,
        calls: Arc<Mutex<Vec<String>>>,
        console: Option<(String, u16)>,
    }

    impl Recorder {
        pub fn answer(mut self, command: &str, success: bool, stdout: &str) -> Self {
            let code = if success { 0 } else { 1 };
            self.answers.insert(
                command.to_string(),
                (Some(code), stdout.to_string(), String::new()),
            );
            self
        }

        pub fn fail(mut self, command: &str, code: Option<i32>, stderr: &str) -> Self {
            self.answers.insert(
                command.to_string(),
                (code, String::new(), stderr.to_string()),
            );
            self
        }

        /// When `command` runs, start answering `/api/version` on `port` as
        /// a console of release 9.9.9 would, standing in for the process
        /// launchd would start.
        pub fn console_on(mut self, command: &str, port: u16) -> Self {
            self.console = Some((command.to_string(), port));
            self
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        /// The call log, readable from another thread while commands run.
        pub fn log(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.calls)
        }
    }

    impl Commands for Recorder {
        fn run(&self, program: &str, args: &[&str]) -> Result<Output, String> {
            let command = std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.lock().unwrap().push(command.clone());
            if let Some((trigger, port)) = &self.console
                && *trigger == command
            {
                answer_version(*port);
            }
            let (code, stdout, stderr) = match self.answers.get(&command) {
                Some(answer) => answer.clone(),
                None if command.starts_with("launchctl print ") => (
                    Some(113),
                    String::new(),
                    format!("Bad request.\nCould not find service \"{LABEL}\" in domain"),
                ),
                None => (Some(1), String::new(), "no answer".into()),
            };
            Ok(Output {
                success: code == Some(0),
                code,
                stdout,
                stderr,
            })
        }
    }

    fn answer_version(port: u16) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let body = r#"{"product":"commonmeasure","version":"9.9.9","pid":4343}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
    }

    /// The context of a shell whose HOME is `home` and whose Edge home is
    /// the default one under it.
    pub fn context(home: &Path) -> Context {
        let exe = home.join("bin").join("commonmeasure");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "binary").unwrap();
        Context {
            user_home: home.to_path_buf(),
            home_override: None,
            log: home.join(".commonmeasure/logs/console.log"),
            working_directory: home.to_path_buf(),
            exe: exe.clone(),
            on_path: Some(exe),
            uid: 501,
            label: LABEL.to_string(),
            settle: Duration::ZERO,
            // Generous: a script written by a test can take longer than
            // the production bound to launch the first time on a busy host.
            version_wait: Duration::from_secs(30),
        }
    }

    /// The same shell with `COMMONMEASURE_HOME` set to `edge`.
    pub fn with_home(mut context: Context, edge: &Path) -> Context {
        context.home_override = Some(edge.to_path_buf());
        context.log = edge.join("logs/console.log");
        context
    }

    /// `launchctl print` for a job loaded from `context`'s plist as
    /// `context` would write it, in the shape launchd prints.
    pub fn printed(context: &Context, listen: &str, pid: u32) -> String {
        let environment = context
            .home_override
            .as_ref()
            .map(|home| format!("\t\tCOMMONMEASURE_HOME => {}\n", home.display()))
            .unwrap_or_default();
        format!(
            "gui/501/{LABEL} = {{\n\tactive count = 1\n\tpath = {plist}\n\ttype = LaunchAgent\n\
             \tstate = running\n\n\tprogram = {exe}\n\targuments = {{\n\t\t{exe}\n\t\tserve\n\
             \t\t--listen\n\t\t{listen}\n\t}}\n\n\tworking directory = {wd}\n\n\
             \tstdout path = {log}\n\tstderr path = {log}\n\tinherited environment = {{\n\
             \t\tSSH_AUTH_SOCK => /private/tmp/listeners\n\t}}\n\n\tenvironment = {{\n\
             \t\tOSLogRateLimit => 64\n{environment}\t\tXPC_SERVICE_NAME => {LABEL}\n\t}}\n\n\
             \tdomain = gui/501 [100024]\n\tpid = {pid}\n\n\tjetsam coalition = {{\n\
             \t\tstate = active\n\t}}\n}}\n",
            plist = context.plist().display(),
            exe = context.exe.display(),
            wd = context.working_directory.display(),
            log = context.log.display(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Recorder, context, printed, with_home};
    use super::*;
    use std::net::TcpListener;

    /// A loopback port nothing listens on.
    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn the_plist_runs_this_binary_at_login_and_keeps_it_alive() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let text = plist(&context, "127.0.0.1:4173");
        let exe = context.exe.display().to_string();
        assert!(text.contains(&format!("<string>{LABEL}</string>")));
        assert!(text.contains(&format!(
            "<array>\n\t\t<string>{exe}</string>\n\t\t<string>serve</string>\n\t\t\
             <string>--listen</string>\n\t\t<string>127.0.0.1:4173</string>\n\t</array>"
        )));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(text.contains("<key>KeepAlive</key>\n\t<true/>"));
        let log = home.path().join(".commonmeasure/logs/console.log");
        assert!(text.contains(&format!(
            "<key>StandardOutPath</key>\n\t<string>{}</string>",
            log.display()
        )));
        assert!(text.contains(&format!(
            "<key>StandardErrorPath</key>\n\t<string>{}</string>",
            log.display()
        )));
        assert!(!text.contains("EnvironmentVariables"));
        assert_eq!(
            read_plist(&text),
            Some(Installed {
                program: context.exe.clone(),
                listen: "127.0.0.1:4173".into(),
                home: None,
                log,
                working_directory: home.path().to_path_buf(),
            })
        );
    }

    #[test]
    fn a_home_chosen_by_environment_is_carried_into_the_service() {
        let home = tempfile::tempdir().unwrap();
        let context = with_home(context(home.path()), &home.path().join("elsewhere & co"));
        let text = plist(&context, "localhost:4173");
        assert!(text.contains(&format!(
            "<key>COMMONMEASURE_HOME</key>\n\t\t<string>{}</string>",
            home.path().join("elsewhere &amp; co").display()
        )));
        assert!(text.contains("elsewhere &amp; co/logs/console.log"));
    }

    #[test]
    fn a_non_loopback_address_is_refused_before_anything_is_written() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let commands = Recorder::default();
        for listen in ["0.0.0.0:4173", "192.0.2.1:4173", "[::]:4173"] {
            let error = install(&context, &commands, listen).unwrap_err();
            assert!(
                error.contains("reachable from outside this machine"),
                "{error}"
            );
        }
        let error = install(&context, &commands, "127.0.0.1:0").unwrap_err();
        assert!(error.contains("needs a fixed port"), "{error}");
        assert!(commands.calls().is_empty());
        assert!(!context.plist().exists());
    }

    #[test]
    fn install_writes_the_plist_and_bootstraps_it_into_the_gui_domain() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listen = format!("127.0.0.1:{}", free_port());
        let commands = Recorder::default()
            .answer(
                &format!("launchctl bootstrap gui/501 {}", context.plist().display()),
                true,
                "",
            )
            .answer(&format!("launchctl kickstart gui/501/{LABEL}"), true, "");
        // Nothing starts listening under the recorder, so install reports
        // that and names the log.
        let error = install(&context, &commands, &listen).unwrap_err();
        assert!(error.contains("nothing listens"), "{error}");
        assert!(error.contains("console.log"), "{error}");
        assert_eq!(
            commands.calls(),
            vec![
                format!("launchctl print gui/501/{LABEL}"),
                format!("launchctl bootstrap gui/501 {}", context.plist().display()),
                format!("launchctl kickstart gui/501/{LABEL}"),
            ]
        );
        let written = std::fs::read_to_string(context.plist()).unwrap();
        assert_eq!(written, plist(&context, &listen));
        assert!(home.path().join(".commonmeasure/logs").is_dir());
    }

    // EGR-131. Catches: the plist staged through the fixed `plist.tmp`,
    // which a leftover of that name passes its mode to and which two
    // installs share; the plist written owner-only.
    #[cfg(unix)]
    #[test]
    fn install_writes_the_plist_under_the_umask_and_leaves_an_old_temporary_alone() {
        under_umask_022(
            "service::tests::install_writes_the_plist_under_the_umask_and_leaves_an_old_temporary_alone",
            install_writes_the_plist_under_the_umask_and_leaves_an_old_temporary_alone_body,
        );
    }

    /// Runs `body` in a child of this test binary whose umask is 022, set
    /// between fork and exec, and asserts that the child ran the one test
    /// `name` and passed. Under a umask that already masks 066, such as 077,
    /// a file written under the umask and one written owner-only are both
    /// 0600, and a mode comparison cannot tell them apart.
    #[cfg(unix)]
    fn under_umask_022(name: &str, body: impl FnOnce()) {
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

    #[cfg(unix)]
    fn install_writes_the_plist_under_the_umask_and_leaves_an_old_temporary_alone_body() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listen = format!("127.0.0.1:{}", free_port());
        let directory = context.plist().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&directory).unwrap();
        let leftover = context.plist().with_extension("plist.tmp");
        std::fs::write(&leftover, b"left by an older install").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o666)).unwrap();
        let fresh = directory.join("fresh");
        std::fs::write(&fresh, b"").unwrap();
        let commands = Recorder::default()
            .answer(
                &format!("launchctl bootstrap gui/501 {}", context.plist().display()),
                true,
                "",
            )
            .answer(&format!("launchctl kickstart gui/501/{LABEL}"), true, "");

        let error = install(&context, &commands, &listen).unwrap_err();
        assert!(error.contains("nothing listens"), "{error}");

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&context.plist()), mode(&fresh));
        assert_eq!(
            std::fs::read_to_string(context.plist()).unwrap(),
            plist(&context, &listen)
        );
        assert_eq!(
            std::fs::read(&leftover).unwrap(),
            b"left by an older install"
        );
    }

    #[test]
    fn installing_again_boots_out_the_loaded_service_first() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listen = format!("127.0.0.1:{}", free_port());
        let commands = Recorder::default()
            .answer(
                &format!("launchctl print gui/501/{LABEL}"),
                true,
                &format!("gui/501/{LABEL} = {{\n\tstate = running\n\tpid = 4242\n}}\n"),
            )
            .answer(&format!("launchctl bootout gui/501/{LABEL}"), true, "")
            .answer(
                &format!("launchctl bootstrap gui/501 {}", context.plist().display()),
                true,
                "",
            )
            .answer(&format!("launchctl kickstart gui/501/{LABEL}"), true, "");
        let _ = install(&context, &commands, &listen);
        assert_eq!(
            commands.calls(),
            vec![
                format!("launchctl print gui/501/{LABEL}"),
                format!("launchctl bootout gui/501/{LABEL}"),
                format!("launchctl bootstrap gui/501 {}", context.plist().display()),
                format!("launchctl kickstart gui/501/{LABEL}"),
            ]
        );
    }

    #[test]
    fn a_port_held_by_another_process_is_named_and_nothing_changes() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accept and drop, so the probe's connection is answered and closed.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        let commands = Recorder::default()
            .answer(
                &format!("lsof -tnP -iTCP:{port} -sTCP:LISTEN"),
                true,
                "23990\n",
            )
            .answer(
                "ps -o args= -p 23990",
                true,
                "/home/op/.local/bin/commonmeasure serve --listen 127.0.0.1:4173\n",
            );
        let error = install(&context, &commands, &format!("127.0.0.1:{port}")).unwrap_err();
        assert!(error.contains("already in use by pid 23990"), "{error}");
        assert!(error.contains("commonmeasure serve --listen"), "{error}");
        assert!(error.contains("kill 23990"), "{error}");
        assert!(!context.plist().exists());
        assert!(
            !commands
                .calls()
                .iter()
                .any(|call| call.contains("bootstrap"))
        );
    }

    /// A service installed from a shell whose Edge home was `edge`, loaded
    /// and running as pid 7.
    fn installed_for(home: &Path, edge: &Path, listen: &str) -> (Context, String) {
        let installer = with_home(context(home), edge);
        std::fs::create_dir_all(installer.plist().parent().unwrap()).unwrap();
        std::fs::write(installer.plist(), plist(&installer, listen)).unwrap();
        let print = printed(&installer, listen, 7);
        (installer, print)
    }

    #[test]
    fn the_plist_reads_back_the_home_log_and_working_directory() {
        let home = tempfile::tempdir().unwrap();
        let edge = home.path().join("edge a");
        let context = with_home(context(home.path()), &edge);
        let installed = read_plist(&plist(&context, "127.0.0.1:4173")).unwrap();
        assert_eq!(
            installed,
            Installed {
                program: context.exe.clone(),
                listen: "127.0.0.1:4173".into(),
                home: Some(edge.clone()),
                log: edge.join("logs/console.log"),
                working_directory: home.path().to_path_buf(),
            }
        );
        // Written again from what was read, the plist is the same bytes.
        let again = context.installed(&installed);
        assert_eq!(
            plist(&again, "127.0.0.1:4173"),
            plist(&context, "127.0.0.1:4173")
        );

        let other_variable = plist(&context, "127.0.0.1:4173").replace(
            "<key>COMMONMEASURE_HOME</key>",
            "<key>PATH</key>\n\t\t<string>/bin</string>\n\t\t<key>COMMONMEASURE_HOME</key>",
        );
        assert_eq!(read_plist(&other_variable), None);
        let split_logs = plist(&context, "127.0.0.1:4173").replacen(
            "console.log</string>",
            "console.out</string>",
            1,
        );
        assert_eq!(read_plist(&split_logs), None);
    }

    #[test]
    fn launchd_s_copy_of_the_job_is_read_from_print() {
        let home = tempfile::tempdir().unwrap();
        let edge = home.path().join("edge a");
        let context = with_home(context(home.path()), &edge);
        let job = Loaded::parse(&printed(&context, "127.0.0.1:4173", 42));
        assert_eq!(job.pid, Some(42));
        assert_eq!(job.state.as_deref(), Some("running"));
        assert_eq!(job.path, Some(context.plist()));
        assert_eq!(job.program(), Some(context.exe.clone()));
        assert_eq!(job.listen(), Some("127.0.0.1:4173"));
        assert_eq!(job.home, JobHome::Set(edge.clone()));
        assert_eq!(job.stdout, Some(edge.join("logs/console.log")));
        assert_eq!(job.working_directory, Some(home.path().to_path_buf()));
        let default_home = super::testing::context(home.path());
        assert_eq!(
            Loaded::parse(&printed(&default_home, "127.0.0.1:4173", 42)).home,
            JobHome::Unset
        );
    }

    #[test]
    fn the_job_s_home_is_unknown_unless_both_environments_were_read() {
        let block = |lines: &[&str]| Some(lines.iter().map(ToString::to_string).collect());
        let own = block(&["XPC_SERVICE_NAME => ai.commonmeasure.console"]);
        let inherited = block(&["SSH_AUTH_SOCK => /private/tmp/listeners"]);
        assert_eq!(
            JobHome::read(own.clone(), inherited.clone()),
            JobHome::Unset
        );
        assert_eq!(
            JobHome::read(block(&["COMMONMEASURE_HOME => /edge/a"]), inherited.clone()),
            JobHome::Set("/edge/a".into())
        );
        assert_eq!(JobHome::read(None, inherited.clone()), JobHome::Unknown);
        assert_eq!(
            JobHome::read(block(&["COMMONMEASURE_HOME =>"]), inherited.clone()),
            JobHome::Empty
        );
        assert_eq!(JobHome::read(own.clone(), None), JobHome::Unknown);
        assert_eq!(
            JobHome::read(block(&["a line launchd did not print before"]), inherited),
            JobHome::Unknown
        );
        assert_eq!(
            JobHome::read(own.clone(), block(&["COMMONMEASURE_HOME => /edge/b"])),
            JobHome::Inherited {
                own: None,
                inherited: "/edge/b".into()
            }
        );
        // An empty value, as the block's trimmed line holds it.
        assert_eq!(
            JobHome::read(own, block(&["COMMONMEASURE_HOME =>"])),
            JobHome::Inherited {
                own: None,
                inherited: "".into()
            }
        );
        // Both: the job's own value is kept beside the inherited one.
        assert_eq!(
            JobHome::read(
                block(&["COMMONMEASURE_HOME => /edge/a"]),
                block(&["COMMONMEASURE_HOME => /edge/b"])
            ),
            JobHome::Inherited {
                own: Some("/edge/a".into()),
                inherited: "/edge/b".into()
            }
        );
    }

    #[test]
    fn only_a_debug_build_takes_another_service_label() {
        let scratch = Some("ai.commonmeasure.test-1".to_string());
        assert_eq!(
            label(true, scratch.clone()).unwrap(),
            "ai.commonmeasure.test-1"
        );
        assert_eq!(label(false, scratch).unwrap(), LABEL);
        assert_eq!(label(true, None).unwrap(), LABEL);
        assert_eq!(label(true, Some(String::new())).unwrap(), LABEL);
        assert!(label(true, Some("gui/501/x".to_string())).is_err());
        assert_eq!(label(false, Some("gui/501/x".to_string())).unwrap(), LABEL);
    }

    #[test]
    fn the_job_s_own_home_wins_over_an_inherited_one_and_an_empty_one_names_none() {
        let (a, b, empty) = (Path::new("/edge/a"), Path::new("/edge/b"), Path::new(""));
        assert_eq!(effective_home(Some(a), b), Some(a));
        assert_eq!(effective_home(None, b), Some(b));
        assert_eq!(effective_home(Some(empty), b), None);
        assert_eq!(effective_home(None, empty), None);
    }

    #[test]
    fn an_absent_label_is_not_loaded_but_a_failed_inspection_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        assert_eq!(loaded(&context, &Recorder::default()).unwrap(), None);

        let commands = Recorder::default().fail(
            &format!("launchctl print gui/501/{LABEL}"),
            Some(5),
            "Operation not permitted",
        );
        let error = loaded(&context, &commands).unwrap_err();
        assert!(error.contains("exit 5"), "{error}");
        assert!(error.contains("Operation not permitted"), "{error}");
        // 113 without launchd's words for an absent label is not taken as
        // absence either.
        let commands =
            Recorder::default().fail(&format!("launchctl print gui/501/{LABEL}"), Some(113), "");
        assert!(loaded(&context, &commands).is_err());
    }

    #[test]
    fn update_finds_a_service_running_the_binary_with_its_installed_home() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let edge_a = home.path().join("edge-a");
        let (installer, print) = installed_for(home.path(), &edge_a, &listen);
        // The shell running update selects another Edge home.
        let shell = with_home(context(home.path()), &home.path().join("edge-b"));
        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, &print);
        let found = inspect_for_update(&shell, &commands, &shell.exe).unwrap();
        let BeforeUpdate::Runs { installed, pid, .. } = found else {
            panic!("{found:?}");
        };
        assert_eq!(pid, Some(7));
        assert_eq!(installed.home, Some(edge_a.clone()));
        assert_eq!(installed.log, installer.log);
        // Inspecting changes nothing.
        assert_eq!(
            commands.calls(),
            vec![format!("launchctl print gui/501/{LABEL}")]
        );

        let commands = Recorder::default()
            .answer(&format!("launchctl bootout gui/501/{LABEL}"), true, "")
            .answer(
                &format!("launchctl bootstrap gui/501 {}", shell.plist().display()),
                true,
                "",
            )
            .answer(&format!("launchctl kickstart gui/501/{LABEL}"), true, "");
        stop_for_update(&shell, &commands, &installed).unwrap();
        // The plist stays, so the service can be started again on it.
        assert!(shell.plist().exists());
        let _ = start_after_update(&shell, &commands, &installed);
        assert_eq!(
            commands.calls(),
            vec![
                format!("launchctl bootout gui/501/{LABEL}"),
                format!("launchctl print gui/501/{LABEL}"),
                format!("launchctl bootstrap gui/501 {}", shell.plist().display()),
                format!("launchctl kickstart gui/501/{LABEL}"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(shell.plist()).unwrap(),
            plist(&installer, &listen),
            "started again with home A's plist, not the shell's home B"
        );
    }

    #[test]
    fn update_leaves_a_service_running_another_binary_and_ignores_an_unloaded_plist() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, print) = installed_for(home.path(), &home.path().join("a"), &listen);
        let other = home.path().join("elsewhere/commonmeasure");
        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, &print);
        assert_eq!(
            inspect_for_update(&installer, &commands, &other).unwrap(),
            BeforeUpdate::RunsOther(installer.exe.clone())
        );
        assert_eq!(
            inspect_for_update(&installer, &Recorder::default(), &installer.exe).unwrap(),
            BeforeUpdate::Absent
        );
    }

    #[test]
    fn update_refuses_a_loaded_service_whose_plist_is_missing_or_unreadable() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, print) = installed_for(home.path(), &home.path().join("a"), &listen);
        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, &print);

        std::fs::remove_file(installer.plist()).unwrap();
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(error.contains("is loaded (pid 7)"), "{error}");
        assert!(error.contains("cannot be read"), "{error}");
        assert!(
            error.contains("commonmeasure service uninstall console"),
            "{error}"
        );
        assert!(
            error.contains(&format!(
                "env COMMONMEASURE_HOME={} {} service install console --listen {listen}",
                home.path().join("a").display(),
                installer.exe.display()
            )),
            "{error}"
        );

        // A binary plist, as `plutil -convert binary1` leaves it.
        std::fs::write(installer.plist(), b"bplist00\xd1\x01\x02").unwrap();
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(error.contains("cannot be read"), "{error}");

        std::fs::write(installer.plist(), "<plist><dict></dict></plist>").unwrap();
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(error.contains("not in the form install writes"), "{error}");
        // Nothing but the inspection reached launchd.
        assert!(commands.calls().iter().all(|call| call.contains("print")));
    }

    #[test]
    fn update_refuses_when_the_service_cannot_be_inspected() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, _) = installed_for(home.path(), &home.path().join("a"), &listen);
        let commands = Recorder::default().fail(
            &format!("launchctl print gui/501/{LABEL}"),
            Some(125),
            "Domain does not support specified action",
        );
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(error.contains("cannot tell whether"), "{error}");
        assert!(error.contains("exit 125"), "{error}");
    }

    #[test]
    fn update_refuses_a_plist_that_differs_from_the_loaded_job() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, print) = installed_for(home.path(), &home.path().join("a"), &listen);
        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, &print);

        // The plist was rewritten for home B without being loaded.
        let rewritten = with_home(context(home.path()), &home.path().join("b"));
        std::fs::write(installer.plist(), plist(&rewritten, &listen)).unwrap();
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(
            error.contains("differs from the job launchd loaded"),
            "{error}"
        );
        assert!(error.contains("COMMONMEASURE_HOME: loaded"), "{error}");
        assert!(error.contains("stdout: loaded"), "{error}");

        // Or for another address.
        std::fs::write(installer.plist(), plist(&installer, "127.0.0.1:1")).unwrap();
        let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
        assert!(error.contains("arguments: loaded"), "{error}");
    }

    /// Inspect a job that `print` describes, with the plist `context`
    /// installs for the default Edge home on disk.
    fn inspect_print(context: &Context, print: &str) -> (Result<BeforeUpdate, String>, Recorder) {
        std::fs::create_dir_all(context.plist().parent().unwrap()).unwrap();
        std::fs::write(context.plist(), plist(context, "127.0.0.1:4173")).unwrap();
        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, print);
        (
            inspect_for_update(context, &commands, &context.exe),
            commands,
        )
    }

    #[test]
    fn update_refuses_a_job_whose_environment_launchd_did_not_print_as_expected() {
        // The reviewer's first parser probe: the environment block renamed.
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let print = printed(&context, "127.0.0.1:4173", 42);
        assert!(matches!(
            inspect_print(&context, &print).0,
            Ok(BeforeUpdate::Runs { .. })
        ));
        for changed in [
            print.replace("\tenvironment = {", "\tchanged environment = {"),
            print.replace("\tinherited environment = {", "\tinheritance = {"),
        ] {
            let (result, commands) = inspect_print(&context, &changed);
            let error = result.unwrap_err();
            assert!(error.contains("no environment block"), "{error}");
            assert!(
                error.contains("commonmeasure service uninstall console"),
                "{error}"
            );
            assert_eq!(
                commands.calls().len(),
                1,
                "only print: {:?}",
                commands.calls()
            );
        }
        // The block was read, and sets the job's own home empty: the refusal
        // says the value does not establish the home.
        let empty = print.replace(
            "\t\tXPC_SERVICE_NAME =>",
            "\t\tCOMMONMEASURE_HOME => \n\t\tXPC_SERVICE_NAME =>",
        );
        let error = inspect_print(&context, &empty).0.unwrap_err();
        assert!(!error.contains("no environment block"), "{error}");
        assert!(
            error.contains("own COMMONMEASURE_HOME set to \"\" and none inherited"),
            "{error}"
        );
        assert!(
            error.contains("install it again from a shell that selects its Edge home"),
            "{error}"
        );
    }

    #[test]
    fn update_refuses_a_job_that_inherits_its_home_from_launchd() {
        // The reviewer's second parser probe: the plist sets no home, and
        // launchd's domain environment does.
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let print = printed(&context, "127.0.0.1:4173", 42).replace(
            "\tinherited environment = {",
            "\tinherited environment = {\n\t\tCOMMONMEASURE_HOME => /review/edge-a",
        );
        let (result, commands) = inspect_print(&context, &print);
        let error = result.unwrap_err();
        assert!(
            error.contains("COMMONMEASURE_HOME=/review/edge-a"),
            "{error}"
        );
        assert!(
            error.contains("launchctl unsetenv COMMONMEASURE_HOME"),
            "{error}"
        );
        assert!(
            error.contains(&format!(
                "env COMMONMEASURE_HOME=/review/edge-a {} service install console --listen \
                 127.0.0.1:4173",
                context.exe.display()
            )),
            "{error}"
        );
        assert_eq!(commands.calls().len(), 1);
    }

    #[test]
    fn an_inherited_home_is_refused_and_the_recovery_keeps_the_job_s_own_home() {
        // The reviewer's both-blocks probe: the plist sets A, the domain
        // passes B. Launchd applies the job's environment last, so the
        // console runs with A, and reinstalling with B would move it.
        let home = tempfile::tempdir().unwrap();
        let context = with_home(context(home.path()), Path::new("/review/explicit-a"));
        let print = printed(&context, "127.0.0.1:4173", 42).replace(
            "\tinherited environment = {",
            "\tinherited environment = {\n\t\tCOMMONMEASURE_HOME => /review/inherited-b",
        );
        let (result, commands) = inspect_print(&context, &print);
        let error = result.unwrap_err();
        let unset = error
            .find("launchctl unsetenv COMMONMEASURE_HOME")
            .expect(&error);
        let reinstall = error
            .find(&format!(
                "env COMMONMEASURE_HOME=/review/explicit-a {} service install console --listen \
                 127.0.0.1:4173",
                context.exe.display()
            ))
            .expect(&error);
        assert!(unset < reinstall, "{error}");
        assert!(
            !error.contains("env COMMONMEASURE_HOME=/review/inherited-b"),
            "{error}"
        );
        assert!(
            error.contains("COMMONMEASURE_HOME=/review/inherited-b"),
            "{error}"
        );
        assert_eq!(commands.calls().len(), 1);
    }

    #[test]
    fn an_inherited_home_whose_effective_value_is_not_known_offers_no_home() {
        // Through `launchctl print` text: launchd prints an empty value as
        // `COMMONMEASURE_HOME => `, with the trailing space, as the reviewer's
        // scratch job showed.
        let home = tempfile::tempdir().unwrap();
        let inherit = |context: &Context, value: &str| {
            let print = printed(context, "127.0.0.1:4173", 42).replace(
                "\tinherited environment = {",
                &format!("\tinherited environment = {{\n\t\tCOMMONMEASURE_HOME => {value}"),
            );
            let (result, commands) = inspect_print(context, &print);
            assert_eq!(commands.calls().len(), 1, "only print");
            result.unwrap_err()
        };
        let chooses = |error: &str| {
            assert!(error.contains("choose the Edge home"), "{error}");
            assert!(
                error.contains("launchctl unsetenv COMMONMEASURE_HOME"),
                "{error}"
            );
            assert!(!error.contains("env COMMONMEASURE_HOME="), "{error}");
        };

        // Inherited empty, no value of its own.
        let error = inherit(&context(home.path()), "");
        assert!(
            error.contains("launchd passes it COMMONMEASURE_HOME=\"\""),
            "{error}"
        );
        chooses(&error);

        // Its own value empty, which launchd applies over the inherited one.
        let error = inherit(
            &with_home(context(home.path()), Path::new("")),
            "/review/inherited-b",
        );
        assert!(
            error.contains("its own environment sets COMMONMEASURE_HOME=\"\""),
            "{error}"
        );
        assert!(
            error.contains("COMMONMEASURE_HOME=/review/inherited-b"),
            "{error}"
        );
        chooses(&error);

        // Its own value set: that is the home, whatever is inherited.
        let own = with_home(context(home.path()), Path::new("/review/explicit-a"));
        let error = inherit(&own, "");
        assert!(
            error.contains(&format!(
                "env COMMONMEASURE_HOME=/review/explicit-a {} service install console",
                own.exe.display()
            )),
            "{error}"
        );
    }

    #[test]
    fn update_refuses_a_plist_edited_since_install_wrote_it() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, print) = installed_for(home.path(), &home.path().join("a"), &listen);
        // The plist install writes is a function of what it reads back.
        let written = std::fs::read_to_string(installer.plist()).unwrap();
        assert_eq!(written, plist(&installer, &listen));
        assert!(!edited(
            &installer,
            &read_plist(&written).unwrap(),
            &written
        ));

        let commands =
            Recorder::default().answer(&format!("launchctl print gui/501/{LABEL}"), true, &print);
        for edit in [
            // The reviewer's live case.
            written.replace(
                "<key>KeepAlive</key>\n\t<true/>",
                "<key>KeepAlive</key>\n\t<false/>",
            ),
            written.replace(
                "</dict>\n</plist>",
                "\t<key>ProcessType</key>\n\t<string>Background</string>\n</dict>\n</plist>",
            ),
            written.replace("\t<key>RunAtLoad</key>", "\t<key>RunAtLoad</key> "),
        ] {
            assert_ne!(edit, written);
            std::fs::write(installer.plist(), &edit).unwrap();
            let error = inspect_for_update(&installer, &commands, &installer.exe).unwrap_err();
            assert!(
                error.contains("has been edited since service install wrote it"),
                "{error}"
            );
            assert!(
                error.contains(&format!(
                    "env COMMONMEASURE_HOME={} {} service install console --listen {listen}",
                    home.path().join("a").display(),
                    installer.exe.display()
                )),
                "{error}"
            );
            assert_eq!(std::fs::read_to_string(installer.plist()).unwrap(), edit);
        }
        assert!(commands.calls().iter().all(|call| call.contains("print")));
    }

    #[test]
    fn status_and_uninstall_name_the_installed_log_not_the_shell_s() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let edge_a = home.path().join("edge-a");
        let (installer, print) = installed_for(home.path(), &edge_a, &listen);
        let shell = with_home(context(home.path()), &home.path().join("edge-b"));
        let commands = Recorder::default()
            .answer(&format!("launchctl print gui/501/{LABEL}"), true, &print)
            .answer("ps -o etime= -p 7", true, "00:00\n");
        let text = status(&shell, &commands).unwrap();
        assert!(
            text.contains(&format!("log        {}", installer.log.display())),
            "{text}"
        );
        assert!(
            text.contains(&format!("home       {}", edge_a.display())),
            "{text}"
        );
        assert!(!text.contains("edge-b"), "{text}");

        let commands = Recorder::default()
            .answer(&format!("launchctl print gui/501/{LABEL}"), true, &print)
            .answer(&format!("launchctl bootout gui/501/{LABEL}"), true, "");
        let text = uninstall(&shell, &commands).unwrap();
        assert!(
            text.contains(&format!("log kept   {}", installer.log.display())),
            "{text}"
        );
    }

    #[test]
    fn status_says_when_the_plist_was_edited_by_hand() {
        let home = tempfile::tempdir().unwrap();
        let listen = format!("127.0.0.1:{}", free_port());
        let (installer, _) = installed_for(home.path(), &home.path().join("a"), &listen);
        let text = status(&installer, &Recorder::default()).unwrap();
        assert!(!text.contains("edited"), "{text}");
        let written = std::fs::read_to_string(installer.plist()).unwrap();
        std::fs::write(
            installer.plist(),
            written.replace(
                "<key>KeepAlive</key>\n\t<true/>",
                "<key>KeepAlive</key>\n\t<false/>",
            ),
        )
        .unwrap();
        let text = status(&installer, &Recorder::default()).unwrap();
        assert!(text.contains("edited since install wrote it"), "{text}");
    }

    #[test]
    fn status_says_when_launchd_could_not_be_asked() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let commands = Recorder::default().fail(
            &format!("launchctl print gui/501/{LABEL}"),
            Some(5),
            "Operation not permitted",
        );
        let text = status(&context, &commands).unwrap();
        assert!(text.contains("loaded     unknown: "), "{text}");
    }

    #[test]
    fn the_reinstall_command_carries_the_installed_home() {
        let installed = Installed {
            program: PathBuf::from("/home/op/.local/bin/commonmeasure"),
            listen: "127.0.0.1:4173".into(),
            home: Some(PathBuf::from("/home/op/edge a")),
            log: PathBuf::from("/home/op/edge a/logs/console.log"),
            working_directory: PathBuf::from("/home/op"),
        };
        assert_eq!(
            reinstall_command(&installed),
            "env COMMONMEASURE_HOME='/home/op/edge a' /home/op/.local/bin/commonmeasure service \
             install console --listen 127.0.0.1:4173"
        );
        let default = Installed {
            home: None,
            ..installed
        };
        assert!(reinstall_command(&default).starts_with("env -u COMMONMEASURE_HOME "));
    }

    #[test]
    fn uninstall_boots_out_and_removes_the_plist() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        std::fs::create_dir_all(context.plist().parent().unwrap()).unwrap();
        std::fs::write(context.plist(), plist(&context, DEFAULT_LISTEN)).unwrap();
        let commands = Recorder::default()
            .answer(
                &format!("launchctl print gui/501/{LABEL}"),
                true,
                "\tpid = 7\n",
            )
            .answer(&format!("launchctl bootout gui/501/{LABEL}"), true, "");
        let text = uninstall(&context, &commands).unwrap();
        assert!(text.contains("stopped and removed"), "{text}");
        assert!(!context.plist().exists());

        let commands = Recorder::default();
        let text = uninstall(&context, &commands).unwrap();
        assert!(text.contains("not installed"), "{text}");
    }

    #[test]
    fn status_reports_a_service_running_a_binary_replaced_since_it_started() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listen = format!("127.0.0.1:{}", free_port());
        std::fs::create_dir_all(context.plist().parent().unwrap()).unwrap();
        std::fs::write(context.plist(), plist(&context, &listen)).unwrap();
        // The binary was written just now; the process has run for an hour.
        let commands = Recorder::default()
            .answer(
                &format!("launchctl print gui/501/{LABEL}"),
                true,
                "\tstate = running\n\tpid = 4242\n",
            )
            .answer("ps -o etime= -p 4242", true, "   01:00:00\n");
        let text = status(&context, &commands).unwrap();
        assert!(text.contains("installed at"), "{text}");
        assert!(text.contains("yes, running, pid 4242"), "{text}");
        assert!(
            text.contains(&format!("on PATH    {}", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        assert!(text.contains("changed after the service started"), "{text}");
        assert!(
            text.contains(&format!(
                "Restart it: commonmeasure service install console --listen {listen}"
            )),
            "{text}"
        );
    }

    #[test]
    fn status_of_a_current_service_raises_nothing() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listen = format!("127.0.0.1:{}", free_port());
        std::fs::create_dir_all(context.plist().parent().unwrap()).unwrap();
        std::fs::write(context.plist(), plist(&context, &listen)).unwrap();
        let commands = Recorder::default()
            .answer(
                &format!("launchctl print gui/501/{LABEL}"),
                true,
                "\tstate = running\n\tpid = 4242\n",
            )
            .answer("ps -o etime= -p 4242", true, "00:00\n");
        let text = status(&context, &commands).unwrap();
        assert!(!text.contains("Restart"), "{text}");
        assert!(text.contains("no, nothing on"), "{text}");
    }

    #[test]
    fn status_without_a_service_says_so_on_the_default_port() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let text = status(&context, &Recorder::default()).unwrap();
        assert!(text.contains(&format!("{LABEL}: not installed")), "{text}");
        assert!(text.contains("loaded     no"), "{text}");
        assert!(text.contains("port       4173"), "{text}");
    }

    /// `status` asks another `commonmeasure` on PATH through the bounded
    /// helper: one that floods its output is stopped at the limit and its
    /// version reported unknown. The test context allows 30 s, so a slow
    /// first launch of the script cannot turn the flood into a timeout.
    #[test]
    #[cfg(unix)]
    fn status_stops_a_commonmeasure_on_path_that_floods_its_version() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let mut context = context(home.path());
        let flood = home.path().join("path").join("commonmeasure");
        std::fs::create_dir_all(flood.parent().unwrap()).unwrap();
        std::fs::write(&flood, "#!/bin/sh\nhead -c 8192 /dev/zero | tr '\\0' x\n").unwrap();
        std::fs::set_permissions(&flood, std::fs::Permissions::from_mode(0o755)).unwrap();
        context.on_path = Some(flood.clone());
        let commands = Recorder::default();
        let text = status(&context, &commands).unwrap();
        assert!(
            text.contains(&format!(
                "on PATH    {}, version unknown (--version printed more than 4096 bytes and \
                 was stopped)",
                flood.display()
            )),
            "{text}"
        );
        let calls = commands.calls();
        assert!(
            calls
                .iter()
                .filter(|call| call.starts_with("launchctl "))
                .all(|call| call == &format!("launchctl print gui/501/{LABEL}")),
            "{calls:?}"
        );
    }

    /// A `commonmeasure` on PATH that never answers `--version` holds
    /// `status` for `version_wait` only. `status` runs on a thread, so a
    /// wait that ignores the bound fails here at 10 s rather than when the
    /// script's `sleep 60` ends.
    #[test]
    #[cfg(unix)]
    fn status_stops_a_commonmeasure_on_path_that_hangs_on_its_version() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let mut context = context(home.path());
        context.version_wait = Duration::from_secs(1);
        let hangs = home.path().join("path").join("commonmeasure");
        std::fs::create_dir_all(hangs.parent().unwrap()).unwrap();
        std::fs::write(&hangs, "#!/bin/sh\nsleep 60\n").unwrap();
        std::fs::set_permissions(&hangs, std::fs::Permissions::from_mode(0o755)).unwrap();
        context.on_path = Some(hangs.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(status(&context, &Recorder::default())));
        let text = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("status was still waiting for --version after 10 s")
            .unwrap();
        assert!(
            text.contains(&format!(
                "on PATH    {}, version unknown (--version did not finish within 1 s and was \
                 stopped)",
                hangs.display()
            )),
            "{text}"
        );
    }

    #[test]
    fn status_names_a_console_started_by_hand_on_the_service_port() {
        let home = tempfile::tempdir().unwrap();
        let context = context(home.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        let listen = format!("127.0.0.1:{port}");
        std::fs::create_dir_all(context.plist().parent().unwrap()).unwrap();
        std::fs::write(context.plist(), plist(&context, &listen)).unwrap();
        let commands = Recorder::default()
            .answer(
                &format!("lsof -tnP -iTCP:{port} -sTCP:LISTEN"),
                true,
                "23990\n",
            )
            .answer(
                "ps -o args= -p 23990",
                true,
                "/home/op/.local/bin/commonmeasure serve\n",
            );
        let text = status(&context, &commands).unwrap();
        assert!(text.contains("loaded     no"), "{text}");
        assert!(
            text.contains("pid 23990 (/home/op/.local/bin/commonmeasure serve), not the service"),
            "{text}"
        );
        assert!(text.contains("does not report its version"), "{text}");
        assert!(
            text.contains(&format!("pid 23990 holds {listen} outside the service")),
            "{text}"
        );
        assert!(text.contains("not loaded. Load it"), "{text}");
    }

    #[test]
    fn elapsed_time_reads_every_ps_form() {
        assert_eq!(elapsed("00:07"), Some(Duration::from_secs(7)));
        assert_eq!(elapsed("12:34"), Some(Duration::from_secs(754)));
        assert_eq!(elapsed("01:00:00"), Some(Duration::from_secs(3600)));
        assert_eq!(
            elapsed("2-01:00:00"),
            Some(Duration::from_secs(2 * 86_400 + 3600))
        );
        assert_eq!(elapsed("soon"), None);
    }
}
