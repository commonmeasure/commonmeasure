//! The Common Measure command line (binary `commonmeasure`).
//!
//! Deliberately thin. Everything that decides, calls or records lives in
//! `commonmeasure-runtime`, so there is no behaviour here that a test of the runtime
//! would miss, and no path by which the CLI can produce a result the runtime
//! would not.

mod artifact;
mod benchmark;
mod enrol;
mod hosted;
mod hosted_tokens;
mod inspect;
mod instance;
mod mcp_session;

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use commonmeasure_harness::{HostSurface, SessionLog, home_dir, policy::SessionPolicy};
use commonmeasure_runtime::{RunOptions, load_suite};

/// The hosts whose hook payloads the observed path reads, as `--host` names
/// them: Claude Code's shape, Cursor's and the Copilot CLI's mapped onto it,
/// and the browser extension's answer message for each browser surface.
const HOSTS: [&str; 8] = [
    "claude-code",
    "codex",
    "pi",
    "cursor",
    "copilot-cli",
    "chatgpt-web",
    "google-ai-overview",
    "bing-copilot-search",
];

/// The hosts the mediated server accepts under `--host`, one per
/// registration `install` writes. A value not listed is refused by the
/// argument parser with this list, never recorded as `claude-code`.
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
    /// Load one policy file through the loader every session uses and report
    /// what it accepted, or the refusal and nothing else. Reads the named
    /// file and changes nothing: the way to settle whether a policy is
    /// valid before it is installed or published
    /// (docs/contracts/source-policy.md).
    Check {
        /// The policy document to load.
        file: PathBuf,
    },
    /// Print the JSON Schema of the policy file, derived from the types the
    /// loader parses with. The structural half of the policy's contract;
    /// the checks the loader makes after parsing are stated in
    /// docs/contracts/source-policy.md.
    Schema,
}

#[derive(Subcommand)]
enum Command {
    /// Associate sessions with saved artifacts and verify portable snapshots offline.
    Artifact(artifact::Artifact),
    /// Run a small, reproducible SimpleQA benchmark through governed search,
    /// answer generation and correctness grading. Local operator access only.
    Benchmark(benchmark::Benchmark),
    /// Set up a project directory, or show its current enrolment.
    Enrol(enrol::Enrol),
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
    /// Print a run dossier or inspect a session source record.
    Inspect {
        /// A run directory previously written by `run`, or a session NDJSON file.
        run: PathBuf,
    },
    /// Record one harness lifecycle event. Reads the host's hook payload on
    /// stdin and always exits zero: capture must never break the agent. On
    /// `session-start` it also emits the standing mediation nudge on stdout,
    /// which the host adds to the session's context.
    Hook {
        /// The event, as the host names it (`post-tool-use`).
        event: String,
        /// Which host is calling: `claude-code`, `codex`, `pi`, `cursor`,
        /// `copilot-cli`, or a browser surface (`chatgpt-web`,
        /// `google-ai-overview`, `bing-copilot-search`), whose payload is the
        /// extension's answer message. The payload is read in that host's
        /// shape, and a payload of another host's shape records nothing.
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
        /// Session identifier. The host supplies one per conversation; without
        /// it, `AGENT_SESSION_ID` from the environment is used, and a fresh
        /// one is generated when neither is set.
        #[arg(long)]
        session: Option<String>,
    },
    /// The hosted edge: the same mediated tools served over Streamable HTTP
    /// to hosts that reach MCP servers from their vendor's cloud, one
    /// endpoint per host word, each person authenticated by a token the hub
    /// issued or a token this edge issued.
    Hosted {
        #[command(subcommand)]
        command: HostedCommand,
    },
    /// Speak Chrome's native messaging protocol on stdin and stdout for the
    /// browser extension: each message is one answer's sources from ChatGPT,
    /// Google AI Overviews or Bing Copilot Search, recorded as observed
    /// crossings, and each is answered with what was recorded. Chrome starts
    /// the binary itself with the extension's origin as the only argument,
    /// which is read as this command; `install chrome` registers it.
    NativeHost {
        /// The calling extension's origin, as Chrome passes it.
        origin: Option<String>,
    },
    /// Report where provider credentials come from and which providers are
    /// configured. Names variables, paths and digests; never prints a value.
    Credentials,
    /// Register this binary with a host, by absolute path, in the host's own
    /// configuration: Claude Code gets the five hooks and the MCP server at
    /// user scope; Codex gets the MCP server table, with the approval mode
    /// that lets its non-interactive runs call the tools; Pi gets an
    /// extension that runs the server, because Pi has no MCP client; Claude
    /// Desktop gets the MCP server in its configuration file; Cursor gets
    /// the MCP server and four hooks in its global files; the Copilot CLI
    /// gets the MCP server in its user file and four hooks in a hook file
    /// of this product's own; VS Code gets the MCP server in the user
    /// mcp.json and no hooks; Chrome gets the native messaging host manifest
    /// the browser extension reaches the binary through, and Chromium and
    /// Brave the same where they are set up. Only this product's entries are
    /// written; everything else in the host's files is kept as read.
    Install {
        /// The host: `claude`, `codex`, `pi`, `claude-desktop`, `cursor`,
        /// `copilot`, `vscode` or `chrome`.
        host: String,
        /// Register this path instead of the running binary. It must exist;
        /// the registration names it as resolved.
        #[arg(long)]
        binary: Option<PathBuf>,
    },
    /// Remove exactly the entries `install` wrote for a host. The evidence in
    /// the operator home is never touched.
    Uninstall {
        /// The host: `claude`, `codex`, `pi`, `claude-desktop`, `cursor`,
        /// `copilot`, `vscode` or `chrome`.
        host: String,
    },
    /// Report each host's registration against the machine: what is
    /// registered and where, which binary it names and the version that
    /// binary reports, whether a session log can be written, and whether the
    /// policy file loads.
    Doctor {
        /// One host (`claude`, `codex`, `pi`, `claude-desktop`, `cursor`,
        /// `copilot`, `vscode` or `chrome`); every host when omitted.
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
    /// synchronisation with the pinned signer's desired policy; and any
    /// candidate policy checked against the loader, or the schema of the
    /// file printed (docs/contracts/source-policy.md).
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Deliver the Content Telemetry projection of the evidence logs to a
    /// configured receiver. It sends witnessed retrieval and grounding facts
    /// only; nothing else leaves the machine (docs/contracts/session-evidence.md).
    /// With no receiver configured this command refuses and sends nothing:
    /// there is no default egress. Due batches get up to ten attempts, starting
    /// at one minute and doubling to an hour. Dead batches stay undelivered
    /// until `relay requeue` explicitly starts another schedule.
    ///
    /// The report names the governing engagements whose policy.json clearance
    /// let each delivered event leave, the sessions withheld because nothing in
    /// them was cleared, and any events carrying no engagement at all. The
    /// reported engagement the console counts work under is a different
    /// identity and is not read here; the console names both and where they
    /// differ.
    Relay {
        #[command(subcommand)]
        action: Option<RelayAction>,
        /// Forecast counts and distinct outgoing hosts without sending or writing.
        #[arg(long)]
        dry_run: bool,
        /// Forecast a draft policy using the ordinary policy loader.
        #[arg(long, requires = "dry_run")]
        policy: Option<PathBuf>,
        /// Receiver base URL, e.g. `http://localhost:8080`. Overrides the
        /// `receiver` in `relay.json` for this invocation. The key in
        /// `relay.json` is sent only to the origin of the receiver in
        /// `relay.json`: another receiver gets `--api-key` or no key. Events
        /// another receiver accepts are recorded delivered and are not sent
        /// to the configured receiver afterwards, so a hub's reporting duty
        /// reads nothing for them.
        #[arg(long)]
        receiver: Option<String>,
        /// API key the receiver of this invocation expects, sent as
        /// `X-API-Key`. Overrides the key in `relay.json` for the configured
        /// receiver; with `--receiver`, it is the only key sent.
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
    /// Register, renew, close and read a working instance at the hub this
    /// edge is enrolled with (docs/contracts/session-evidence.md §Instance
    /// registration). Each command records what it did and the accepted
    /// binding's revision, validity window and next check in
    /// ~/.commonmeasure/instances/<instance>.json, readable by the owner
    /// only; a refusal is recorded with the hub's reason.
    Instance {
        #[command(subcommand)]
        command: instance::InstanceCommand,
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
    /// file only; makes no network call. An absent or invalid label fails.
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
        /// Local PEM trust anchors for claim and CAWG identity verification.
        /// No trust list is downloaded; the report records the bundle's hash.
        #[arg(long)]
        trust_anchors: Option<PathBuf>,
        /// Fail unless the credential is valid and trusted under the supplied
        /// anchors. This does not establish factual accuracy or licence rights.
        #[arg(long, requires = "trust_anchors")]
        require_trusted: bool,
    },
    /// Write the guide's public education pages into a directory, one
    /// self-contained HTML file per source, with every product panel
    /// stripped and no product name in the output; the console serves the
    /// product variant at /guide.
    Guide {
        /// Directory to write into; created if absent.
        out: PathBuf,
    },
    /// Serve the operator console on loopback: Overview, Record, Policy,
    /// Sources and Compare, rendered from the local evidence logs.
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

#[derive(Subcommand)]
enum HostedCommand {
    /// Serve the mediated tools over HTTP from the command line. Requires an
    /// enrolled home: the hub named in ~/.commonmeasure/enrolment.json is
    /// the issuer whose tokens are accepted, and its organisation the one a
    /// token must name. The policy governs private addresses as it does on
    /// stdio. Prints `listening on <url>` once bound.
    Serve {
        /// Address to listen on. The public address is --origin; this is
        /// where the process itself accepts connections, ordinarily behind
        /// a load balancer holding the certificate.
        #[arg(long, default_value = "127.0.0.1:8765")]
        listen: String,
        /// The origin hosts reach this edge on (`https://<label>.edge.example`,
        /// scheme and authority only). Each endpoint is `<origin>/mcp/<host>`
        /// and is the exact audience a token must name.
        #[arg(long)]
        origin: String,
        /// A further Origin header value to accept beside --origin itself. A
        /// request carrying any other Origin is refused with 403. Repeatable.
        #[arg(long = "allow-origin")]
        allow_origin: Vec<String>,
        /// A host word to serve; every word when none is given. Repeatable.
        #[arg(long = "host")]
        host: Vec<String>,
    },
    /// Run as the deployed service: origin, allowed origins, host words and
    /// interval from ~/.commonmeasure/hosted-service.json. Refuses to start
    /// unenrolled, under local policy, or while another process holds the
    /// home. Runs the relay, the policy refresh, the issuer key refresh and
    /// the idle-session sweep on the interval, and refuses every loopback,
    /// private, link-local and .internal address whatever the policy says,
    /// so the machine's metadata service is never reached. Prints
    /// `listening on <url>` once bound.
    Service {
        /// Address to listen on, overriding the configuration's `listen`.
        #[arg(long)]
        listen: Option<String>,
    },
    /// Bearer tokens this edge issues for a host with no OAuth. Stored as
    /// hashes in ~/.commonmeasure/hosted-tokens.json; a policy binds the
    /// label with `edge_token`.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
}

#[derive(Subcommand)]
enum TokenCommand {
    /// Mint a token for one label and print it once. Only its hash is kept.
    Issue {
        /// The principal the token stands for, for example `ci:owner/repo`.
        label: String,
        /// Accept the token on this endpoint's host word alone, for example
        /// `copilot-cloud-agent`. Without it the token is accepted on every
        /// endpoint.
        #[arg(long)]
        host: Option<String>,
    },
    /// Revoke a label's token. Its next request is refused by name.
    Revoke { label: String },
    /// The labels issued, when, and whether each stands.
    List,
}

#[derive(clap::Subcommand)]
enum RelayAction {
    /// Start a new schedule for dead batches; sends nothing. Omit --batch for all.
    Requeue {
        /// Spool index of one dead batch.
        #[arg(long)]
        batch: Option<u64>,
    },
}

fn main() -> ExitCode {
    // Chrome starts a native messaging host with the extension's origin as
    // the first argument and no subcommand of ours, so that argument alone
    // selects the native host.
    let mut arguments: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if arguments
        .get(1)
        .and_then(|first| first.to_str())
        .is_some_and(|first| first.starts_with("chrome-extension://"))
    {
        arguments.insert(1, "native-host".into());
    }
    let cli = Cli::parse_from(arguments);
    let result = match cli.command {
        Command::Benchmark(args) => benchmark::run(args),
        Command::Enrol(args) => enrol::run(args),
        Command::Run {
            suite,
            output,
            live,
            replay,
        } => run(&suite, output, live, replay.as_deref()),
        Command::Inspect { run } => inspect::inspect(&run),
        Command::Hook { event, host } => return hook(&event, &host),
        Command::Mcp { host, session } => serve_mcp(&host, session.as_deref()),
        Command::Hosted { command } => run_hosted(command),
        Command::NativeHost { origin: _ } => return native_host(),
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
            PolicyAction::Check { file } => check_policy(&file),
            PolicyAction::Schema => show_policy_schema(),
        },
        Command::Relay {
            action,
            dry_run,
            policy,
            receiver,
            api_key,
            run,
            session,
        } => match action {
            Some(RelayAction::Requeue { batch }) => (|| {
                if dry_run
                    || policy.is_some()
                    || receiver.is_some()
                    || api_key.is_some()
                    || !run.is_empty()
                    || !session.is_empty()
                {
                    return Err("relay requeue does not accept delivery or forecast options".into());
                }
                let home = home_dir().map_err(|e| e.to_string())?;
                let count = commonmeasure_relay::spool::Spool::open(&home)
                    .and_then(|spool| spool.requeue(batch, chrono::Utc::now()))
                    .map_err(|e| format!("{e:#}"))?;
                write_stdout(&format!(
                    "requeued {count} dead batches; still undelivered until the receiver accepts them\n"
                ))
            })(),
            None => relay(receiver, api_key, run, session, dry_run, policy),
        },
        Command::Connect {
            hub,
            token,
            managed,
        } => connect(hub.as_deref(), token.as_deref(), managed),
        Command::Instance { command } => instance::run(command),
        Command::Disconnect => disconnect(),
        Command::Artifact(args) => artifact::run(args),
        Command::Guide { out } => write_guide(&out),
        Command::Provenance {
            file,
            run,
            trust_anchors,
            require_trusted,
        } => provenance_report(
            &file,
            run.as_deref(),
            trust_anchors.as_deref(),
            require_trusted,
        ),
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
fn provenance_report(
    file: &Path,
    run: Option<&Path>,
    trust_anchors: Option<&Path>,
    require_trusted: bool,
) -> Result<(), String> {
    use commonmeasure_runtime::processor::provenance;
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    let trust_pem = trust_anchors
        .map(|path| {
            std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read trust anchors {}: {error}", path.display()))
        })
        .transpose()?;
    let read = provenance::read_back_with_trust(&text, trust_pem.as_deref())
        .map_err(|error| format!("{}: {error}", file.display()))?;
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
    if read.validation["state"] == "invalid" {
        return Err("the provenance credential is invalid; see the validation report".to_owned());
    }
    if require_trusted && read.validation["trusted"] != true {
        return Err(
            "the provenance credential is not trusted under the supplied anchors".to_owned(),
        );
    }
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
/// when capture is unavailable (`docs/FAIL-POLICY.md` §8). It must never
/// claim to have recorded something it did not: a failure to append leaves this log
/// instance owing a gap, which a later append through the same instance
/// materialises. A hook process usually
/// makes one append and exits, so a failed one commonly leaves no gap record at
/// all — session-log completeness is per-process
/// (`docs/contracts/session-evidence.md` §Where).
fn hook(event: &str, host: &str) -> ExitCode {
    let mut raw = String::new();
    let surface = HostSurface::parse(host).unwrap_or(HostSurface::ClaudeCode);
    // A browser surface's payload is the extension's answer message, which
    // has no lifecycle: it is recorded at `post-tool-use`, the moment after
    // the crossing, and every other event records and prints nothing.
    if surface.is_browser() {
        if event == "post-tool-use"
            && std::io::stdin().read_to_string(&mut raw).is_ok()
            && let Ok(answer) =
                serde_json::from_str::<commonmeasure_harness::browser::BrowserAnswer>(&raw)
            && let Ok(home) = home_dir()
        {
            let _ = commonmeasure_harness::browser::record(
                &home,
                &answer,
                Some(surface),
                uuid_like_session,
            );
        }
        return ExitCode::SUCCESS;
    }
    // The payload in the named host's shape; another host's shape, or no
    // payload, records nothing. Cursor, VS Code and the Copilot CLI load
    // Claude Code's hook files and run their commands with their own
    // payloads, which is why the reader is told which host it is reading.
    let value = match std::io::stdin().read_to_string(&mut raw) {
        Ok(_) => serde_json::from_str::<serde_json::Value>(&raw).ok(),
        Err(_) => None,
    };
    // Cursor, Gemini CLI, the Devin CLI and Grok Build run Claude Code's hook
    // commands and set a variable of their own for every hook; a command told
    // it is reading Claude Code and running under one of them reads nothing,
    // whatever the payload's shape, because some of them send Claude Code's
    // shape and the record would name a host that did not run.
    let under_another_host = surface
        .foreign_environment(|variable| std::env::var_os(variable).is_some())
        .is_some();
    // Another host's payload, or another host's environment, is refused:
    // nothing is read and, at session start, nothing is printed. A Claude
    // Code payload that is merely unreadable keeps the nudge, because
    // delivery is the point and an unreadable payload costs the record and
    // not the delivery.
    let refused = under_another_host
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
        // Cursor and the Copilot CLI read a session-start hook's stdout as
        // JSON, each under its own field name.
        if surface == HostSurface::Cursor {
            let _ = std::io::stdout().write_all(
                serde_json::json!({"additional_context": commonmeasure_harness::nudge::TEXT})
                    .to_string()
                    .as_bytes(),
            );
        } else if surface == HostSurface::CopilotCli {
            let _ = std::io::stdout().write_all(
                serde_json::json!({"additionalContext": commonmeasure_harness::nudge::TEXT})
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
                // An enrolled edge whose record says its directory proof is
                // due renews it beside the policy refresh, inside the same
                // wait, so the identity record below says whether the key
                // is listed.
                let started = std::time::Instant::now();
                let directory_proof = start_directory_proof_refresh(&home);
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
                finish_directory_proof_refresh(directory_proof, started);
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
                    let listing = enrolment.listing_at(&home, chrono::Utc::now());
                    let _ = log.record_edge_identity(surface.id(), &enrolment, &listing);
                }
                // The host process this hook runs under, which is the only
                // join between this log and the one the MCP server the same
                // host started writes under its own identifier.
                let _ = log.record_host_process(
                    surface.id(),
                    "hook",
                    &commonmeasure_harness::host_process::find(),
                );
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
            "session-end" => "SessionEnd".to_owned(),
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
    // The host says the session ended, with its own word for why. Nothing
    // else is read at that moment, and the record's absence is not evidence
    // that a session continues, because a crash sends no hook.
    if input.hook_event_name.as_deref() == Some("SessionEnd") {
        let _ = log.record_session_ended(surface.id(), input.reason.as_deref());
        start_session_end_relay(&home);
        return ExitCode::SUCCESS;
    }
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

/// Start a relay run in the background when a session ends, so a local edge
/// delivers without anyone typing `commonmeasure relay`.
///
/// The run is the whole relay, every session and every due spooled batch,
/// rather than the ended session alone: the MCP server the host started
/// writes its crossings under its own session identifier, so the hook's
/// session holds only part of the work. No receiver in `relay.json` starts
/// nothing, as the relay itself would refuse, and so does the `relay/manual`
/// marker, the operator's switch for reviewing each run before it leaves
/// (`commonmeasure_harness::delivery`). The child takes no stdio from
/// the hook and runs in its own process group, so the host neither waits for
/// it nor ends it with the hook. The hook exits zero whatever happens here:
/// a relay already running holds the spool lock and the child exits at once,
/// leaving the work in the session logs and the spool for the next run,
/// whose outcome `status` and `doctor` report from `relay/receipts.json` and
/// the spool.
fn start_session_end_relay(home: &Path) {
    if !matches!(
        commonmeasure_relay::config::RelayConfig::load(home),
        Ok(Some(_))
    ) || !commonmeasure_harness::delivery::automatic(home)
    {
        return;
    }
    let Ok(binary) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(binary);
    command
        .arg("relay")
        .env("COMMONMEASURE_HOME", home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    // Not waited for: the child outlives the hook by design.
    #[cfg(not(windows))]
    let _ = command.spawn();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        // A host that runs its hooks inside a job object with
        // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` would kill the relay when the
        // hook's job closes. Breakaway is refused where the job does not
        // permit it, and the spawn itself then fails, so the flag is an
        // attempt rather than a requirement: on failure the child starts
        // without it and shares the job's fate.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
        if command.spawn().is_err() {
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            let _ = command.spawn();
        }
    }
}

/// The largest message accepted from the browser. Chrome allows 64 MiB
/// towards a host; one answer's sources are a few kilobytes.
const NATIVE_MESSAGE_MAX: u32 = 8 * 1024 * 1024;

/// Serve the browser extension over Chrome's native messaging framing: each
/// message is a 32-bit length in native byte order followed by that many
/// bytes of JSON, in both directions. Each message is answered, and nothing
/// but framed replies reaches stdout, which Chrome reads as protocol.
///
/// Exits zero whatever the messages held, as a hook does: a message that
/// cannot be recorded is answered with the reason and the browser carries on.
fn native_host() -> ExitCode {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let mut length = [0u8; 4];
        if stdin.read_exact(&mut length).is_err() {
            return ExitCode::SUCCESS;
        }
        let length = u32::from_ne_bytes(length);
        if length > NATIVE_MESSAGE_MAX {
            // The stream cannot be trusted past a length this wrong, so this
            // answer is the last.
            let _ = write_native_message(
                &mut stdout,
                &serde_json::json!({"recorded": 0, "error": format!(
                    "a message of {length} bytes exceeds the {NATIVE_MESSAGE_MAX} this host accepts"
                )}),
            );
            return ExitCode::SUCCESS;
        }
        let mut body = vec![0u8; length as usize];
        if stdin.read_exact(&mut body).is_err()
            || write_native_message(&mut stdout, &native_reply(&body)).is_err()
        {
            return ExitCode::SUCCESS;
        }
    }
}

/// The answer to one native message: the recording's outcome, or the
/// binary's status when the popup asks for it.
fn native_reply(body: &[u8]) -> serde_json::Value {
    use commonmeasure_harness::browser;
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error) => {
            return serde_json::json!({"recorded": 0, "error": format!("the message is not JSON: {error}")});
        }
    };
    let home = match home_dir() {
        Ok(home) => home,
        Err(error) => return serde_json::json!({"recorded": 0, "error": error.to_string()}),
    };
    // The popup asks whether the binary answers and whether it can record.
    if value["status"] == true {
        let recording = commonmeasure_harness::registration::recording_line(&home);
        return serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "recording": recording});
    }
    let answer = match serde_json::from_value::<browser::BrowserAnswer>(value) {
        Ok(answer) => answer,
        Err(error) => {
            return serde_json::json!({"recorded": 0, "error": format!("the message is not an answer: {error}")});
        }
    };
    match browser::record(&home, &answer, None, uuid_like_session) {
        Ok(recorded) => {
            serde_json::json!({"recorded": recorded.crossings, "session": recorded.session_id})
        }
        Err(error) => serde_json::json!({"recorded": 0, "error": error}),
    }
}

fn write_native_message(
    out: &mut impl std::io::Write,
    reply: &serde_json::Value,
) -> std::io::Result<()> {
    let body = reply.to_string();
    let length = u32::try_from(body.len()).unwrap_or(u32::MAX);
    out.write_all(&length.to_ne_bytes())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

fn serve_mcp(host: &str, session: Option<&str>) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    // Provider credentials from the operator home, applied here — at process
    // start, before any thread exists — so the mediated tools work whatever
    // directory the host launched the server in. The launching environment
    // always wins over the file, and a present file that cannot be used
    // fails the mediator loudly, exactly as an invalid policy.json does.
    let credentials = commonmeasure_supply::credentials::apply(&home)?;
    let session_id = mcp_session_id(session)?;
    if let Some(loaded) = &credentials.loaded {
        // The host reads stdout as protocol, so every human-facing word goes
        // to stderr. A stray println here corrupts the JSON-RPC stream.
        eprintln!(
            "commonmeasure: credentials from {} ({} applied, {} shadowed by the environment)",
            credentials.path.display(),
            loaded.applied.len(),
            loaded.shadowed.len()
        );
    }
    // Stdio carries no cwd, but the server inherits the harness's own. It is
    // resolved once, here, so the scope that governs the session and the cwd
    // its crossings record are the same fact.
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let mut server = mcp_session::open(
        &home,
        host,
        &session_id,
        cwd,
        None,
        credentials,
        mcp_session::Transport::STDIO,
    )?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    server
        .serve(stdin.lock(), stdout.lock())
        .map_err(|error| error.to_string())
}

fn run_hosted(command: HostedCommand) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    match command {
        HostedCommand::Serve {
            listen,
            origin,
            allow_origin,
            host,
        } => hosted::serve(
            &home,
            hosted::Options {
                listen,
                origin,
                allowed_origins: allow_origin,
                hosts: if host.is_empty() {
                    hosted::HOST_WORDS
                        .iter()
                        .map(|word| (*word).to_owned())
                        .collect()
                } else {
                    host
                },
                service: None,
            },
        ),
        HostedCommand::Service { listen } => hosted::service(&home, listen),
        HostedCommand::Token { command } => match command {
            TokenCommand::Issue { label, host } => {
                let secret = hosted_tokens::issue(&home, &label, host.as_deref())?;
                // The one time the secret exists in the clear: stdout, so an
                // operator can pipe it straight into the host's secret store.
                eprintln!(
                    "commonmeasure: issued edge token for {label:?}; only its hash is kept in {}",
                    hosted_tokens::path(&home).display()
                );
                write_stdout(&format!("{secret}\n"))
            }
            TokenCommand::Revoke { label } => {
                hosted_tokens::revoke(&home, &label)?;
                write_stdout(&format!("revoked edge token {label:?}\n"))
            }
            TokenCommand::List => {
                let mut lines = String::new();
                for token in hosted_tokens::list(&home)? {
                    let standing = match &token.revoked_at {
                        Some(at) => format!("revoked {at}"),
                        None => "standing".to_owned(),
                    };
                    let bound = match &token.host {
                        Some(host) => format!("/mcp/{host}"),
                        None => "every endpoint".to_owned(),
                    };
                    let _ = writeln!(
                        lines,
                        "{}\t{}\t{standing}\t{bound}",
                        token.label, token.issued_at
                    );
                }
                write_stdout(&lines)
            }
        },
    }
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
             claude-desktop, cursor, copilot (or copilot-cli), vscode and chrome"
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
            HostSurface::CopilotCli,
            HostSurface::VsCode,
            HostSurface::Chrome,
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
    let _ = writeln!(out, "{}", hosted::service_line(&home));
    out.push_str(&commonmeasure_relay::state::egress_text(
        &commonmeasure_relay::egress_report(&home),
    ));
    let _ = writeln!(
        out,
        "{}",
        commonmeasure_relay::state::last_delivery_text(&home, chrono::Utc::now())
    );
    let _ = writeln!(
        out,
        "{}",
        automatic_relay_line(
            &home,
            commonmeasure_harness::registration::session_end_registered(&paths)
        )
    );
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

/// What `doctor` says about relaying without a person. Three things stop it,
/// and each stops it on its own: no receiver in `relay.json`, the
/// `relay/manual` marker the operator writes to review each run, and no
/// carrier. Two carriers relay by themselves: the `SessionEnd` hook of a
/// Claude Code registration (`session_end`), and a running hosted service's
/// interval relay, which relays every session in the home whatever its host.
/// The service counts only while it holds the home's lock, so a configured
/// but stopped service reads as off. The line names the first stop that
/// applies, or every carrier in force, and where the marker applies it names
/// what else the marker does, because a licence demanding usage reporting is
/// refused while automatic delivery is off (owner decision, 22 September
/// 2026).
fn automatic_relay_line(home: &Path, session_end: bool) -> String {
    match commonmeasure_relay::config::RelayConfig::load(home) {
        Err(error) => return format!("automatic relay: off, {error}"),
        Ok(None) => return "automatic relay: off, no receiver is configured".to_owned(),
        Ok(Some(_)) => {}
    }
    if let Some(reason) = commonmeasure_harness::delivery::withheld_reason(home) {
        return format!(
            "automatic relay: off, {reason}; a source whose licence demands usage reporting is \
             refused while the marker is there"
        );
    }
    let service = hosted::ServiceConfig::read(home).ok().flatten();
    let running = service
        .as_ref()
        .filter(|_| hosted::HomeLock::held(home))
        .map(|config| {
            format!(
                "on the hosted service's interval (every {}s), which relays every session in \
                 this home whatever its host",
                config.interval_seconds
            )
        });
    match (session_end, running) {
        (true, Some(service)) => format!(
            "automatic relay: at each Claude Code session end (its SessionEnd hook), and {service}"
        ),
        (false, Some(service)) => format!("automatic relay: {service}"),
        (true, None) => "automatic relay: at each Claude Code session end (its SessionEnd hook); \
                         no other local host sends the event, so with them run `commonmeasure \
                         relay`, and a source whose licence demands usage reporting is refused \
                         there"
            .to_owned(),
        (false, None) => format!(
            "automatic relay: off, no Claude Code registration sends SessionEnd{}; run \
             `commonmeasure relay`, or `commonmeasure install claude`",
            if service.is_some() {
                " and the hosted service is configured but not running"
            } else {
                ""
            }
        ),
    }
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

/// The variable Goose sets, in the environment of every stdio server it
/// starts, to its own session identifier.
const AGENT_SESSION_ID: &str = "AGENT_SESSION_ID";

/// The mediated server's session identifier, from the first of: the host's
/// `--session`, `AGENT_SESSION_ID` in the environment, and one minted here.
/// Taking Goose's identifier puts the records of every server one Goose
/// session starts in one log. Goose also sends the identifier as
/// `_meta.agent-session-id` on each request; that is not read, because the
/// log is opened once per process before any request arrives, and a value
/// that could differ between requests cannot name it. An identifier that is
/// not a plain name fails the start, naming where it came from, rather than
/// being replaced by a minted one that would split the host's session.
fn mcp_session_id(session: Option<&str>) -> Result<String, String> {
    let (session_id, source) = match session {
        Some(session) => (session.to_owned(), "--session"),
        None => match std::env::var(AGENT_SESSION_ID) {
            Ok(session) if !session.is_empty() => (session, AGENT_SESSION_ID),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(format!(
                    "{AGENT_SESSION_ID} is not valid Unicode, so it cannot name a session; \
                     pass --session"
                ));
            }
            _ => (uuid_like_session(), "the server"),
        },
    };
    if commonmeasure_harness::safe_session(&session_id).is_none() {
        return Err(format!(
            "session {session_id:?} from {source} is not a plain identifier: use ASCII \
             letters, digits, '.', '_' and '-', at most 128 characters, not starting with '.'"
        ));
    }
    Ok(session_id)
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
/// two identities. The console is the one that does.
fn relay(
    receiver: Option<String>,
    api_key: Option<String>,
    runs: Vec<PathBuf>,
    sessions: Vec<String>,
    dry_run: bool,
    policy: Option<PathBuf>,
) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    // The clearances the relay reads come from the policy on disk, so a
    // managed edge refreshes it first. Nothing is printed when the desired
    // policy was already in force and its envelope has not expired.
    if !dry_run
        && let Some(sync) =
            sync_managed_policy(&home, commonmeasure_harness::managed::DEFAULT_BUDGET)
    {
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
    // Kept for the basis line under a forecast, which names the draft it was
    // taken against.
    let draft = policy.clone();
    let report = match commonmeasure_relay::relay(
        &home,
        &commonmeasure_relay::RelayOptions {
            dry_run,
            policy,
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
                if let Some(directory_proof) = &failure.directory_proof {
                    write_stdout(&format!("{directory_proof}\n"))?;
                }
                write_stdout(&commonmeasure_relay::state::egress_text(
                    &commonmeasure_relay::egress_report(&home),
                ))?;
                return Err(failure.delivery_text());
            }
            // The run completed for every session that read, so the operator
            // gets its report as usual and the command still fails naming
            // the sessions it skipped.
            if let Some(skipped) = error.downcast_ref::<commonmeasure_relay::UnreadableSessions>() {
                write_stdout(&relay_report_text(&skipped.report))?;
                if dry_run {
                    write_stdout(&dry_run_basis_text(&home, draft.as_deref()))?;
                }
                return Err(skipped.to_string());
            }
            return Err(format!("{error:#}"));
        }
    };
    write_stdout(&relay_report_text(&report))?;
    if dry_run {
        write_stdout(&dry_run_basis_text(&home, draft.as_deref()))?;
    }
    Ok(())
}

/// What a forecast was taken against, printed under it.
///
/// A dry run makes no network call and writes nothing, so it syncs neither
/// the managed policy nor the directory grants: it reads the policy and the
/// grants already on disk. A real run refreshes both before projecting, and
/// the clearances and internal prefixes it then reads can differ from these.
/// Without this the forecast reads as the run, which it is not on a managed
/// edge or one whose grants have moved.
fn dry_run_basis_text(home: &Path, draft: Option<&Path>) -> String {
    let mut out = String::new();
    if let Some(draft) = draft {
        let _ = writeln!(out, "forecast policy: the draft {}", draft.display());
    } else {
        let management = commonmeasure_harness::managed::management(home, chrono::Utc::now());
        let _ = writeln!(
            out,
            "forecast policy: {}",
            match (management.mode.as_str(), management.applied_revision) {
                ("managed", Some(revision)) => format!(
                    "managed revision {revision}, as applied on disk{}",
                    stale_note(&serde_json::json!(management.stale_since))
                ),
                ("managed", None) => "managed, no desired revision applied yet".to_owned(),
                _ => format!(
                    "{}, as it stands on disk",
                    home.join("policy.json").display()
                ),
            }
        );
    }
    out.push_str(
        "a real run syncs managed policy and directory grants first, which can change what is \
         cleared and what leaves\n",
    );
    out
}

/// The relay report as the operator reads it. Shared by `relay` and
/// `connect`, whose probe is a relay run.
fn relay_report_text(report: &commonmeasure_relay::RelayReport) -> String {
    let mut out = String::new();
    if report.dry_run {
        out.push_str("dry run: nothing was sent; no state was changed\n");
    }
    if let Some(standing) = &report.standing {
        out.push_str(&format!("{standing}\n"));
    }
    if let Some(directory_proof) = &report.directory_proof {
        out.push_str(&format!("{directory_proof}\n"));
    }
    if report.dry_run {
        out.push_str(&format!(
            "would deliver {} events in {} batches to {} (new at the receiver: unknown)\n",
            report.events_delivered, report.batches_delivered, report.receiver
        ));
    } else {
        out.push_str(&format!(
            "delivered {} events in {} batches to {} ({} new at the receiver)\n",
            report.events_delivered,
            report.batches_delivered,
            report.receiver,
            report
                .events_new_at_receiver
                .map(|count| count.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ));
    }
    // What leaves, by event type, before anything about projection: the
    // spool count below excludes batches earlier runs queued, and turn
    // boundaries usually outnumber content events many times (EGR-25).
    for (kind, events) in &report.events_by_type {
        out.push_str(&format!("  {events} {kind}\n"));
    }
    for delivered in &report.delivered_by_clearance {
        out.push_str(&format!(
            "  {} under {}\n",
            delivered.events, delivered.clearance
        ));
    }
    if report.instance_references_withheld > 0 {
        out.push_str(&if report.dry_run {
            format!(
                "warning: {} of those events are queued with an instance reference that {} did \
                 not issue. Delivery would withhold the reference and record the events \
                 delivered; the issuing hub would not be sent them afterwards, so its reporting \
                 duties would read nothing for them\n",
                report.instance_references_withheld, report.receiver
            )
        } else {
            format!(
                "{}\n",
                commonmeasure_relay::withheld_references_warning(
                    report.instance_references_withheld,
                    &report.receiver
                )
            )
        });
    }
    out.push_str(&format!(
        "projected {} of {} sessions and {} runs; {} events {}\n",
        report.sessions_projected,
        report.sessions_read,
        report.runs_projected,
        report.events_enqueued,
        if report.dry_run {
            "would be newly spooled"
        } else {
            "newly spooled"
        }
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
    if report.batches_queued > 0 || report.batches_dead > 0 {
        out.push_str(&format!(
            "{} queued batches and {} dead batches remain undelivered\n",
            report.batches_queued, report.batches_dead
        ));
    }
    if report.dry_run {
        if report.hosts.is_empty() {
            out.push_str("hosts that would leave: none\n");
        } else {
            out.push_str("hosts that would leave:\n");
            for host in &report.hosts {
                out.push_str(&format!("  {host}\n"));
            }
        }
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
    out.push_str(&format!("{}\n", report.directory_proof));
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
    // was refused or a first synchronisation that failed exits non-zero.
    // An organisation awaiting its first revision is enrolled successfully;
    // its local policy stays in force and it has not converged.
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
                chrono::Utc::now,
            ) {
                Ok(sync) => {
                    out.push_str("first policy sync:\n");
                    for line in sync_report_text(&sync).lines() {
                        out.push_str(&format!("  {line}\n"));
                    }
                    if sync.awaiting_first_revision() {
                        out.push_str("  managed enrolment complete; waiting for the organisation's first policy revision.\n  Publish a policy in the hub, then run `commonmeasure policy sync`; sessions also refresh automatically.\n");
                    } else if !sync.converged() {
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
    match &report.retired_instance_sessions {
        Ok(sessions) if sessions.is_empty() => {}
        Ok(sessions) => out.push_str(&format!(
            "no longer attributed to a registered instance: session {}\n  the instances were \
             registered under the key given up; their records stay under instances/ and the \
             hub ends them with the key or at their expiry\n",
            sessions.join(", ")
        )),
        Err(reason) => out.push_str(&format!(
            "the session pointers under instances/by-session were not removed: {reason}\n  \
             remove that directory, or those sessions are stopped before every mediated fetch and \
             search\n"
        )),
    }
    write_stdout(&out)
}

/// Serve the operator console until stopped.
fn serve_console(listen: String, allow_remote: bool) -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let home = if home.is_absolute() {
        home
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(home)
    };
    let credentials = commonmeasure_supply::credentials::apply(&home)?;
    let providers: Vec<serde_json::Value> = commonmeasure_supply::IMPLEMENTED_PROVIDERS
        .iter().map(|name| {
            let adapter = commonmeasure_supply::supplier_from_environment(name).ok();
            let capable = adapter.as_ref().is_some_and(|a| a.capabilities().iter().any(|c|
                matches!(c, commonmeasure_types::ProviderCapability::Search | commonmeasure_types::ProviderCapability::Query)));
            serde_json::json!({"name": name, "connected": adapter.is_some(), "comparable": capable,
                "availability": if capable { "Configured" } else if adapter.is_some() { "No search or query capability" } else { "Configuration unavailable" }})
        }).collect();
    let comparison_home = home.clone();
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .display()
        .to_string();
    let search: commonmeasure_console::SearchRunner = std::sync::Arc::new(move |request| {
        commonmeasure_harness::compare::run(
            &comparison_home,
            Some(cwd.clone()),
            credentials.clone(),
            request,
        )
    });
    commonmeasure_console::serve(commonmeasure_console::ServeOptions {
        listen,
        allow_remote,
        home,
        providers,
        search: Some(search),
        // The harness's process-table probe: present, absent, or `None` when
        // the table cannot be read or the recorded start time does not parse.
        liveness: Some(std::sync::Arc::new(|pid: u32, started_at: &str| {
            let started_at = chrono::DateTime::parse_from_rfc3339(started_at)
                .ok()?
                .with_timezone(&chrono::Utc);
            commonmeasure_harness::host_process::is_present(pid, started_at)
        })),
    })
    .map_err(|error| format!("{error:#}"))
}

/// What became of one crossing. A refusal is its own outcome, because a
/// refused fetch retrieved nothing and is not counted with the crossings that
/// did.
fn crossing_outcome(payload: &serde_json::Value) -> &'static str {
    if payload["refusal"].is_string() {
        "refused"
    } else if payload["grounded"] == serde_json::Value::Bool(true)
        && payload["context_observation"] != "host_required"
    {
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
    let mut records =
        SessionLog::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    // A hook and the MCP server of one host session write two logs on the
    // hosts that pass the server no session identifier. The logs whose
    // `host_process` records name the same process are read together, so
    // the counts below and the crossings compared with a context snapshot
    // include the mediated crossings the other log holds. Nothing else
    // joins two logs: not the working directory, not timing.
    let join = commonmeasure_harness::host_process::joined_logs(&home, &path)
        .map_err(|error| error.to_string())?;
    for other in &join.others {
        records.extend(
            SessionLog::read(other).map_err(|error| format!("{}: {error}", other.display()))?,
        );
    }
    if !join.others.is_empty() {
        // Timestamp order between the logs, position order within one: the
        // timestamps are UTC to the millisecond in one format, so they
        // compare as text, and the sort is stable.
        records.sort_by(|a, b| a["timestamp"].as_str().cmp(&b["timestamp"].as_str()));
    }

    // The same count the console's session detail states, from the one
    // summariser both surfaces share: two accounts of one session that were
    // computed apart could disagree, and one of them did — see
    // `commonmeasure_harness::SessionSummary`.
    let summary = commonmeasure_harness::summarise(&records);

    let mut out = String::new();
    let _ = writeln!(out, "session    {}", path.display());
    match &join.host_process {
        Some(process) => {
            let named = format!(
                "host process {} (pid {}, started {})",
                process.command,
                process.pid,
                process
                    .started_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            );
            for other in &join.others {
                let _ = writeln!(out, "joined     {} on {named}", other.display());
            }
            if join.others.is_empty() {
                let _ = writeln!(out, "joined     nothing: no other log names {named}");
            }
        }
        None => {
            let _ = writeln!(
                out,
                "joined     nothing: this log names no host process, so the join is unavailable"
            );
        }
    }
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
    let _ = writeln!(out, "{}", summary.host_observed.display());
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
            if let Some(handle) = payload["acquisition_id"].as_str() {
                let observed =
                    commonmeasure_harness::session::host_observations(&records, Some(handle));
                if observed.context_entries > 0 {
                    "host-observed"
                } else {
                    "retrieved"
                }
            } else {
                crossing_outcome(payload)
            },
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

fn local_host_observations(
    home: &std::path::Path,
) -> Result<commonmeasure_harness::session::HostObservationSummary, String> {
    let mut summary = commonmeasure_harness::session::HostObservationSummary::default();
    for path in SessionLog::list(home).map_err(|error| error.to_string())? {
        let bytes = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let records: Vec<serde_json::Value> = bytes
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        summary.merge(commonmeasure_harness::session::host_observations(
            &records, None,
        ));
    }
    Ok(summary)
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
    let mut document = fleet::status(
        &home,
        cwd.as_deref(),
        &edge_identity(&home),
        &commonmeasure_harness::managed::management(&home, now),
        now,
    );
    document["egress"] = commonmeasure_relay::egress_report(&home);
    // Private local status extension, separate from the fleet policy contract.
    let host_observed = local_host_observations(&home);
    document["host_observed"] = match &host_observed {
        Ok(summary) => summary.to_value(),
        Err(error) => serde_json::json!({"unavailable": error}),
    };
    if json {
        return write_stdout(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?
        ));
    }
    let applied = &document["applied"];
    let mut out = commonmeasure_relay::state::egress_text(&document["egress"]);
    match host_observed {
        Ok(summary) => {
            let _ = writeln!(out, "{}", summary.display());
        }
        Err(error) => {
            let _ = writeln!(out, "host observations unavailable: {error}");
        }
    }
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
    let _ = writeln!(
        out,
        "service mode      {}",
        hosted::service_line(&home).trim_start_matches("hosted service: ")
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

/// Renew the enrolled key's directory proof on its own thread when the
/// enrolment record says one is due, so a session or server start pays for
/// it inside the wait it already makes for the policy refresh rather than
/// after it. `None` when nothing is due and no request is made.
fn start_directory_proof_refresh(
    home: &Path,
) -> Option<std::sync::mpsc::Receiver<commonmeasure_relay::ProofRefresh>> {
    let now = chrono::Utc::now();
    let due = commonmeasure_harness::EnrolmentRecord::load(home)
        .ok()
        .flatten()
        .is_some_and(|record| record.directory_proof_due_at(home, now));
    if !due {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let home = home.to_owned();
    std::thread::spawn(move || {
        let budget = commonmeasure_harness::managed::SESSION_START_BUDGET;
        if let Some(refresh) = commonmeasure_relay::refresh_directory_proof_if_due(&home, budget) {
            let _ = tx.send(refresh);
        }
    });
    Some(rx)
}

/// Wait for a renewal started at `started` until the session-start budget
/// is spent. A renewal still running then is abandoned: its record write is
/// atomic, and the next start or relay run tries again.
fn finish_directory_proof_refresh(
    refresh: Option<std::sync::mpsc::Receiver<commonmeasure_relay::ProofRefresh>>,
    started: std::time::Instant,
) {
    if let Some(refresh) = refresh {
        let budget = commonmeasure_harness::managed::SESSION_START_BUDGET;
        let _ = refresh.recv_timeout(budget.saturating_sub(started.elapsed()));
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
            match sync_within(home, &edge_identity(home), chrono::Utc::now, budget) {
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

/// Load one candidate policy through the loader and report what it accepted.
///
/// The refusal is the loader's own sentence, so an author gets the text the
/// edge would have produced rather than a second opinion about it. The digest
/// printed is the digest of the policy in the loader's form, which is what a
/// fleet-status document reports as applied
/// (`docs/contracts/canonical-json.md` §Shared policy vectors).
fn check_policy(file: &Path) -> Result<(), String> {
    let encoded = std::fs::read(file)
        .map_err(|error| format!("cannot read the policy at {}: {error}", file.display()))?;
    let policy = commonmeasure_harness::policy::PolicyDocument::check(&encoded, file)?;
    let mut out = String::new();
    let _ = writeln!(out, "accepted      {}", file.display());
    let _ = writeln!(
        out,
        "mode          {}",
        serde_json::to_value(policy.policy_mode)
            .ok()
            .and_then(|mode| mode.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned())
    );
    let _ = writeln!(out, "constraints   {}", policy.constraints.len());
    let _ = writeln!(out, "scopes        {}", policy.scopes.len());
    for scope in &policy.scopes {
        let _ = writeln!(
            out,
            "  {} — engagement {}, telemetry egress {}",
            scope.matcher,
            scope.engagement.as_deref().unwrap_or("none"),
            if scope.allow_telemetry_egress {
                "cleared"
            } else {
                "not cleared"
            }
        );
    }
    let _ = writeln!(out, "principals    {}", policy.principals.len());
    let _ = writeln!(out, "terms         {}", policy.terms.len());
    let _ = writeln!(
        out,
        "digest        {}",
        commonmeasure_types::canonical::canonical_digest(
            &serde_json::to_value(&policy)
                .map_err(|error| format!("cannot serialise the policy: {error}"))?
        )
    );
    write_stdout(&out)
}

/// The policy file's JSON Schema, in the form committed beside the contract.
fn show_policy_schema() -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(&commonmeasure_harness::policy::schema())
        .map_err(|error| format!("cannot render the schema: {error}"))?;
    write_stdout(&format!("{rendered}\n"))
}

/// One synchronisation with the pinned signer's desired policy.
fn sync_policy() -> Result<(), String> {
    let home = home_dir().map_err(|error| error.to_string())?;
    let report =
        commonmeasure_harness::managed::sync(&home, &edge_identity(&home), chrono::Utc::now)?;
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

    /// A managed home's forecast names the revision on disk, and where the
    /// hub has not renewed it, since when it has been stale; before any
    /// revision is applied it says so. The state is written as the sync
    /// leaves it, so no hub is needed to reach these branches.
    #[test]
    fn a_managed_forecast_names_the_applied_revision_and_its_staleness() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("deployment.json"),
            json!({"mode": "managed", "organisation": "org-1",
                   "policy_url": "https://hub.example/api/v1/policy",
                   "signer": {"key_id": "k1", "algorithm": "ed25519",
                              "public_key": "00".repeat(32)}})
            .to_string(),
        )
        .unwrap();
        let basis = super::dry_run_basis_text(home.path(), None);
        assert!(
            basis.starts_with("forecast policy: managed, no desired revision applied yet\n"),
            "{basis}"
        );

        let applied = |expires_at: &str| {
            std::fs::create_dir_all(home.path().join("managed")).unwrap();
            std::fs::write(
                home.path().join("managed/state.json"),
                json!({"applied": {"revision": 7, "digest": "sha256:00",
                                   "issued_at": "2026-09-01T00:00:00Z",
                                   "expires_at": expires_at,
                                   "activated_at": "2026-09-01T00:00:01Z",
                                   "signer_key_id": "k1"}})
                .to_string(),
            )
            .unwrap();
        };
        applied("2999-01-01T00:00:00Z");
        let basis = super::dry_run_basis_text(home.path(), None);
        assert!(
            basis.starts_with("forecast policy: managed revision 7, as applied on disk\n"),
            "{basis}"
        );
        assert!(basis.contains("a real run syncs managed policy"), "{basis}");

        applied("2026-09-02T00:00:00Z");
        let basis = super::dry_run_basis_text(home.path(), None);
        assert!(
            basis.starts_with(
                "forecast policy: managed revision 7, as applied on disk (stale since \
                 2026-09-02T00:00:00Z: the hub has not renewed it; the policy stays in force)\n"
            ),
            "{basis}"
        );
    }

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

    /// `doctor`'s automatic relay line names the hosted service as a carrier
    /// only while it holds the home's lock, and names both carriers where
    /// both apply.
    #[test]
    fn the_automatic_relay_line_counts_a_running_hosted_service() {
        let home = tempfile::tempdir().expect("tempdir");
        let home = home.path();
        std::fs::write(
            home.join("relay.json"),
            r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
        )
        .unwrap();
        let off = super::automatic_relay_line(home, false);
        assert!(
            off.starts_with("automatic relay: off, no Claude Code registration sends SessionEnd;"),
            "{off}"
        );
        std::fs::write(
            home.join("hosted-service.json"),
            r#"{"origin":"https://edge.example","hosts":["chatgpt"],"interval_seconds":60}"#,
        )
        .unwrap();
        let stopped = super::automatic_relay_line(home, false);
        assert!(
            stopped.contains("and the hosted service is configured but not running"),
            "{stopped}"
        );
        let lock = std::fs::File::create(home.join("hosted-service.lock")).unwrap();
        lock.lock().expect("held as a running service holds it");
        assert_eq!(
            super::automatic_relay_line(home, false),
            "automatic relay: on the hosted service's interval (every 60s), which relays every \
             session in this home whatever its host"
        );
        let both = super::automatic_relay_line(home, true);
        assert!(
            both.starts_with(
                "automatic relay: at each Claude Code session end (its SessionEnd hook), and on \
                 the hosted service's interval (every 60s)"
            ),
            "{both}"
        );
        drop(lock);
        let hook = super::automatic_relay_line(home, true);
        assert!(
            hook.contains("no other local host sends the event"),
            "{hook}"
        );
    }
}
