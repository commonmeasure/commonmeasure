//! The steps `install.sh --connect <hub> [--token <token>]` hands to the
//! binary it has just placed: register every host present, connect to the
//! hub under managed policy (by the hub's device-code flow without a
//! token), check the working directory's reporting, install
//! the background relay, make one governed fetch of the first-run page,
//! relay, and print where the evidence shows. A fresh directory needs
//! explicit history confirmation.
//!
//! Each step prints one line. A step that fails stops the run and the
//! command exits non-zero, which the installer reports as its exit status 3;
//! an outcome the operator has to act on (no consent, a hub waiting for its
//! first revision or an approval, a refused page, a directory that cannot be
//! enrolled, a background relay that cannot be installed) is printed and the
//! run goes on. The steps call the functions `install`, `connect`, `enrol`,
//! `service install relay`, `mcp` and `relay` call, so a registration, an
//! enrolment, a LaunchAgent or a crossing is the one those commands make.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use commonmeasure_harness::delivery::{LockState, lock_state, relay_loop_running};
use commonmeasure_harness::registration::{self, HostPaths};
use commonmeasure_harness::{HostSurface, SessionLog, directory};
use serde_json::{Value, json};

/// The page the first run fetches: the company's own site, which carries an
/// RSL licence with a reporting binding (owner decision, 5 October 2026). A
/// constant, so the first run cannot be pointed at another URL by its
/// command line or by the environment of a release build.
pub(crate) const FIRST_RUN_PAGE: &str = "https://commonmeasure.ai/";

/// The first run's word for its own session: the client name it gives in
/// `initialize`, and the host word every record of that session carries
/// (`docs/contracts/session-evidence.md` §Client identity). It names no
/// registered host, so installer traffic is never recorded or delivered as
/// a host's session, and `commonmeasure mcp --host` does not accept it.
const CLIENT_NAME: &str = "commonmeasure-first-run";

/// Where the hub shows delivered evidence, under the hub's base URL.
const FLEET_EVIDENCE_PATH: &str = "/dashboard";

/// How long the relay step waits for a background relay's run in progress
/// to release the spool. A relay started by step 6 runs at once, and that
/// first run can still hold the spool when step 8 starts.
const SPOOL_WAIT: Duration = Duration::from_secs(30);

#[derive(clap::Args)]
pub struct FirstRun {
    /// The hub's base URL, as `commonmeasure connect` takes it.
    hub: String,
    /// The enrolment token the hub's Enrol this machine card mints,
    /// single-use and short-lived. It is passed to the hub and written
    /// nowhere. Without it the connect step prints a code to approve in the
    /// hub, as `commonmeasure connect` does without one.
    #[arg(long)]
    token: Option<String>,
}

pub fn run(args: FirstRun) -> Result<(), String> {
    let Some(token) = args.token else {
        return steps(&args.hub, None, &mut |line: String| {
            crate::write_stdout(&format!("{line}\n"))
        });
    };
    // The exchange trims the token. Protect that same spelling, including
    // peer refusals which the connect, policy and relay clients persist.
    let token = token.trim().to_owned();
    let _protection = commonmeasure_http::EphemeralCredentialGuard::new(&args.hub, &token)
        .map_err(|error| error.to_string())?;
    // Nothing the hub or a step says reaches the terminal with the token in
    // it, whatever the text quotes.
    let redact = |text: String| {
        if token.is_empty() {
            text
        } else {
            text.replace(&token, "<token>")
        }
    };
    steps(&args.hub, Some(&token), &mut |line: String| {
        crate::write_stdout(&format!("{}\n", redact(line)))
    })
    .map_err(redact)
}

fn steps(
    hub: &str,
    token: Option<&str>,
    say: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<(), String> {
    let home = commonmeasure_harness::home_dir().map_err(|error| error.to_string())?;
    // Provider credentials go into the process environment before any
    // thread exists, as `commonmeasure mcp` applies them; the governed fetch
    // runs in this process.
    let credentials = commonmeasure_supply::credentials::apply(&home);

    let paths = HostPaths::from_environment()?;
    hosts(&paths, say)?;

    let connected = match token {
        Some(token) => {
            crate::enrol_edge(&home, hub, crate::Route::Token(token), true).map_err(|error| {
                format!(
                    "connect: {error}. Nothing after it ran; mint a new token on the hub and run \
                     the line again"
                )
            })?
        }
        None => {
            let name = crate::host_name().ok_or(
                "connect: this machine's host name cannot be read, so the hub has no name to \
                 show for it; run `commonmeasure connect <hub> --managed --name NAME`, then the \
                 line again. Nothing after this step ran",
            )?;
            let mut shown = None;
            let connected = crate::enrol_edge(
                &home,
                hub,
                crate::Route::Device {
                    name: &name,
                    retry: "run the line again for a new code",
                    show: &mut |code| {
                        shown = Some(say(format!("connect: {}", crate::approval_line(code))));
                        crate::open_approval_page(code);
                    },
                },
                true,
            )
            .map_err(|error| format!("connect: {error}. Nothing after it ran"));
            shown.transpose()?;
            connected?
        }
    };
    say(connect_line(&connected))?;
    if let Some(failure) = connected.failure {
        return Err(format!("connect: {failure}"));
    }

    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let permitted = enrol(&home, &cwd, say)?;

    let credentials = credentials.map_err(|error| {
        // The enrolment above stands, so the remedy is the file, not the
        // line: running the line again would enrol the machine a second
        // time.
        format!(
            "fetch: not made: the governed fetch cannot run: {error}. Fix or remove that file; \
             the enrolment above stands, and the next governed fetch a host makes reads the \
             file again. Nothing after this step ran"
        )
    })?;
    // The first run's own session sends no session-end event, so the page's
    // reporting demand is met only where a background relay or the hosted
    // service already holds the home when the fetch is ruled on.
    say(background_relay(&home))?;
    say(fetch(&home, &cwd, credentials)?)?;

    say(relay(&home, permitted)?)?;

    say(format!(
        "evidence: {}{FLEET_EVIDENCE_PATH}",
        connected.report.hub.trim_end_matches('/')
    ))?;
    let console = crate::console::locate(None)?;
    say(if console.answers() {
        format!("console: {}", console.url())
    } else {
        format!(
            "console: none answers at {}; run `commonmeasure serve` to open it",
            console.url()
        )
    })
}

/// Step 3: register every host this machine has, each exactly as
/// `commonmeasure install <host>` writes it, and name the rest as not found.
fn hosts(
    paths: &HostPaths,
    say: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<(), String> {
    let binary = registration::resolve_binary(None)?;
    let mut registered = Vec::new();
    let mut absent = Vec::new();
    for surface in registration::HOSTS {
        if registration::detected(surface, paths).is_none() {
            absent.push(surface.id());
            continue;
        }
        if let Err(error) = registration::install(surface, &binary, paths) {
            let done = if registered.is_empty() {
                String::new()
            } else {
                format!(" ({} registered before it)", ids(&registered))
            };
            return Err(format!(
                "hosts: {} was found but not registered: {error}{done}. Fix that and run \
                 the line again; nothing after this step ran",
                surface.id()
            ));
        }
        registered.push(surface);
    }
    let found = if registered.is_empty() {
        "none registered".to_owned()
    } else {
        format!("registered {}", ids(&registered))
    };
    let missing = if absent.is_empty() {
        String::new()
    } else {
        format!("; not found: {}", absent.join(", "))
    };
    say(format!("hosts: {found}{missing}"))
}

fn ids(surfaces: &[HostSurface]) -> String {
    surfaces
        .iter()
        .map(|surface| surface.id())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Step 4's line: who the edge enrolled with and where its policy stands.
fn connect_line(connected: &crate::Connected) -> String {
    let report = &connected.report;
    let enrolled = format!(
        "connect: enrolled with {} in {} as {}",
        report.hub, report.organization.name, report.name
    );
    match (&report.managed, &connected.sync) {
        (Some(Ok(_)), Some(Ok(sync))) if sync.awaiting_first_revision() => format!(
            "{enrolled}; waiting for revision: the organisation has published no policy \
             revision, so the local policy stays in force; publish one on the hub's Policy page"
        ),
        (Some(Ok(_)), Some(Ok(sync))) if sync.converged() => format!(
            "{enrolled}; managed, policy revision {} in force",
            sync.applied
                .as_ref()
                .map(|applied| applied.revision.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ),
        _ => enrolled,
    }
}

/// Step 5: enrol the working directory for hub reporting under its own
/// name, through the code `enrol --name --reporting hub` runs and never
/// with `--include-history`. Whatever stops it, nothing is enrolled, the
/// line names the command to run in a project directory, and the run goes
/// on. Returns whether hub reporting is permitted for the directory.
fn enrol(
    home: &Path,
    cwd: &Path,
    say: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<bool, String> {
    let named = |name: &str| {
        format!(
            "`commonmeasure enrol --name {} --reporting hub --include-history`",
            shell_word(name)
        )
    };
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| home.canonicalize().ok());
    let root = match directory::selected(cwd) {
        Ok(root) if Some(&root) == user_home.as_ref() => {
            say(format!(
                "enrol: nothing enrolled: {} is your home directory, and hub reporting covers \
                 every directory under it; in a project directory run {}",
                root.display(),
                named("<project>")
            ))?;
            return Ok(false);
        }
        Ok(root) => root,
        Err(error) => {
            say(format!(
                "enrol: nothing enrolled: {error}; in a project directory run {}",
                named("<project>")
            ))?;
            return Ok(false);
        }
    };
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A directory enrolled before keeps its enrolment; the line says where
    // its reporting stands, as `enrol --sync` reports it.
    let status = directory::status(home, &root)?;
    if status["project"]["reporting"] == json!(true) {
        let state = status["reporting"].as_str().unwrap_or("unknown");
        let enrolled = format!(
            "enrol: {} is enrolled as {} for hub reporting",
            root.display(),
            status["project"]["name"].as_str().unwrap_or(&name)
        );
        say(match state {
            "permitted" => format!("{enrolled}; permitted"),
            "approval_pending" => format!(
                "{enrolled}; awaiting approval: an owner approves it on the hub's Project \
                 reporting page"
            ),
            other => format!(
                "{enrolled}; reporting {other}: run `commonmeasure enrol --sync` there for the detail"
            ),
        })?;
        return Ok(state == "permitted");
    }
    match crate::enrol::select(home, &root, &name, true, false, |_| Ok(())) {
        Ok(_) => {
            let state = directory::status(home, &root)?["reporting"]
                .as_str()
                .unwrap_or("unknown")
                .to_owned();
            say(format!(
                "enrol: {} enrolled as {name} for hub reporting; reporting {state}",
                root.display()
            ))?;
            Ok(state == "permitted")
        }
        Err(reason) => {
            say(format!(
                "enrol: nothing enrolled: {reason}; to enrol it, run {} in {}",
                named(&name),
                root.display()
            ))?;
            Ok(false)
        }
    }
}

/// `text` as one shell word; `<project>` stays as written, for the operator
/// to replace.
fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if plain || text == "<project>" {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The host word the first run's own session is recorded under:
/// [`CLIENT_NAME`], the word the session evidence contract gives the
/// installer (§Client identity). `claude-code`, the `--host` default, would
/// deliver installer traffic to the hub as a Claude Code session. A debug
/// build takes another word from `COMMONMEASURE_TEST_FIRST_RUN_HOST`; a
/// release build reads no variable.
fn session_host() -> String {
    #[cfg(debug_assertions)]
    if let Some(word) = std::env::var("COMMONMEASURE_TEST_FIRST_RUN_HOST")
        .ok()
        .filter(|word| !word.is_empty())
    {
        return word;
    }
    CLIENT_NAME.to_owned()
}

/// [`FIRST_RUN_PAGE`], or in a debug build the fixture page
/// `COMMONMEASURE_TEST_FIRST_RUN_PAGE` names, as `COMMONMEASURE_TEST_HOSTS`
/// points a name at a loopback server.
fn page() -> String {
    #[cfg(debug_assertions)]
    if let Some(page) = std::env::var("COMMONMEASURE_TEST_FIRST_RUN_PAGE")
        .ok()
        .filter(|page| !page.is_empty())
    {
        return page;
    }
    FIRST_RUN_PAGE.to_owned()
}

/// Step 7: one `context_fetch` of the first-run page in a session of its
/// own, served by the same server `commonmeasure mcp` runs, with nothing
/// added to the ruling. A refusal is an outcome, reported in the edge's
/// words, never retried and never replaced by another page.
fn fetch(
    home: &Path,
    cwd: &Path,
    credentials: commonmeasure_supply::credentials::CredentialsStatus,
) -> Result<String, String> {
    let page = page();
    let consent = commonmeasure_harness::consent::Standing::load(home);
    let consent_note = if consent.agreed() {
        String::new()
    } else {
        format!(
            "; reporting consent is {}: sources whose licence demands reporting are refused until \
             you run `commonmeasure consent agree`",
            consent.state().replace('_', " ")
        )
    };
    let host = session_host();
    let session_id = format!("first-run-{}", crate::uuid_like_session());
    let mut server = crate::mcp_session::open(
        home,
        &host,
        &session_id,
        Some(cwd.display().to_string()),
        None,
        credentials,
        crate::mcp_session::Transport::STDIO,
    )
    .map_err(|error| format!("fetch: the session could not open: {error}"))?;
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
        },
    });
    server.handle_message_text(&initialize.to_string());
    server.handle_message_text(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "context_fetch", "arguments": {"url": page}},
    });
    let answer = server
        .handle_message_text(&call.to_string())
        .ok_or("fetch: the server gave no answer to context_fetch")?;
    let log = session_log(home, &session_id);
    let crossing = log
        .as_deref()
        .and_then(|log| SessionLog::read(log).ok())
        .and_then(|records| {
            records.into_iter().rev().find(|record| {
                record["event"] == "crossing_mediated" || record["event"] == "crossing_refused"
            })
        });
    let recorded = log
        .map(|log| format!("; recorded in {}", log.display()))
        .unwrap_or_default();
    if answer["result"]["isError"] == json!(true) || answer.get("error").is_some() {
        let reason = commonmeasure_harness::provenance::payload_text(&answer["result"])
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|text| text["error"].as_str().map(str::to_owned))
            .or_else(|| answer["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "the server's answer carried no reason".to_owned());
        return Ok(format!(
            "fetch: {page} refused: {reason}{consent_note}{recorded}"
        ));
    }
    let payload = crossing.as_ref().map(|record| &record["payload"]);
    // Every licence the page's declarations named, as the record holds them
    // (`docs/contracts/session-evidence.md` §Source declarations).
    let licences: Vec<&str> = payload
        .and_then(|payload| payload["declarations"]["licences"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|licence| licence["url"].as_str())
        .collect();
    let licence = if licences.is_empty() {
        "; no licence declared".to_owned()
    } else {
        format!("; licence {}", licences.join(", "))
    };
    let route = payload
        .and_then(|payload| payload["declarations"]["reporting"]["route"].as_str())
        .map(|route| format!(", reported through the {}", route.replace('_', " ")))
        .unwrap_or_default();
    Ok(format!(
        "fetch: {page} delivered, mediated{licence}{route}{recorded}"
    ))
}

fn session_log(home: &Path, session_id: &str) -> Option<PathBuf> {
    SessionLog::list(home).ok()?.into_iter().find(|path| {
        path.file_stem()
            .is_some_and(|stem| stem.to_string_lossy() == session_id)
    })
}

/// Step 8: one relay run through the code `commonmeasure relay` runs.
fn relay(home: &Path, permitted: bool) -> Result<String, String> {
    // The clearances the relay reads come from the policy on disk, so a
    // managed edge refreshes it first, as `commonmeasure relay` does.
    let _ = crate::sync_managed_policy(home, commonmeasure_harness::managed::DEFAULT_BUDGET);
    // A run that finds the spool held exits without sending, so this one
    // waits for a background relay's run in progress, within bounds.
    if relay_loop_running(home) {
        let spool = commonmeasure_relay::spool::lock_path(home);
        let deadline = Instant::now() + SPOOL_WAIT;
        while lock_state(&spool) == LockState::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let report = commonmeasure_relay::relay(home, &commonmeasure_relay::RelayOptions::default())
        .map_err(|error| {
            let text = match error.downcast_ref::<commonmeasure_relay::DeliveryFailure>() {
                Some(failure) => failure.delivery_text(),
                None => format!("{error:#}"),
            };
            format!(
                "relay: {text}. Everything before it stands; run `commonmeasure relay` to retry"
            )
        })?;
    Ok(format!(
        "relay: delivered {} event(s) in {} batch(es) to {}{}{}",
        report.events_delivered,
        report.batches_delivered,
        report.receiver,
        report
            .events_new_at_receiver
            .map(|count| format!(" ({count} new at the receiver)"))
            .unwrap_or_default(),
        if permitted {
            ""
        } else {
            // A directory enrolled and still awaiting approval clears
            // nothing either, so the line does not say none is enrolled.
            "; hub reporting is permitted for no directory yet, so only a use admitted under \
             reporting consent is cleared to leave"
        }
    ))
}

/// Step 6: a background relay for this home before the fetch. The first
/// run's own session, and every host but Claude Code, sends no session-end
/// event, so only a carrier that relays the home on an interval delivers
/// their reports without a person. On macOS the relay service is installed,
/// as `commonmeasure service install relay` installs it, where none exists,
/// and the step waits until it holds the home. One serving another home is
/// left where it is, since moving it stops that home's reporting. Elsewhere,
/// or where the install fails, the line names the remedy and the run goes
/// on: the fetch is then refused for want of automatic delivery.
fn background_relay(home: &Path) -> String {
    let every = crate::relay_loop::DEFAULT_EVERY_SECS;
    if relay_loop_running(home) {
        return "service: a background relay holds this edge home".to_owned();
    }
    if crate::relay_setup::running(home) {
        return "service: the hosted service relays this edge home".to_owned();
    }
    if !crate::service::launch_agents() {
        return format!(
            "service: no background relay holds this edge home; run one under your service \
             manager: commonmeasure relay --every {every}"
        );
    }
    let context = match crate::service::relay_context() {
        Ok(context) => context,
        Err(reason) => return install_failed(&reason),
    };
    match crate::service::relay_agent(&context) {
        Some(agent) if agent.may_serve(home) => {
            "service: the background relay service is installed for this edge home, but no \
             relay holds it; `commonmeasure doctor` says why, and `commonmeasure service \
             install relay` starts it again"
                .to_owned()
        }
        Some(_) => {
            "service: the background relay service relays another home; to move it here, run \
             `commonmeasure service install relay`"
                .to_owned()
        }
        None => match crate::service::install_relay(&context, &crate::service::System, every) {
            Ok(_) => format!(
                "service: background relay service installed; it relays this edge home every \
                 {every} s"
            ),
            Err(reason) => install_failed(&reason),
        },
    }
}

fn install_failed(reason: &str) -> String {
    format!(
        "service: installing the background relay service failed: {reason}. Retry with \
         `commonmeasure service install relay`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_name_is_quoted_only_where_the_shell_needs_it() {
        assert_eq!(shell_word("site-2"), "site-2");
        assert_eq!(shell_word("<project>"), "<project>");
        assert_eq!(shell_word("my project"), "'my project'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word("a<b"), "'a<b'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn a_release_build_fetches_only_the_compiled_page() {
        assert_eq!(FIRST_RUN_PAGE, "https://commonmeasure.ai/");
    }

    /// A release build records the session under the installer's word
    /// whatever the environment holds; run under `--release` with
    /// `COMMONMEASURE_TEST_FIRST_RUN_HOST` set, this shows the variable is
    /// not read there.
    #[test]
    fn the_session_is_recorded_under_the_installers_word() {
        if cfg!(debug_assertions) && std::env::var_os("COMMONMEASURE_TEST_FIRST_RUN_HOST").is_some()
        {
            return;
        }
        assert_eq!(session_host(), "commonmeasure-first-run");
    }
}
