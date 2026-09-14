//! The Common Measure command line (binary `commonmeasure`).
//!
//! Deliberately thin. Everything that decides, calls or records lives in
//! `commonmeasure-runtime`, so there is no behaviour here that a test of the runtime
//! would miss, and no path by which the CLI can produce a result the runtime
//! would not.

mod inspect;

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use commonmeasure_harness::{
    HostSurface, SessionLog, home_dir, mcp::McpServer, policy::SessionPolicy,
};
use commonmeasure_runtime::{RunOptions, load_suite};

/// The hosts the harness commands accept, as `--host` names them.
/// The hosts whose hook payloads the observed path reads: Claude Code's
/// shape, and Cursor's mapped onto it.
const HOSTS: [&str; 4] = ["claude-code", "codex", "pi", "cursor"];

/// The hosts the mediated server accepts under `--host`: the three with a
/// registration, and the four the host-surfaces investigation found reach
/// the server through a configuration write, so their sessions record the
/// host they came from before an `install` for them exists. A value not
/// listed is refused by the argument parser with this list, never recorded
/// as `claude-code`.
const MCP_HOSTS: [&str; 7] = [
    "claude-code",
    "codex",
    "pi",
    "claude-desktop",
    "cursor",
    "copilot-cli",
    "vscode",
];

#[derive(Parser)]
#[command(
    name = "commonmeasure",
    version,
    about = "Records, checks and reports the content an AI agent takes in.",
    long_about = "Records, checks and reports the content an AI agent takes in.\n\n\
                  `hook` and `mcp` are the harness side: they record every crossing a host \
                  reports and, for the mediated tools, apply the operator's policy before \
                  the content moves. `run` and `inspect` are the batch side: a job is run \
                  through named supply plans and published as an inspectable run directory. \
                  `serve` renders the operator console over the local record, and `relay` is \
                  the only way records leave the machine."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum PolicyAction {
    /// Print the canonical pre-image of the effective policy for this
    /// directory and principal, and its digest, so a reviewer can recompute
    /// the identity every mediated crossing here records
    /// (docs/contracts/fleet-status.md).
    Identity,
    /// Fetch the desired policy from the pinned signer named in
    /// deployment.json, verify it, and activate it through the policy
    /// loader; keep the policy already in force when anything fails
    /// (docs/contracts/policy-envelope.md). A local edge refuses and makes
    /// no request.
    Sync,
}

#[derive(Subcommand)]
enum Command {
    /// Run one job through each named supply plan and publish the run
    /// directory: the sealed manifest, the exact provider responses, the
    /// admitted context, the inference record and an append-only evidence
    /// log. External provider calls happen only with --live, because they
    /// cost money. With --replay <dir>, provider plans are served from that
    /// directory's verified recorded responses through the same adapters and
    /// policy. Inference happens only when COMMONMEASURE_INFERENCE_ENDPOINT names
    /// a reachable OpenAI-compatible gateway. Where a dependency is absent the
    /// run records an explicit unavailable result.
    Run {
        /// Path to the job JSON.
        suite: PathBuf,
        /// Directory to publish the run into. An existing run there is
        /// replaced only once the new one is complete.
        #[arg(long)]
        output: PathBuf,
        /// Authorise external provider calls. These are billable, so they are
        /// off unless asked for.
        #[arg(long)]
        live: bool,
        /// Serve every provider plan from the verified recordings in this
        /// directory (its `replay-manifest.json` names them) through real
        /// loopback origins, adapters and policy. No external acquisition
        /// call is made, and a provider without a recording fails the run
        /// rather than falling back to live access.
        #[arg(long, value_name = "DIR", conflicts_with = "live")]
        replay: Option<PathBuf>,
    },
    /// Print the run dossier and the record each claim cites. Reads only the
    /// published artefacts.
    Inspect {
        /// A run directory previously written by `run`.
        run: PathBuf,
    },
    /// Record one harness lifecycle event. Reads the host's hook payload on
    /// stdin and always exits zero: capture must never break the agent. On
    /// `session-start` it also emits the standing mediation nudge on stdout,
    /// which the host adds to the session's context.
    Hook {
        /// The event, as the host names it (`post-tool-use`).
        event: String,
        /// Which host is calling: `claude-code`, `codex`, `pi` or `cursor`.
        /// The payload is read in that host's shape, and a payload of
        /// another host's shape records nothing.
        #[arg(long, default_value = "claude-code", value_parser = HOSTS)]
        host: String,
    },
    /// Serve the mediated context tools over MCP on stdio.
    Mcp {
        /// Which host is calling: `claude-code`, `codex`, `pi`,
        /// `claude-desktop`, `cursor`, `copilot-cli` or `vscode`. Recorded as
        /// `host` on every record; the client's own name and version from
        /// the protocol's initialize request are recorded beside it.
        #[arg(long, default_value = "claude-code", value_parser = MCP_HOSTS)]
        host: String,
        /// Session identifier. The host supplies one per conversation; a fresh
        /// one is generated when it does not.
        #[arg(long)]
        session: Option<String>,
    },
    /// Report where provider credentials come from and which providers are
    /// configured. Names variables, paths and digests; never prints a value.
    Credentials,
    /// Register this binary with a host, by absolute path, in the host's own
    /// configuration: Claude Code gets the four hooks and the MCP server at
    /// user scope; Codex gets the MCP server table, with the approval mode
    /// that lets its non-interactive runs call the tools; Pi gets an
    /// extension that runs the server, because Pi has no MCP client; Claude
    /// Desktop gets the MCP server in its configuration file; Cursor gets
    /// the MCP server and four hooks in its global files. Only this
    /// product's entries are written; everything else in the host's files
    /// is kept as read.
    Install {
        /// The host: `claude`, `codex`, `pi`, `claude-desktop` or `cursor`.
        host: String,
        /// Register this path instead of the running binary. It must exist;
        /// the registration names it as resolved.
        #[arg(long)]
        binary: Option<PathBuf>,
    },
    /// Remove exactly the entries `install` wrote for a host. The evidence in
    /// the operator home is never touched.
    Uninstall {
        /// The host: `claude`, `codex`, `pi`, `claude-desktop` or `cursor`.
        host: String,
    },
    /// Report each host's registration against the machine: what is
    /// registered and where, which binary it names and the version that
    /// binary reports, whether a session log can be written, and whether the
    /// policy file loads.
    Doctor {
        /// One host (`claude`, `codex`, `pi`, `claude-desktop` or `cursor`);
        /// every host when omitted.
        host: Option<String>,
    },
    /// Reconstruct crossings from host transcripts for work done before
    /// Common Measure was installed.
    Import {
        /// Only import sessions that started on or after this date
        /// (`YYYY-MM-DD`).
        #[arg(long)]
        since: Option<String>,
        /// Report what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print what a harness session recorded.
    Session {
        /// Session id, or the most recent session when omitted.
        session: Option<String>,
    },
    /// Print this edge's fleet-status document: the identity of the policy
    /// in force for this directory and principal, the declared policy's
    /// digest, versions, the last enforcement time and a bounded allowance
    /// summary (docs/contracts/fleet-status.md). No policy text, prompt,
    /// answer, per-crossing spend or engagement name is in it. Nothing is
    /// sent anywhere: this prints what a receiver would be given.
    Status {
        /// Print the document itself rather than the readable summary.
        #[arg(long)]
        json: bool,
    },
    /// The policy in force: its identity, and, on a managed edge, its
    /// synchronisation with the pinned signer's desired policy.
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Deliver the Content Telemetry projection of the evidence logs to a
    /// configured receiver. It sends witnessed retrieval and grounding facts
    /// only; nothing else leaves the machine (docs/contracts/session-evidence.md).
    /// With no receiver configured this command refuses and sends nothing:
    /// there is no default egress.
    ///
    /// The report names the governing engagements whose policy.json clearance
    /// let each delivered event leave, the sessions withheld because nothing in
    /// them was cleared, and any events carrying no engagement at all. The
    /// reported engagement the console counts work under is a different
    /// identity and is not read here; the console names both and where they
    /// differ.
    Relay {
        /// Receiver base URL, e.g. `http://localhost:8080`. Overrides the
        /// `receiver` in `relay.json` for this invocation.
        #[arg(long)]
        receiver: Option<String>,
        /// API key the receiver expects, sent as `X-API-Key`. Overrides the
        /// key in `relay.json` for this invocation.
        #[arg(long)]
        api_key: Option<String>,
        /// A published run directory to project alongside the session logs.
        /// Repeatable.
        #[arg(long)]
        run: Vec<PathBuf>,
        /// Project only this session. Repeatable; every recorded session when
        /// omitted.
        #[arg(long)]
        session: Vec<String>,
    },
    /// Enrol this edge with a Common Measure Hub in one command. Exchanges
    /// the owner's short-lived token for an org-scoped ingest key, mints
    /// the edge's signing key (the private half never leaves
    /// ~/.commonmeasure/), registers the public half under the organisation,
    /// writes the relay configuration, and makes a first relay run through
    /// the real delivery path. There is no default hub: with nothing named
    /// this refuses. Enrolment does not change policy mode.
    Connect {
        /// The hub's base URL, e.g. `https://hub.example`.
        hub: Option<String>,
        /// The enrolment token an owner minted in the hub, single-use and
        /// short-lived.
        #[arg(long)]
        token: Option<String>,
        /// Also take the organisation's policy from this hub: read the hub's
        /// policy signer under the new ingest key, pin it in
        /// ~/.commonmeasure/deployment.json as `managed`, and make a first
        /// policy synchronisation. Without it the edge stays in `local`
        /// mode and enrolment changes no policy.
        #[arg(long)]
        managed: bool,
    },
    /// Leave the hub: revoke this edge's key and ingest key there when it
    /// can be reached, and remove ~/.commonmeasure/relay.json, the private
    /// key and the enrolment record either way.
    Disconnect,
    /// Read the provenance label back from a text output: extract the C2PA
    /// manifest embedded under Annex A.8, validate it against the text it
    /// is bound to, and print what it claims (ingredients by content hash
    /// and grade, the source record's identifiers, the operator's
    /// declaration and identity, the validation state) as JSON. Reads the
    /// file only; makes no network call. A file with no label is an error.
    /// With --run, the source record the run's summary implies is re-derived
    /// and compared with the label's field by field; a difference is
    /// reported and the command fails.
    Provenance {
        /// A labelled output: a `provenance/<plan>.txt` from a run, or the
        /// same text pasted into any file.
        file: PathBuf,
        /// A run directory whose `summary.json` the label's source record is
        /// checked against.
        #[arg(long)]
        run: Option<PathBuf>,
    },
    /// Serve the operator console on loopback: Overview, Record, Policy,
    /// Sources and Compare, rendered from the local evidence logs.
    /// Write the guide's public education pages into a directory, one
    /// self-contained HTML file per source, with every product panel
    /// stripped and no product name in the output; the console serves the
    /// product variant at /guide.
    Guide {
        /// Directory to write into; created if absent.
        out: PathBuf,
    },
    Serve {
        /// Address to listen on. Loopback by default, because the console is
        /// the operator reading their own record. A non-loopback address is
        /// refused without --allow-remote.
        #[arg(long, default_value = "127.0.0.1:4173")]
        listen: String,
        /// Permit a --listen that is reachable from off this machine. The
        /// console has no credential of any kind, so this publishes every
        /// recorded URL, host, cwd and session, and the Compare form, to
        /// anyone who can reach the address. A loopback --listen still
        /// answers only to loopback names, with or without this flag.
        #[arg(long)]
        allow_remote: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run {
            suite,
            output,
            live,
            replay,
        } => run(&suite, output, live, replay.as_deref()),
        Command::Inspect { run } => inspect::inspect(&run),
        Command::Hook { event, host } => return hook(&event, &host),
        Command::Mcp { host, session } => serve_mcp(&host, session.as_deref()),
        Command::Credentials => credentials_report(),
        Command::Install { host, binary } => install_host(&host, binary.as_deref()),
        Command::Uninstall { host } => uninstall_host(&host),
        Command::Doctor { host } => doctor(host.as_deref()),
        Command::Import { since, dry_run } => import(since.as_deref(), dry_run),
        Command::Session { session } => show_session(session.as_deref()),
        Command::Status { json } => show_status(json),
        Command::Policy { action } => match action {
            PolicyAction::Identity => show_policy_identity(),
            PolicyAction::Sync => sync_policy(),
        },
        Command::Relay {
            receiver,
            api_key,
            run,
            session,
        } => relay(receiver, api_key, run, session),
        Command::Connect {
            hub,
            token,
            managed,
        } => connect(hub.as_deref(), token.as_deref(), managed),
        Command::Disconnect => disconnect(),
        Command::Guide { out } => write_guide(&out),
        Command::Provenance { file, run } => provenance_report(&file, run.as_deref()),
        Command::Serve {
            listen,
            allow_remote,
        } => serve_console(listen, allow_remote),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("commonmeasure: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The principal's allowance for a live-capable run: the same home, policy
/// document and principal resolution every harness command uses, so the run
/// path cannot acquire a second identity scheme. A principal that declares
/// no allowance still gets a context, so the quote record can say "declares
/// no allowance" rather than the weaker "no context was held"; `None` is
/// only a machine with no resolvable home. A policy file that exists but
/// does not parse is an error, because running as if it declared nothing
/// would enforce less than the operator wrote. Replay runs never get here:
/// no money moves, so the allowance ledger is not consulted
/// (`crates/commonmeasure-runtime/src/replay.rs`).
fn resolved_allowance() -> Result<Option<commonmeasure_runtime::allowance::AllowanceContext>, String>
{
    let Ok(home) = commonmeasure_harness::home_dir() else {
        return Ok(None);
    };
    let policy = commonmeasure_harness::policy::SessionPolicy::load(
        &home,
        std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.to_str().map(str::to_owned))
            .as_deref(),
    )?;
    Ok(Some(
        commonmeasure_runtime::allowance::AllowanceContext::new(
            &home,
            policy.principal().to_owned(),
            policy.allowances().to_vec(),
        ),
    ))
}

fn run(
    suite_path: &Path,
    output: PathBuf,
    live: bool,
    replay: Option<&Path>,
) -> Result<(), String> {
    let suite = load_suite(suite_path).map_err(|error| error.to_string())?;
    let report = match replay {
        Some(directory) => {
            // The supply must outlive the run: dropping it stops the loopback
            // origins the resolver aims the adapters at.
            let supply = commonmeasure_runtime::ReplaySupply::for_suite(directory, &suite)
                .map_err(|error| error.to_string())?;
            commonmeasure_runtime::execute(&suite, &supply.run_options(output))
        }
        None => {
            let mut options = RunOptions::from_environment(output, live);
            options.allowance = resolved_allowance()?;
            commonmeasure_runtime::execute(&suite, &options)
        }
    }
    .map_err(|error| error.to_string())?;
    println!(
        "run {} published to {}\nmanifest {}",
        report.run_id,
        report.output.display(),
        report.manifest_hash
    );
    Ok(())
}

/// The second session's view of a labelled output: what the embedded
/// manifest claims, as the C2PA reader finds it, with nothing taken from the
/// run directory unless `--run` names one. That independence is the point:
/// the label has to carry its own provenance. Given a run, the source record
/// its summary implies is re-derived and every difference from the label's
/// is reported, and a difference fails the command.
fn provenance_report(file: &Path, run: Option<&Path>) -> Result<(), String> {
    use commonmeasure_runtime::processor::provenance;
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    let read =
        provenance::read_back(&text).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut report = serde_json::to_value(&read).map_err(|error| error.to_string())?;
    let mut differences = Vec::new();
    if let Some(run) = run {
        let summary_path = run.join("summary.json");
        let summary: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&summary_path)
                .map_err(|error| format!("cannot read {}: {error}", summary_path.display()))?,
        )
        .map_err(|error| format!("{} is not JSON: {error}", summary_path.display()))?;
        let plan_id = read.source_record["plan_id"]
            .as_str()
            .ok_or("the label's source record names no plan")?;
        let implied = provenance::record_from_summary(&summary, plan_id)?;
        differences = provenance::record_differences(&read.source_record, &implied);
        report["record"] = serde_json::json!({
            "run": run.display().to_string(),
            "plan_id": plan_id,
            "matches": differences.is_empty(),
            "differences": differences,
        });
    }
    let mut out = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    out.push('\n');
    write_stdout(&out)?;
    if differences.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the label's source record differs from the record {} implies: {}",
            run.map(|run| run.display().to_string()).unwrap_or_default(),
            differences.join("; ")
        ))
    }
}

/// Write a whole document to stdout, treating a closed pipe as success. These
/// commands exist to be read, which usually means through `head` or a pager,
/// and failing when the reader stops first would be the tool complaining about
/// being used as intended.
fn write_stdout(out: &str) -> Result<(), String> {
    match std::io::stdout().write_all(out.as_bytes()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(format!("cannot write to stdout: {error}")),
    }
}

/// Record one hook event.
///
/// Exits zero whatever happens, because capture must not break the agent
/// when capture is unavailable (`AGENTS.md`). It must never claim to have
/// recorded something it did not: a failure to append leaves this log
/// instance owing a gap, which a later append through the same instance
/// materialises. A hook process usually
/// makes one append and exits, so a failed one commonly leaves no gap record at
/// all — session-log completeness is per-process
/// (`docs/contracts/session-evidence.md` §Where).
fn hook(event: &str, host: &str) -> ExitCode {
    let mut raw = String::new();
    let surface = HostSurface::parse(host).unwrap_or(HostSurface::ClaudeCode);
    // The payload in the named host's shape; another host's shape, or no
    // payload, records nothing. Cursor and VS Code load Claude Code's hook
    // file and run its commands with their own payloads, which is why the
    // reader is told which host it is reading.
    let value = match std::io::stdin().read_to_string(&mut raw) {
        Ok(_) => serde_json::from_str::<serde_json::Value>(&raw).ok(),
        Err(_) => None,
    };
    // Cursor sets CURSOR_PROJECT_DIR for every hook it runs, its own and the
    // Claude Code ones it loads as third-party hooks; a command told it is
    // reading Claude Code and running under Cursor reads nothing, whatever
    // the payload's shape, because Cursor's documentation does not say
    // which shape those hooks receive.
    let under_cursor =
        surface != HostSurface::Cursor && std::env::var_os("CURSOR_PROJECT_DIR").is_some();
    // Another host's payload, or Cursor's environment, is refused: nothing
    // is read and, at session start, nothing is printed. A Claude Code
    // payload that is merely unreadable keeps the nudge, because delivery
    // is the point and an unreadable payload costs the record and not the
    // delivery.
    let refused = under_cursor
        || value
            .as_ref()
            .is_some_and(|value| commonmeasure_harness::HookInput::is_foreign(surface, value));
    let parsed = value
        .as_ref()
        .filter(|_| !refused)
        .and_then(|value| commonmeasure_harness::HookInput::from_payload(surface, value));
    // Cursor reads a hook's stdout as JSON: the nudge travels as
    // `additional_context`, a prompt hook answers `continue`, and the rest
    // answer an empty object. Claude Code reads plain text at session start
    // and nothing otherwise.
    let cursor_answer = |body: &str| {
        if surface == HostSurface::Cursor {
            let _ = std::io::stdout().write_all(body.as_bytes());
        }
    };

    // The standing mediation nudge (`commonmeasure_harness::nudge`): the host adds a
    // SessionStart hook's stdout to the session's context, so emitting is the
    // whole delivery. It does not depend on the payload — that is needed only
    // to record the issuance, which is best-effort like all capture: an
    // unreadable payload, a missing home or an unwritable log costs the
    // record, never the nudge and never the exit code.
    if event == "session-start" {
        if refused {
            return ExitCode::SUCCESS;
        }
        if surface == HostSurface::Cursor {
            let _ = std::io::stdout().write_all(
                serde_json::json!({"additional_context": commonmeasure_harness::nudge::TEXT})
                    .to_string()
                    .as_bytes(),
            );
        } else {
            let _ = std::io::stdout().write_all(commonmeasure_harness::nudge::TEXT.as_bytes());
        }
        if let Some(input) = &parsed
            && let Ok(home) = home_dir()
        {
            let session_id = input.session_id.as_deref().unwrap_or("unknown-session");
            if let Ok(mut log) = SessionLog::open(&home, session_id) {
                // A managed edge refreshes its policy first, so the session
                // runs under the revision the hub desires now and the
                // records below name that policy. The nudge is already out
                // and the exit code is fixed, so a hub that does not answer
                // costs the wait and nothing else.
                if let Some(sync) =
                    sync_managed_policy(&home, commonmeasure_harness::managed::SESSION_START_BUDGET)
                {
                    let _ = log.record_policy_sync(surface.id(), "session_start", sync);
                }
                // The session starts under a policy, and the record names
                // which: the identity a mediated crossing in this directory
                // would meet, or the reason the policy could not be read.
                let policy = SessionPolicy::load(&home, input.cwd.as_deref());
                let _ = log.record_nudge(
                    surface.id(),
                    input.source.as_deref(),
                    &commonmeasure_harness::boundary_policy(policy.as_ref()),
                );
                // An enrolled edge names its key id in every session from
                // the moment it enrolled; an unreadable record costs the
                // line, never the nudge.
                if let Ok(Some(enrolment)) = commonmeasure_harness::EnrolmentRecord::load(&home) {
                    let _ = log.record_edge_identity(surface.id(), &enrolment);
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    let Some(mut input) = parsed else {
        cursor_answer(if event == "user-prompt-submit" {
            r#"{"continue":true}"#
        } else {
            "{}"
        });
        return ExitCode::SUCCESS;
    };
    if input.hook_event_name.is_none() {
        input.hook_event_name = Some(match event {
            "post-tool-use" => "PostToolUse".to_owned(),
            "user-prompt-submit" => "UserPromptSubmit".to_owned(),
            "stop" => "Stop".to_owned(),
            other => other.to_owned(),
        });
    }
    cursor_answer(
        if input.hook_event_name.as_deref() == Some("UserPromptSubmit") {
            r#"{"continue":true}"#
        } else {
            "{}"
        },
    );
    let session_id = input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown-session".to_owned());

    let Ok(home) = home_dir() else {
        return ExitCode::SUCCESS;
    };
    // The operator's policy, resolved against the directory the host reports,
    // read once for two purposes: its named exceptions to the privacy floor,
    // and the identity a turn boundary records. A policy that cannot be read
    // lowers nothing: the floor is the default, and a hook is the one place
    // a load error may not interrupt anybody — the mediated server, which
    // can refuse, is where a broken policy is surfaced.
    let policy = SessionPolicy::load(&home, input.cwd.as_deref());
    let internal_prefixes = policy
        .as_ref()
        .map(|policy| policy.internal_prefixes().to_vec())
        .unwrap_or_default();
    let Ok(mut log) = SessionLog::open(&home, &session_id) else {
        return ExitCode::SUCCESS;
    };
    for crossing in commonmeasure_harness::capture(&input, surface, &internal_prefixes) {
        let _ = log.record_crossing(&crossing);
    }
    // The prompt's URLs as hashes and the statements pasted material carries
    // (`commonmeasure_harness::prompt`). Written before the turn boundary so
    // a mediated fetch in the same turn finds it. No prompt text is kept.
    if input.hook_event_name.as_deref() == Some("UserPromptSubmit")
        && let Some(prompt) = input.prompt.as_deref()
    {
        let scan = commonmeasure_harness::prompt::scan(prompt);
        let _ = log.record_prompt_sources(surface.id(), &scan);
    }
    if let Some(boundary) = match input.hook_event_name.as_deref() {
        Some("UserPromptSubmit") => Some("turn_started"),
        Some("Stop") => Some("turn_completed"),
        _ => None,
    } {
        let _ = log.record_turn(
            boundary,
            surface.id(),
            input.turn_id(),
            serde_json::json!({"cwd": input.cwd, "transcript": input.transcript_path}),
            &commonmeasure_harness::boundary_policy(policy.as_ref()),
        );
    }
    // Stop is the stable boundary for a context-budget snapshot: the host has
    // just finished a turn, so its transcript's last model call is what the
    // model was sent this turn. The reader is Claude Code's transcript shape,
    // so only that surface claims the basis; best-effort like everything else
    // here — an unreadable transcript is a missing snapshot, not an error.
    if input.hook_event_name.as_deref() == Some("Stop")
        && surface == HostSurface::ClaudeCode
        && let Some(transcript) = input.transcript_path.as_deref()
        && let Some(snapshot) = commonmeasure_harness::snapshot::from_claude_transcript(
            std::path::Path::new(transcript),
        )
    {
        let _ =
            log.record_context_snapshot(snapshot.to_payload(&session_id, surface.id(), transcript));
    }
    ExitCode::SUCCESS
}

fn serve_mcp(host: &str, session: Option<&str>) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    // Provider credentials from the operator home, applied here — at process
    // start, before any thread exists — so the mediated tools work whatever
    // directory the host launched the server in. The launching environment
    // always wins over the file, and a present file that cannot be used
    // fails the mediator loudly, exactly as an invalid policy.json does.
    let credentials = commonmeasure_supply::credentials::apply(&home)?;
    let session_id = session.map(str::to_owned).unwrap_or_else(uuid_like_session);
    // A session's policy is refreshed once, by whichever path opens it
    // first: a session-start hook where the host has one, else this server,
    // before the policy is resolved, so the session runs under the revision
    // the hub desires now. A session whose log already carries the hook's
    // refresh is not refreshed again; a host that registered the server
    // without the hook is, whatever it calls itself.
    let policy_sync = (!SessionLog::holds_event(&home, &session_id, "policy_sync"))
        .then(|| sync_managed_policy(&home, commonmeasure_harness::managed::SESSION_START_BUDGET))
        .flatten();
    // Stdio carries no cwd, but the server inherits the harness's own. It is
    // resolved once, here, so the scope that governs the session and the cwd
    // its crossings record are the same fact.
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let policy = SessionPolicy::load(&home, cwd.as_deref())?;
    let mut log = SessionLog::open(&home, &session_id).map_err(|error| error.to_string())?;
    // Every mediated crossing this server records names the policy that
    // ruled on it. The server resolves its policy once, here, and the log
    // stamps that one identity for the life of the process.
    log.set_policy_identity(&policy.identity());
    if let Some(sync) = policy_sync {
        log.record_policy_sync(host, "server_start", sync)
            .map_err(|error| {
                format!(
                    "could not record policy_sync to {}: {error}",
                    log.path().display()
                )
            })?;
    }
    // An enrolled edge names its key id in every session, whichever path
    // opened the log. Like credentials_loaded below, this precedes every
    // crossing, so a failed append is a failed session start.
    if let Some(enrolment) = commonmeasure_harness::EnrolmentRecord::load(&home)? {
        log.record_edge_identity(host, &enrolment)
            .map_err(|error| {
                format!(
                    "could not record edge_identity to {}: {error}",
                    log.path().display()
                )
            })?;
    }
    // Where each credential came from is evidence — path, digest and names,
    // never a value. Recorded only when a file was actually loaded: a session
    // with crossings and no credentials_loaded event ran on the launching
    // environment alone, so absence still answers the provenance question,
    // and a session that never crosses is not forced to open a log for this.
    if let Some(loaded) = &credentials.loaded {
        // This is the one evidence write that precedes every crossing, and the
        // inference the record promises — no `credentials_loaded` event means
        // the session ran on the launching environment alone — holds only if
        // a loaded file always leaves its event. A failed append is therefore
        // a failed session start, naming the log path, not a session that
        // quietly runs on credentials the record does not show.
        log.record_credentials(host, credentials.to_value())
            .map_err(|error| {
                format!(
                    "could not record credentials_loaded to {}: {error}",
                    log.path().display()
                )
            })?;
        // The host reads stdout as protocol, so every human-facing word goes
        // to stderr. A stray println here corrupts the JSON-RPC stream.
        eprintln!(
            "commonmeasure: credentials from {} ({} applied, {} shadowed by the environment)",
            credentials.path.display(),
            loaded.applied.len(),
            loaded.shadowed.len()
        );
    }
    eprintln!(
        "commonmeasure: mediating session {session_id}, recording to {}{}",
        log.path().display(),
        policy
            .scope()
            .map(|scope| format!(", under scope \"{scope}\""))
            .unwrap_or_default()
    );
    let mut server = McpServer::new(log, policy, host, cwd, credentials);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    server
        .serve(stdin.lock(), stdout.lock())
        .map_err(|error| error.to_string())
}

/// The credentials doctor: the same loader the MCP server runs at start, then
/// one line per provider saying configured-or-not and where the variable came
/// from. Everything here is a name, a path or a digest; no value is read back
/// out of the environment for printing.
fn credentials_report() -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let status = commonmeasure_supply::credentials::apply(&home)?;
    match &status.loaded {
        Some(loaded) => {
            println!(
                "credentials file: {} ({})",
                status.path.display(),
                loaded.sha256
            );
            if !loaded.shadowed.is_empty() {
                println!(
                    "  shadowed by the launching environment, which wins: {}",
                    loaded.shadowed.join(", ")
                );
            }
        }
        None => println!(
            "credentials file: {} is absent. Create it with KEY=VALUE lines and chmod 600, \
             or export variables in the environment that launches the harness.",
            status.path.display()
        ),
    }
    for provider in commonmeasure_supply::IMPLEMENTED_PROVIDERS {
        let Some(variable) = commonmeasure_supply::required_variable(provider) else {
            continue;
        };
        match commonmeasure_supply::supplier_from_environment(provider) {
            Ok(_) => {
                let from_file = status
                    .loaded
                    .as_ref()
                    .is_some_and(|loaded| loaded.applied.iter().any(|name| name == variable));
                let source = if from_file {
                    "the operator file"
                } else {
                    "the environment"
                };
                println!("  {provider:<12} configured ({variable} from {source})");
            }
            Err(commonmeasure_supply::SupplyError::CredentialMissing { variable }) => {
                println!("  {provider:<12} unavailable ({variable} is not set)");
            }
            Err(other) => println!("  {provider:<12} unavailable ({other})"),
        }
    }
    Ok(())
}

fn host_named(name: &str) -> Result<HostSurface, String> {
    HostSurface::parse(name).ok_or_else(|| {
        format!(
            "unknown host {name:?}; the hosts are claude (or claude-code), codex, pi, \
             claude-desktop and cursor"
        )
    })
}

/// Write one host's registration, naming the binary by absolute path.
fn install_host(host: &str, binary: Option<&Path>) -> Result<(), String> {
    use commonmeasure_harness::registration;
    let surface = host_named(host)?;
    let paths = registration::HostPaths::from_environment()?;
    let binary = registration::resolve_binary(binary)?;
    let lines = registration::install(surface, &binary, &paths)?;
    write_stdout(&format!("{}\n", lines.join("\n")))
}

fn uninstall_host(host: &str) -> Result<(), String> {
    use commonmeasure_harness::registration;
    let surface = host_named(host)?;
    let paths = registration::HostPaths::from_environment()?;
    let lines = registration::uninstall(surface, &paths)?;
    write_stdout(&format!("{}\n", lines.join("\n")))
}

/// Each host's registration checked against the machine. Exits zero
/// whatever it finds: the report is the result.
fn doctor(host: Option<&str>) -> Result<(), String> {
    use commonmeasure_harness::registration;
    let paths = registration::HostPaths::from_environment()?;
    let home = home_dir().map_err(|error| error.to_string())?;
    let surfaces = match host {
        Some(name) => vec![host_named(name)?],
        None => vec![
            HostSurface::ClaudeCode,
            HostSurface::Codex,
            HostSurface::Pi,
            HostSurface::ClaudeDesktop,
            HostSurface::Cursor,
        ],
    };
    let mut out = format!(
        "this binary: {} ({})\noperator home: {}\n",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "unresolved".to_owned()),
        env!("CARGO_PKG_VERSION"),
        home.display()
    );
    if let Some(line) = managed_policy_line(&home) {
        let _ = writeln!(out, "{line}");
    }
    for surface in surfaces {
        let report = registration::doctor(surface, &paths, &home);
        let _ = writeln!(
            out,
            "\n{:<12} {}",
            report.host,
            if report.registered {
                "registered"
            } else {
                "not registered"
            }
        );
        for line in report.lines {
            let _ = writeln!(out, "  {line}");
        }
    }
    write_stdout(&out)
}

/// What `doctor` says about managed policy: nothing on a local edge, the
/// revision in force and its expiry on a managed one, and since when it has
/// been stale where the hub has not renewed it.
fn managed_policy_line(home: &Path) -> Option<String> {
    let management = commonmeasure_harness::managed::management(home, chrono::Utc::now());
    match management.mode.as_str() {
        "local" => None,
        "managed" => Some(
            match (management.applied_revision, management.applied_expires_at) {
                (Some(revision), Some(expires_at)) => format!(
                    "managed policy: revision {revision} in force, expires {expires_at}{}",
                    stale_note(&serde_json::json!(management.stale_since))
                ),
                _ => format!(
                    "managed policy: no desired revision activated yet; last sync {}",
                    management.desired["outcome"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| "none".to_owned())
                ),
            },
        ),
        _ => Some(format!(
            "managed policy: unavailable ({})",
            inspect::text(&management.desired["unavailable"])
        )),
    }
}

/// A session identifier when the host supplies none. Derived from the clock and
/// the process, which is enough to keep two concurrent sessions apart without
/// pulling in a dependency the rest of this binary does not need.
fn uuid_like_session() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    format!("local-{now}-{}", std::process::id())
}

/// Reconstruct crossings from host transcripts.
///
/// Imported crossings are the weakest grade the store holds and are recorded as
/// `reconstructed`, naming the transcript each came from. A host that reached
/// the network by shelling out is reported as unimportable with its reason
/// rather than yielding an empty result, because "nothing we can use" and
/// "nothing happened" are different facts.
fn import(since: Option<&str>, dry_run: bool) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let cutoff = since
        .map(|date| {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| format!("--since expects YYYY-MM-DD, got {date}"))
        })
        .transpose()?;
    // One importer at a time per home. What the log is owed is decided by
    // reading it, and paid by appending to it; a second console running those
    // two steps in between records the same transcript facts twice, into a log
    // nothing can retract them from.
    let store =
        commonmeasure_harness::import::Store::open(&home).map_err(|error| error.to_string())?;

    let mut out = String::new();
    let mut total = 0usize;
    for host in commonmeasure_harness::known_hosts() {
        match host.importable {
            Err(reason) => {
                let _ = writeln!(out, "{:<12} not importable", host.host.id());
                let _ = writeln!(out, "             {reason}");
                continue;
            }
            Ok(()) if !host.root.exists() => {
                let _ = writeln!(
                    out,
                    "{:<12} no transcripts at {}",
                    host.host.id(),
                    host.root.display()
                );
                continue;
            }
            Ok(()) => {}
        }

        let transcripts = commonmeasure_harness::import::claude_transcripts(&host.root);
        let (mut imported, mut already, mut skipped, mut unreadable) =
            (0usize, 0usize, 0usize, 0usize);
        for path in &transcripts {
            let crossings = match commonmeasure_harness::import::from_claude_transcript(path) {
                Ok(crossings) => crossings,
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };
            let crossings: Vec<_> = crossings
                .into_iter()
                .filter(|crossing| {
                    cutoff.is_none_or(|cutoff| crossing.timestamp.date_naive() >= cutoff)
                })
                .collect();
            if crossings.is_empty() {
                continue;
            }
            // Importing is idempotent: a crossing the session log already holds
            // is reported as already recorded, never appended a second time. A
            // re-run therefore imports only what is new, and a dry run reports
            // what a real run would actually add.
            let session_id = crossings[0].session_id.clone();
            let (fresh, held) = store.not_yet_recorded(&session_id, crossings);
            already += held;
            if fresh.is_empty() {
                continue;
            }
            if dry_run {
                imported += fresh.len();
                continue;
            }
            match commonmeasure_harness::SessionLog::open(&home, &session_id) {
                Ok(mut log) => {
                    for crossing in &fresh {
                        match log.record_crossing(crossing) {
                            Ok(_) => imported += 1,
                            Err(_) => skipped += 1,
                        }
                    }
                }
                Err(_) => skipped += fresh.len(),
            }
        }
        total += imported;
        let _ = writeln!(
            out,
            "{:<12} {} new crossings from {} transcripts{}{}{}",
            host.host.id(),
            imported,
            transcripts.len(),
            if already > 0 {
                format!(", {already} already recorded")
            } else {
                String::new()
            },
            if unreadable > 0 {
                format!(", {unreadable} unreadable")
            } else {
                String::new()
            },
            if skipped > 0 {
                format!(", {skipped} could not be recorded")
            } else {
                String::new()
            },
        );
    }

    let _ = writeln!(
        out,
        "\n{} crossings {}. They are recorded as `reconstructed`: read back from a\n\
         transcript after the fact, not observed at the time.",
        total,
        if dry_run {
            "would be imported"
        } else {
            "imported"
        }
    );
    write_stdout(&out)
}

/// Deliver the projection and account for exactly what happened. The counts
/// printed are the relay's durable ones: a second run over the same evidence
/// logs reports "0 new at the receiver", because delivery is idempotent.
///
/// The engagements named are governing engagements, composed by the relay
/// itself from the clearance decisions it took. This command reads no
/// attribution rules and names no reported engagement, though it links the two
/// crates that could: egress is a capture-time enforcement act, and the name it
/// happened under is the one that authorised it. Printing the read-time name
/// beside it would put a projection that can be re-edited afterwards into the
/// account of what left, and would make this the second surface reconciling the
/// two identities. The console is the one that does
/// (`DECISIONS.md` §Session policy and egress).
fn relay(
    receiver: Option<String>,
    api_key: Option<String>,
    runs: Vec<PathBuf>,
    sessions: Vec<String>,
) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    // The clearances the relay reads come from the policy on disk, so a
    // managed edge refreshes it first. Nothing is printed when the desired
    // policy was already in force and its envelope has not expired.
    if let Some(sync) = sync_managed_policy(&home, commonmeasure_harness::managed::DEFAULT_BUDGET) {
        let outcome = sync["outcome"].as_str().unwrap_or_default();
        let fresh = outcome == "already_applied" && sync["stale_since"].is_null();
        if !fresh {
            eprintln!(
                "commonmeasure: policy sync {outcome}{}{}{}",
                sync["revision"]
                    .as_u64()
                    .map(|revision| format!(" (revision {revision})"))
                    .unwrap_or_default(),
                sync["reason"]
                    .as_str()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default(),
                stale_note(&sync["stale_since"])
            );
        }
    }
    let report = match commonmeasure_relay::relay(
        &home,
        &commonmeasure_relay::RelayOptions {
            receiver,
            api_key,
            runs,
            sessions,
        },
    ) {
        Ok(report) => report,
        Err(error) => {
            // The key's standing is the operator's first fact: a receiver
            // that answers 401 to a revoked key is the revocation, printed
            // on its own line before the failure that followed from it.
            if let Some(failure) = error.downcast_ref::<commonmeasure_relay::DeliveryFailure>() {
                if let Some(standing) = &failure.standing {
                    write_stdout(&format!("{standing}\n"))?;
                }
                return Err(failure.delivery_text());
            }
            return Err(format!("{error:#}"));
        }
    };
    write_stdout(&relay_report_text(&report))
}

/// The relay report as the operator reads it. Shared by `relay` and
/// `connect`, whose probe is a relay run.
fn relay_report_text(report: &commonmeasure_relay::RelayReport) -> String {
    let mut out = String::new();
    if let Some(standing) = &report.standing {
        out.push_str(&format!("{standing}\n"));
    }
    out.push_str(&format!(
        "projected {} of {} sessions and {} runs; {} events newly spooled\n",
        report.sessions_projected,
        report.sessions_read,
        report.runs_projected,
        report.events_enqueued
    ));
    if report.sessions_withheld > 0 {
        out.push_str(&format!(
            "  {} withheld: no crossing cleared to leave\n",
            report.sessions_withheld
        ));
    }
    if report.sessions_withheld_access_context > 0 {
        out.push_str(&format!(
            "  {} withheld: operator terms require access_context on the session, which the \
             event-batch delivery format cannot carry\n",
            report.sessions_withheld_access_context
        ));
    }
    // The sessions that were neither projected nor withheld by policy: they
    // held no witnessed crossing, or held only crossings the privacy floor
    // keeps home whatever the policy says.
    let quiet = report.sessions_read
        - report.sessions_projected
        - report.sessions_withheld
        - report.sessions_withheld_access_context;
    if quiet > 0 {
        out.push_str(&format!("  {quiet} with nothing eligible to project\n"));
    }
    if report.sessions_projected > 0 {
        out.push_str(&format!(
            "  {} refused crossings in the projected sessions, on the wire as a count per \
             session with no URL and no reason\n",
            report.refused_reported
        ));
    }
    out.push_str(&format!(
        "delivered {} events in {} batches to {} ({} new at the receiver)\n",
        report.events_delivered,
        report.batches_delivered,
        report.receiver,
        report.events_new_at_receiver
    ));
    for delivered in &report.delivered_by_clearance {
        out.push_str(&format!(
            "  {} under {}\n",
            delivered.events, delivered.clearance
        ));
    }
    out
}

/// Enrol with a hub. Both the hub and the token must be named: there is no
/// default hub and no ambient token, and the refusal says which is missing.
fn connect(hub: Option<&str>, token: Option<&str>, managed: bool) -> Result<(), String> {
    let (Some(hub), Some(token)) = (hub, token) else {
        let missing = match (hub, token) {
            (None, None) => "no hub URL and no --token",
            (None, Some(_)) => "no hub URL",
            (Some(_), None) => "no --token",
            (Some(_), Some(_)) => unreachable!("both present"),
        };
        return Err(format!(
            "{missing}: run `commonmeasure connect <hub-url> --token <token>` with the token an \
             owner minted in the hub; there is no default hub and nothing was sent"
        ));
    };
    let home = home_dir().map_err(|error| error.to_string())?;
    let report = commonmeasure_relay::connect(&home, hub, token, managed)
        .map_err(|error| format!("{error:#}"))?;
    let mut out = format!(
        "enrolled with {} in {} as {}\n  key id   {}\n  receiver {}\n  written  {}, {} and {}\n",
        report.hub,
        report.organization.name,
        report.name,
        report.key_id,
        report.receiver,
        home.join("edge-key.json").display(),
        home.join("enrolment.json").display(),
        home.join("relay.json").display(),
    );
    if let Some(replaced) = &report.replaced_receiver {
        out.push_str(&format!(
            "  relay.json previously named {replaced}; it now names the hub\n"
        ));
    }
    match &report.relay {
        Ok(relay) => {
            out.push_str("first relay run:\n");
            for line in relay_report_text(relay).lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        Err(error) => out.push_str(&format!(
            "first relay run failed: {error}\n  the enrolment stands; run `commonmeasure relay` \
             to retry\n"
        )),
    }
    // What --managed did, and whether the machine is where the operator
    // asked it to be. A scripted setup reads the exit code, so a pin that
    // was refused or a first synchronisation that activated nothing exits
    // non-zero after the account is printed; the enrolment stands either way.
    let mut managed_failure: Option<String> = None;
    match &report.managed {
        None => {}
        Some(Ok(pin)) => {
            out.push_str(&format!(
                "deployment  managed: signer {} pinned in {}; policy from {}\n",
                pin.key_id,
                pin.path.display(),
                pin.policy_url
            ));
            if let Some(previous) = &pin.replaced {
                out.push_str(&format!("  replaced {previous}\n"));
            }
            match commonmeasure_harness::managed::sync(
                &home,
                &edge_identity(&home),
                chrono::Utc::now(),
            ) {
                Ok(sync) => {
                    out.push_str("first policy sync:\n");
                    for line in sync_report_text(&sync).lines() {
                        out.push_str(&format!("  {line}\n"));
                    }
                    if !sync.converged() {
                        managed_failure = Some(format!(
                            "enrolled and pinned to signer {}, but the first policy \
                             synchronisation did not activate a policy ({}); the local policy \
                             stays in force until `commonmeasure policy sync` converges",
                            pin.key_id, sync.sync.outcome
                        ));
                    }
                }
                Err(error) => {
                    out.push_str(&format!("first policy sync refused: {error}\n"));
                    managed_failure = Some(format!(
                        "enrolled and pinned to signer {}, but the first policy synchronisation \
                         was refused: {error}",
                        pin.key_id
                    ));
                }
            }
        }
        Some(Err(reason)) => {
            out.push_str(&format!(
                "deployment  not pinned: {reason}\n  the enrolment stands and the edge is in \
                 local mode\n"
            ));
            managed_failure = Some(format!(
                "enrolled, but not managed: {reason}. Run `commonmeasure connect` again with \
                 --managed once the signer can be read"
            ));
        }
    }
    write_stdout(&out)?;
    match managed_failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}

/// Leave the hub. The local files go either way; whether the hub revoked
/// the credentials is stated, never assumed.
fn disconnect() -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let report = commonmeasure_relay::disconnect(&home).map_err(|error| format!("{error:#}"))?;
    let mut out = String::new();
    match &report.revoked_at_hub {
        Ok(()) => out.push_str(&format!(
            "revoked edge key {} and its ingest key at {}\n",
            report.key_id, report.hub
        )),
        Err(reason) => out.push_str(&format!(
            "edge key {} was not revoked at {}: {reason}\n  revoke it from the hub's API keys \
             page\n",
            report.key_id, report.hub
        )),
    }
    for path in &report.removed {
        out.push_str(&format!("removed {}\n", path.display()));
    }
    if let Some(policy_url) = &report.removed_deployment {
        out.push_str(&format!(
            "deployment.json pinned this hub's policy ({policy_url}); removed with the \
             enrolment, so the edge is in local mode\n"
        ));
    }
    if let Some(policy_url) = &report.kept_deployment {
        out.push_str(&format!(
            "deployment.json names another hub's policy ({policy_url}); kept\n"
        ));
    }
    write_stdout(&out)
}

/// Serve the operator console until stopped.
fn serve_console(listen: String, allow_remote: bool) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    // Load the operator's own credentials so the Sources screen can report
    // which providers are connected. The console only checks presence here; it
    // never fetches, and no key is passed into the sink.
    let _ = commonmeasure_supply::credentials::apply(&home);
    let providers: Vec<serde_json::Value> = commonmeasure_supply::IMPLEMENTED_PROVIDERS
        .iter()
        .copied()
        .filter(|name| {
            *name != "internal" && commonmeasure_supply::required_variable(name).is_some()
        })
        .map(|name| {
            serde_json::json!({
                "name": name,
                "connected": commonmeasure_supply::supplier_from_environment(name).is_ok(),
            })
        })
        .collect();
    // The Compare screen runs a live search across the connected providers.
    // The closure holds the provider adapters the sink deliberately does not,
    // so acquisition stays out of the console; it makes real, credit-spending
    // calls only when the operator submits a query.
    let connected: Vec<String> = providers
        .iter()
        .filter(|provider| provider.get("connected") == Some(&serde_json::Value::Bool(true)))
        .filter_map(|provider| {
            provider
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_owned)
        })
        .collect();
    let search: commonmeasure_console::SearchRunner = std::sync::Arc::new(move |query: &str| {
        connected
            .iter()
            .map(|name| run_provider_search(name, query))
            .collect()
    });
    commonmeasure_console::serve(commonmeasure_console::ServeOptions {
        listen,
        allow_remote,
        home,
        providers,
        search: Some(search),
    })
    .map_err(|error| format!("{error:#}"))
}

/// One provider's answer to a Compare query: its results, cost and latency, or
/// the error it failed with. A search limit of five keeps the probe cheap.
fn run_provider_search(name: &str, query: &str) -> serde_json::Value {
    let adapter = match commonmeasure_supply::supplier_from_environment(name) {
        Ok(adapter) => adapter,
        Err(error) => return serde_json::json!({"provider": name, "error": format!("{error}")}),
    };
    match adapter.search(query, 5, &[]) {
        Ok(acquisition) => serde_json::json!({
            "provider": name,
            "results_count": acquisition.envelopes.len(),
            "results": acquisition.envelopes.iter().take(5).map(|envelope| serde_json::json!({
                "host": envelope.host,
                "title": envelope.title,
                "url": envelope.source_url,
            })).collect::<Vec<_>>(),
            "cost": serde_json::to_value(&acquisition.charge).unwrap_or(serde_json::Value::Null),
            "latency_ms": acquisition.latency_ms,
        }),
        Err(error) => serde_json::json!({"provider": name, "error": format!("{error}")}),
    }
}

/// What became of one crossing. A refusal is its own outcome, because a
/// refused fetch retrieved nothing and is not counted with the crossings that
/// did.
fn crossing_outcome(payload: &serde_json::Value) -> &'static str {
    if payload["refusal"].is_string() {
        "refused"
    } else if payload["grounded"] == serde_json::Value::Bool(true) {
        "grounded"
    } else {
        "retrieved"
    }
}

fn show_session(session: Option<&str>) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let path = match session {
        Some(id) => home.join("sessions").join(format!("{id}.ndjson")),
        None => SessionLog::list(&home)
            .map_err(|error| error.to_string())?
            .into_iter()
            .next()
            .ok_or("no session has recorded anything yet")?,
    };
    let records =
        SessionLog::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;

    // The same count the console's session detail states, from the one
    // summariser both surfaces share: two accounts of one session that were
    // computed apart could disagree, and one of them did — see
    // `commonmeasure_harness::SessionSummary`.
    let summary = commonmeasure_harness::summarise(&records);

    let mut out = String::new();
    let _ = writeln!(out, "session    {}", path.display());
    let _ = writeln!(out, "records    {}", summary.records);
    let _ = writeln!(
        out,
        "crossings  {} observed, {} mediated, {} refused, {} reconstructed",
        summary.observed, summary.mediated, summary.refused, summary.reconstructed
    );
    let _ = writeln!(
        out,
        "grounded   {} put page text into the model's context{}{}",
        summary.grounded_witnessed,
        if summary.named_not_read() > 0 {
            format!(
                "; {} named a URL whose page was never read",
                summary.named_not_read()
            )
        } else {
            String::new()
        },
        if summary.grounded_reconstructed > 0 {
            format!(
                "; {} more are claimed only by transcripts, with nothing \
                 watching at the time",
                summary.grounded_reconstructed
            )
        } else {
            String::new()
        }
    );
    // The MCP client's own name and version, where a server in this session
    // recorded one at initialize, beside the host the registration named.
    for record in records
        .iter()
        .filter(|record| record["event"] == "client_identified")
    {
        let client = &record["payload"]["client"];
        let _ = writeln!(
            out,
            "client     {} {} via host {}",
            client["name"].as_str().unwrap_or("unnamed"),
            client["version"].as_str().unwrap_or(""),
            record["payload"]["host"].as_str().unwrap_or("unknown")
        );
    }
    // Context snapshots: an observation with a named basis, shown with it.
    // The crossings line beneath joins what this runtime witnessed to the
    // host's totals without claiming the one explains the other.
    let snapshots: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| record["event"] == "context_snapshot")
        .map(|record| &record["payload"])
        .collect();
    if !snapshots.is_empty() {
        let _ = writeln!(
            out,
            "\ncontext    {} snapshot(s) at Stop boundaries; basis: {}",
            snapshots.len(),
            snapshots[0]["basis"].as_str().unwrap_or("unstated")
        );
        for payload in &snapshots {
            let _ = writeln!(
                out,
                "  {}  {} tokens sent to {} ({} cache-read, {} cache-written, {} uncached); {} out",
                payload["observed_at"].as_str().unwrap_or("unknown time"),
                payload["context_tokens"],
                payload["model"].as_str().unwrap_or("an unnamed model"),
                payload["cache_read_input_tokens"],
                payload["cache_creation_input_tokens"],
                payload["input_tokens"],
                payload["output_tokens"],
            );
        }
        let _ = writeln!(
            out,
            "  unavailable at this boundary: {}",
            snapshots
                .last()
                .and_then(|payload| payload["unavailable"].as_array())
                .map(|names| {
                    names
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(|name| name.split(" (").next().unwrap_or(name))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "unstated".to_owned())
        );
        let witnessed_tokens: u64 = records
            .iter()
            .filter(|record| {
                matches!(
                    record["event"].as_str(),
                    Some("crossing_observed") | Some("crossing_mediated")
                )
            })
            .filter_map(|record| record["payload"]["estimated_tokens"].as_u64())
            .sum();
        let _ = writeln!(
            out,
            "  acquired content witnessed by crossings: ~{witnessed_tokens} tokens \
             (characters/4) — a lower bound on acquired input, not an \
             explanation of the host totals"
        );
        write_inventory(&mut out, &records);
    }

    // Which policy the session's mediated crossings and boundaries met, as
    // the records name it. Records from before the field existed are
    // counted and said to predate it, never read as "no policy".
    let mut identities: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    let mut unavailable: u64 = 0;
    let mut predating: u64 = 0;
    for record in &records {
        let payload = &record["payload"];
        if !matches!(
            record["event"].as_str(),
            Some("crossing_mediated")
                | Some("crossing_refused")
                | Some("nudge_issued")
                | Some("turn_started")
                | Some("turn_completed")
        ) {
            continue;
        }
        if let Some(identity) = payload["policy_identity"].as_str() {
            *identities.entry(identity).or_default() += 1;
        } else if payload["policy_unavailable"].is_string() {
            unavailable += 1;
        } else {
            predating += 1;
        }
    }
    if !identities.is_empty() || unavailable > 0 || predating > 0 {
        let _ = writeln!(out, "\npolicy identity (mediated crossings and boundaries)");
        for (identity, count) in &identities {
            let _ = writeln!(out, "  {identity}  {count} record(s)");
        }
        if unavailable > 0 {
            let _ = writeln!(
                out,
                "  {unavailable} record(s) at boundaries where the policy could not be read"
            );
        }
        if predating > 0 {
            let _ = writeln!(
                out,
                "  {predating} record(s) predate the field and name no policy"
            );
        }
    }

    let _ = writeln!(out, "\ncrossings");
    for record in &records {
        let payload = &record["payload"];
        let Some(url) = payload["url"].as_str() else {
            continue;
        };
        // The three ways a crossing can end with no content are named
        // separately, because they are three parties' decisions: this
        // operator's policy, the origin's own refusal, and the transport
        // (`docs/contracts/session-evidence.md`). The identity the request
        // presented is on the line above them, because whether a publisher
        // could verify who fetched is what the rest is read against.
        let _ = writeln!(
            out,
            "  {:<10} {:<9} {}{}{}{}{}",
            inspect::text(&payload["mode"]),
            crossing_outcome(payload),
            url,
            presented_identity(payload)
                .map(|identity| format!("\n      identity: {identity}"))
                .unwrap_or_default(),
            payload["refusal"]
                .as_str()
                .map(|reason| format!("\n      refused: {reason}"))
                .unwrap_or_default(),
            payload["challenge"]
                .as_str()
                .map(|reason| format!("\n      the origin refused the request: {reason}"))
                .unwrap_or_default(),
            payload["failure"]
                .as_str()
                .map(|reason| format!("\n      no answer: {reason}"))
                .unwrap_or_default()
        );
    }
    write_stdout(&out)
}

/// What one crossing's request presented, in a phrase: the enrolled key that
/// signed it, or that it carried no signature and why. `None` where no
/// request left the machine, which is every crossing but a mediated one.
fn presented_identity(payload: &serde_json::Value) -> Option<String> {
    let identity = payload.get("identity")?;
    match identity["key_id"].as_str() {
        Some(key_id) => Some(format!("signed as {key_id}")),
        None => Some(format!(
            "unsigned — {}",
            inspect::text(&identity["unsigned"])
        )),
    }
}

/// This edge's fleet-status document, built from the same loader, resolver
/// and ledger enforcement reads, for the directory and principal this
/// command runs as.
fn show_status(json: bool) -> Result<(), String> {
    use commonmeasure_harness::fleet;
    let home = home_dir().map_err(|error| error.to_string())?;
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let now = chrono::Utc::now();
    let document = fleet::status(
        &home,
        cwd.as_deref(),
        &edge_identity(&home),
        &commonmeasure_harness::managed::management(&home, now),
        now,
    );
    if json {
        return write_stdout(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?
        ));
    }
    let applied = &document["applied"];
    let mut out = String::new();
    let _ = writeln!(
        out,
        "contract          {}",
        inspect::text(&document["contract"])
    );
    let _ = writeln!(
        out,
        "edge              {}",
        document["edge"]["key_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!(
                "unknown ({})",
                inspect::text(&document["edge"]["unknown"])
            ))
    );
    let _ = writeln!(
        out,
        "deployment mode   {}",
        inspect::text(&document["deployment_mode"])
    );
    let desired = &document["desired"];
    let _ = writeln!(
        out,
        "desired revision  {}",
        if document["deployment_mode"] == "local" {
            "none (local mode accepts no remote policy)".to_owned()
        } else if let Some(reason) = desired["unavailable"].as_str() {
            format!("unavailable: {reason}")
        } else if desired.is_null() {
            "none learned yet".to_owned()
        } else {
            format!(
                "{}; last sync {}{}",
                desired["revision"]
                    .as_u64()
                    .map(|revision| revision.to_string())
                    .unwrap_or_else(|| "none learned".to_owned()),
                inspect::text(&desired["outcome"]),
                desired["reason"]
                    .as_str()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default()
            )
        }
    );
    if let Some(revision) = applied["revision"].as_u64() {
        let _ = writeln!(
            out,
            "applied revision  {revision}, expires {}{}",
            inspect::text(&applied["expires_at"]),
            stale_note(&applied["stale_since"])
        );
    }
    match applied["unavailable"].as_str() {
        Some(reason) => {
            let _ = writeln!(out, "policy            unavailable: {reason}");
        }
        None => {
            let _ = writeln!(
                out,
                "policy digest     {}",
                applied["policy_digest"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(
                        || "none (no policy file; observing, refusing nothing)".to_owned()
                    )
            );
            let _ = writeln!(
                out,
                "policy identity   {}  ({}, resolver {})",
                inspect::text(&applied["policy_identity"]["digest"]),
                inspect::text(&applied["policy_identity"]["schema"]),
                inspect::text(&applied["policy_identity"]["resolver"]),
            );
            let _ = writeln!(
                out,
                "principal         {} (basis {})",
                inspect::text(&applied["principal"]["name"]),
                inspect::text(&applied["principal"]["basis"]),
            );
        }
    }
    let _ = writeln!(
        out,
        "last enforcement  {}",
        document["last_enforcement"]["at"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!(
                "unknown ({})",
                inspect::text(&document["last_enforcement"]["unknown"])
            ))
    );
    match document["allowances"].as_array() {
        Some(periods) if periods.is_empty() => {
            let _ = writeln!(out, "allowances        none declared");
        }
        Some(periods) => {
            let _ = writeln!(out, "allowances");
            for period in periods {
                match period["unavailable"].as_str() {
                    Some(reason) => {
                        let _ = writeln!(
                            out,
                            "  {}  unavailable: {reason}",
                            inspect::text(&period["principal"])
                        );
                    }
                    None => {
                        let _ = writeln!(
                            out,
                            "  {}  {} {}  remaining {} {}{}",
                            inspect::text(&period["principal"]),
                            inspect::text(&period["period"]),
                            inspect::text(&period["period_key"]),
                            inspect::text(&period["remaining"]["currency"]),
                            period["remaining"]["micros"]
                                .as_u64()
                                .map(|micros| format!(
                                    "{}.{:06}",
                                    micros / 1_000_000,
                                    micros % 1_000_000
                                ))
                                .unwrap_or_else(|| "unknown".to_owned()),
                            if period["exceeded"] == serde_json::Value::Bool(true) {
                                " (exceeded)"
                            } else {
                                ""
                            }
                        );
                    }
                }
            }
        }
        None => {
            let _ = writeln!(
                out,
                "allowances        unavailable: {}",
                inspect::text(&document["allowances"]["unavailable"])
            );
        }
    }
    let _ = writeln!(
        out,
        "\nThe identity and digest are drift evidence, not proof that this edge \
         enforced the policy; `commonmeasure status --json` prints the document."
    );
    write_stdout(&out)
}

/// The key id minted at enrolment is this edge's identity. An edge with no
/// enrolment record is not enrolled, and every surface that reports the
/// identity says so; an edge with one reports the key id it holds.
fn edge_identity(home: &Path) -> commonmeasure_harness::fleet::EdgeIdentity {
    use commonmeasure_harness::fleet::EdgeIdentity;
    match commonmeasure_harness::enrolled_key_id(home) {
        Ok(Some(key_id)) => EdgeIdentity::KeyId(key_id),
        Ok(None) => EdgeIdentity::Unknown {
            reason: format!(
                "not enrolled: no enrolment record at {}",
                commonmeasure_harness::EnrolmentRecord::path(home).display()
            ),
        },
        Err(reason) => EdgeIdentity::Unknown { reason },
    }
}

/// One synchronisation of managed policy on the paths that run it without
/// being asked: session start and the relay. `None` on a local edge, where
/// there is no management request to make. On a managed edge the result is
/// the record of what happened, including a deployment or state file that
/// could not be read; the policy on disk keeps governing whatever it says
/// (`docs/contracts/policy-envelope.md` §Cadence and staleness).
fn sync_managed_policy(home: &Path, budget: std::time::Duration) -> Option<serde_json::Value> {
    use commonmeasure_harness::managed::{SyncReport, is_managed, sync_within};
    match is_managed(home) {
        Ok(false) => None,
        Ok(true) => Some(
            match sync_within(home, &edge_identity(home), chrono::Utc::now(), budget) {
                Ok(report) => report.to_record(),
                Err(reason) => SyncReport::unavailable(&reason),
            },
        ),
        Err(reason) => Some(SyncReport::unavailable(&reason)),
    }
}

/// The suffix a policy line carries once the applied envelope has expired:
/// the policy stays in force and the line says since when it has been
/// stale. Empty while the envelope is current.
fn stale_note(stale_since: &serde_json::Value) -> String {
    stale_since
        .as_str()
        .map(|since| {
            format!(" (stale since {since}: the hub has not renewed it; the policy stays in force)")
        })
        .unwrap_or_default()
}

/// The canonical pre-image and its digest, for the reviewer recomputing an
/// identity by hand.
fn show_policy_identity() -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let policy = SessionPolicy::load(&home, cwd.as_deref())?;
    let identity = policy.identity();
    write_stdout(&format!(
        "{}\n{}\n",
        commonmeasure_types::canonical::canonical_json(&policy.canonical()),
        identity.digest
    ))
}

/// One synchronisation with the pinned signer's desired policy.
fn sync_policy() -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let report =
        commonmeasure_harness::managed::sync(&home, &edge_identity(&home), chrono::Utc::now())?;
    write_stdout(&sync_report_text(&report))?;
    if report.converged() {
        Ok(())
    } else {
        Err(format!(
            "the desired policy was not activated ({}); the policy already in force stays",
            report.sync.outcome
        ))
    }
}

/// One synchronisation's report as the operator reads it. Shared by
/// `policy sync` and `connect --managed`, whose first synchronisation is
/// the same request.
fn sync_report_text(report: &commonmeasure_harness::managed::SyncReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "policy url    {}", report.policy_url);
    let _ = writeln!(
        out,
        "outcome       {}{}",
        report.sync.outcome,
        report
            .sync
            .revision
            .map(|revision| format!(" (revision {revision})"))
            .unwrap_or_default()
    );
    if let Some(reason) = &report.sync.reason {
        let _ = writeln!(out, "reason        {reason}");
    }
    match &report.applied {
        Some(applied) => {
            let _ = writeln!(
                out,
                "in force      revision {} ({}), activated {}, expires {}{}",
                applied.revision,
                applied.digest,
                applied.activated_at,
                applied.expires_at,
                stale_note(&serde_json::json!(report.stale_since))
            );
        }
        None => {
            let _ = writeln!(
                out,
                "in force      the local policy file; no desired revision has been activated"
            );
        }
    }
    out
}

/// The category inventory of the last snapshot, then what changed between
/// consecutive snapshots: the API-reported delta beside the estimated growth
/// of host scaffolding, conversation and acquired content, the witnessed
/// crossings recorded between the two boundaries, and the remainder the
/// estimates do not account for. The three attributions are estimates on the
/// inventory's own basis; the delta is the provider's count; nothing here
/// claims the estimates explain the delta, which is why the remainder is
/// printed rather than absorbed. A boundary with no counters, or whose
/// inventory carries different categories from its neighbour's, is printed
/// as not comparable rather than compared against zero.
fn write_inventory(out: &mut String, records: &[serde_json::Value]) {
    use commonmeasure_harness::snapshot::{
        CATEGORY_ACQUIRED, CATEGORY_CONVERSATION, ContextInventory,
    };
    struct Boundary {
        position: usize,
        context_tokens: Option<u64>,
        inventory: Option<ContextInventory>,
        observed_at: String,
    }
    let boundaries: Vec<Boundary> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record["event"] == "context_snapshot")
        .map(|(position, record)| {
            let payload = &record["payload"];
            Boundary {
                position,
                context_tokens: payload["context_tokens"].as_u64(),
                inventory: ContextInventory::from_payload(payload),
                observed_at: payload["observed_at"]
                    .as_str()
                    .unwrap_or("unknown time")
                    .to_owned(),
            }
        })
        .collect();
    let Some(Boundary {
        inventory: Some(last),
        ..
    }) = boundaries.last()
    else {
        let _ = writeln!(
            out,
            "  no inventory: the last snapshot carries none this report can read"
        );
        return;
    };
    let _ = writeln!(
        out,
        "\ninventory  at the last boundary, estimated on the host's own records (characters/4):"
    );
    for (name, footprint) in &last.categories {
        let _ = writeln!(
            out,
            "  {name:<24} ~{:>7} tokens over {} record(s)",
            footprint.estimated_tokens(),
            footprint.records
        );
    }
    let _ = writeln!(
        out,
        "  available on demand: {} tool(s), {} skill(s), {} agent type(s); definitions loaded \
         on demand: {}; tools invoked: {} call(s) over {} tool(s); compactions: {}",
        last.tools_on_demand.len(),
        last.skills_on_demand.len(),
        last.agents_on_demand.len(),
        if last.definitions_loaded.is_empty() {
            "none".to_owned()
        } else {
            last.definitions_loaded
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        },
        last.invoked.values().sum::<u64>(),
        last.invoked.len(),
        last.compactions
    );
    if boundaries.len() < 2 {
        return;
    }
    let _ = writeln!(out, "\nbetween boundaries");
    let signed = |after: u64, before: u64| after as i64 - before as i64;
    for pair in boundaries.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        let witnessed: u64 = records[from.position..to.position]
            .iter()
            .filter(|record| {
                matches!(
                    record["event"].as_str(),
                    Some("crossing_observed") | Some("crossing_mediated")
                )
            })
            .filter_map(|record| record["payload"]["estimated_tokens"].as_u64())
            .sum();
        let heading = format!("  {} → {}:", from.observed_at, to.observed_at);
        let (Some(from_tokens), Some(to_tokens)) = (from.context_tokens, to.context_tokens) else {
            let _ = writeln!(
                out,
                "{heading} not comparable, a boundary carries no counters; witnessed crossings \
                 between them ~{witnessed}"
            );
            continue;
        };
        let delta = signed(to_tokens, from_tokens);
        let (Some(before), Some(after)) = (&from.inventory, &to.inventory) else {
            let _ = writeln!(
                out,
                "{heading} {delta:+} tokens API-reported; witnessed crossings between them \
                 ~{witnessed}; a boundary carries no inventory, so nothing is attributed"
            );
            continue;
        };
        let carried = after.scaffolding_carried();
        let comparable = carried == before.scaffolding_carried();
        let group =
            |name: &str| -> Option<i64> { Some(signed(after.tokens(name)?, before.tokens(name)?)) };
        let scaffolding = match (
            comparable,
            after.scaffolding_tokens(),
            before.scaffolding_tokens(),
        ) {
            (true, Some(after), Some(before)) => Some(signed(after, before)),
            _ => None,
        };
        let conversation = group(CATEGORY_CONVERSATION);
        let acquired = group(CATEGORY_ACQUIRED);
        let compacted = if after.compactions > before.compactions {
            format!(
                " ({} compaction(s) rebuilt the window in this interval)",
                after.compactions - before.compactions
            )
        } else {
            String::new()
        };
        match (scaffolding, conversation, acquired) {
            (Some(scaffolding), Some(conversation), Some(acquired)) => {
                let remainder = delta - scaffolding - conversation - acquired;
                let _ = writeln!(
                    out,
                    "{heading} {delta:+} tokens API-reported\n    estimated growth: host \
                     scaffolding {scaffolding:+} ({}), conversation {conversation:+}, acquired \
                     content {acquired:+}; witnessed crossings between the boundaries \
                     ~{witnessed}; remainder {remainder:+} not attributed{compacted}",
                    carried.join(", ")
                );
            }
            _ => {
                let _ = writeln!(
                    out,
                    "{heading} {delta:+} tokens API-reported; witnessed crossings between them \
                     ~{witnessed}; not attributed, because the two inventories carry different \
                     categories{compacted}"
                );
            }
        }
    }
}

/// The public education pages, one file per source.
fn write_guide(out: &Path) -> Result<(), String> {
    let pages = commonmeasure_console::guide::public_pages()?;
    std::fs::create_dir_all(out).map_err(|error| format!("create {}: {error}", out.display()))?;
    for page in pages {
        let path = out.join(format!("{}.html", page.name));
        std::fs::write(&path, page.html.as_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        println!("wrote {} ({} bytes)", path.display(), page.html.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::crossing_outcome;

    /// A refused crossing fetched nothing. The end-to-end account of the
    /// session report lives in `tests/hook_e2e.rs`; what is asserted here is
    /// the outcome each payload earns.
    #[test]
    fn a_refused_crossing_is_not_labelled_retrieved() {
        assert_eq!(
            crossing_outcome(&json!({
                "url": "https://example.com/x",
                "grounded": false,
                "refusal": "the session policy denies example.com",
            })),
            "refused"
        );
        assert_eq!(
            crossing_outcome(&json!({"url": "https://example.com/x", "grounded": true})),
            "grounded"
        );
        assert_eq!(
            crossing_outcome(&json!({"url": "https://example.com/x", "grounded": false})),
            "retrieved"
        );
    }
}
