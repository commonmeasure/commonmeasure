//! The steps `install.sh --connect <hub> --token <token>` hands to the binary
//! it has just placed: register every host present, connect to the hub
//! under managed policy, check the working directory's reporting, relay,
//! and print where the evidence shows. A fresh directory needs explicit
//! history confirmation; a release build makes no first-run fetch until
//! the session record has a host word for installer traffic.
//!
//! Each step prints one line. A step that fails stops the run and the
//! command exits non-zero, which the installer reports as its exit status 3;
//! an outcome the operator has to act on (no consent, a hub waiting for its
//! first revision or an approval, a refused page, a directory that cannot be
//! enrolled) is printed and the run goes on. The steps call the functions
//! `install`, `connect`, `enrol`, `mcp` and `relay` call, so a registration,
//! an enrolment or a crossing is the one those commands make.

use std::path::{Path, PathBuf};

use commonmeasure_harness::registration::{self, HostPaths};
use commonmeasure_harness::{HostSurface, SessionLog, delivery, directory};
use serde_json::{Value, json};

/// The page the first run fetches: the company's own site, which carries an
/// RSL licence with a reporting binding (owner decision, 5 October 2026). A
/// constant, so the first run cannot be pointed at another URL by its
/// command line or by the environment of a release build.
pub(crate) const FIRST_RUN_PAGE: &str = "https://commonmeasure.ai/";

/// The client name the first run gives its own session in `initialize`, so
/// the record names the program that asked.
const CLIENT_NAME: &str = "commonmeasure-first-run";

/// Where the hub shows delivered evidence, under the hub's base URL.
const FLEET_EVIDENCE_PATH: &str = "/dashboard";

#[derive(clap::Args)]
pub struct FirstRun {
    /// The hub's base URL, as `commonmeasure connect` takes it.
    hub: String,
    /// The enrolment token the hub's Enrol this machine card mints,
    /// single-use and short-lived. It is passed to the hub and written
    /// nowhere.
    #[arg(long)]
    token: String,
}

pub fn run(args: FirstRun) -> Result<(), String> {
    // The exchange trims the token. Protect that same spelling, including
    // peer refusals which the connect, policy and relay clients persist.
    let token = args.token.trim().to_owned();
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
    steps(&args.hub, &token, &mut |line: String| {
        crate::write_stdout(&format!("{}\n", redact(line)))
    })
    .map_err(redact)
}

fn steps(
    hub: &str,
    token: &str,
    say: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<(), String> {
    let home = commonmeasure_harness::home_dir().map_err(|error| error.to_string())?;
    // Provider credentials go into the process environment before any
    // thread exists, as `commonmeasure mcp` applies them; the governed fetch
    // runs in this process.
    let credentials = commonmeasure_supply::credentials::apply(&home);

    let paths = HostPaths::from_environment()?;
    let registered = hosts(&paths, say)?;

    let connected = crate::enrol_edge(&home, hub, token, true)
        .map_err(|error| format!("connect: {error}. Nothing after it ran; mint a new token on the hub and run the line again"))?;
    say(connect_line(&connected))?;
    if let Some(failure) = connected.failure {
        return Err(format!("connect: {failure}"));
    }

    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let permitted = enrol(&home, &cwd, say)?;

    let credentials = credentials.map_err(|error| {
        // The enrolment above stands, so the remedy is the file, not the
        // line: a new token would enrol the machine a second time.
        format!(
            "fetch: not made: the governed fetch cannot run: {error}. Fix or remove that file; \
             the enrolment above stands, and the next governed fetch a host makes reads the \
             file again. Nothing after this step ran"
        )
    })?;
    say(fetch(&home, &cwd, credentials)?)?;

    say(relay(&home, permitted)?)?;
    if let Some(line) = background_relay(&home, &registered)? {
        say(line)?;
    }

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
) -> Result<Vec<HostSurface>, String> {
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
    say(format!("hosts: {found}{missing}"))?;
    Ok(registered)
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

/// The host word the first run's own session is recorded under. The session
/// evidence contract names a word for each registration only (§Client
/// identity), and the first run is not one of those hosts: recording it
/// under `claude-code`, the default, would deliver installer traffic to the
/// hub as a Claude Code session. Until the contract names a word for it,
/// a release build makes no fetch and says so. A debug build takes the word
/// from `COMMONMEASURE_TEST_FIRST_RUN_HOST`, so the tests drive the rest of
/// the step through the production path.
fn session_host() -> Option<String> {
    #[cfg(debug_assertions)]
    if let Some(word) = std::env::var("COMMONMEASURE_TEST_FIRST_RUN_HOST")
        .ok()
        .filter(|word| !word.is_empty())
    {
        return Some(word);
    }
    None
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

/// Step 6: one `context_fetch` of the first-run page in a session of its
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
    let Some(host) = session_host() else {
        return Ok(format!(
            "fetch: not made: the session record has no host word for a fetch the installer \
             makes, and each word it has names a host this is not, so {page} was not fetched{consent_note}"
        ));
    };
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
        let reason = answer["result"]["content"][0]["text"]
            .as_str()
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

/// Step 7: one relay run through the code `commonmeasure relay` runs.
fn relay(home: &Path, permitted: bool) -> Result<String, String> {
    // The clearances the relay reads come from the policy on disk, so a
    // managed edge refreshes it first, as `commonmeasure relay` does.
    let _ = crate::sync_managed_policy(home, commonmeasure_harness::managed::DEFAULT_BUDGET);
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

/// Part of step 7: a host registered here that sends no session-end event
/// has its reports delivered only by a background relay. On macOS the
/// relay service is installed where none exists; one serving another home
/// is left where it is, since moving it stops that home's reporting, and
/// the command is printed. Elsewhere the service-manager line is printed.
fn background_relay(home: &Path, registered: &[HostSurface]) -> Result<Option<String>, String> {
    let without_end: Vec<&str> = registered
        .iter()
        .map(|surface| surface.id())
        .filter(|id| *id != "chrome" && !delivery::SESSION_END_HOSTS.contains(id))
        .collect();
    if without_end.is_empty() || crate::relay_setup::running(home) {
        return Ok(None);
    }
    let hosts = without_end.join(", ");
    if cfg!(target_os = "macos") {
        let agent = crate::service::relay_context()
            .ok()
            .and_then(|context| crate::service::relay_agent(&context));
        if let Some(agent) = agent {
            return Ok(Some(match agent.may_serve(home) {
                true => format!(
                    "relay: the background relay service is installed for this home ({hosts} send no session-end event); `commonmeasure doctor` says whether it runs"
                ),
                false => format!(
                    "relay: {hosts} send no session-end event, and the background relay service relays another home; to move it here, run `commonmeasure service install relay`"
                ),
            }));
        }
        crate::service::run(crate::service::ServiceCommand::Install {
            service: crate::service::ServiceName::Relay,
            listen: None,
            every: None,
        })
        .map_err(|reason| {
            format!(
                "relay: {hosts} send no session-end event, and installing the background relay \
                 failed: {reason}. Retry with `commonmeasure service install relay`"
            )
        })?;
        return Ok(Some(format!(
            "relay: background relay service installed for {hosts}, which send no session-end event"
        )));
    }
    Ok(Some(format!(
        "relay: {hosts} send no session-end event; run the background relay under your service \
         manager: commonmeasure relay --every {}",
        crate::relay_loop::DEFAULT_EVERY_SECS
    )))
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
}
