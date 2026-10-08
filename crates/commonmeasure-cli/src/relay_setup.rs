//! Relay guidance and the explicit installation offer after managed enrolment.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use commonmeasure_harness::delivery::{SESSION_END_HOSTS, relay_loop_running, service_running};

fn command(macos: bool) -> String {
    if macos {
        "commonmeasure service install relay".to_owned()
    } else {
        format!(
            "commonmeasure relay --every {}",
            crate::relay_loop::DEFAULT_EVERY_SECS
        )
    }
}

pub(crate) fn host_hint(host: &str, running: bool, macos: bool) -> Option<String> {
    if host == "chrome" || SESSION_END_HOSTS.contains(&host) {
        return None;
    }
    Some(if running {
        format!(
            "{host}: an automatic relay holds this home; sources whose licences demand usage reporting still require reporting consent and a receiver that is this edge's hub or the licence's endpoint"
        )
    } else {
        format!(
            "{host}: sources whose licences demand usage reporting are refused without an automatic relay; run `{}`. Reporting consent and a receiver that is this edge's hub or the licence's endpoint are also required",
            command(macos)
        )
    })
}

/// A running hosted service also carries reporting for this home's sessions.
pub(crate) fn running(home: &Path) -> bool {
    relay_loop_running(home) || service_running(home)
}

/// Piped input and CI defer the choice even if a controlling terminal exists.
/// Automation may allocate a terminal without anyone there to answer it.
fn should_prompt(macos: bool, stdin: bool, stdout: bool, ci: bool, skip: bool) -> bool {
    macos && stdin && stdout && !ci && !skip
}

pub(crate) fn offer(home: &Path, skip: bool) -> Result<(), String> {
    if running(home) {
        return Ok(());
    }
    let macos = cfg!(target_os = "macos");
    let agent = macos
        .then(crate::service::relay_context)
        .and_then(Result::ok)
        .and_then(|context| crate::service::relay_agent(&context));
    let other_home = agent
        .as_ref()
        .filter(|agent| !agent.may_serve(home))
        .and_then(|agent| agent.home.clone());
    let installed_here = agent.as_ref().is_some_and(|agent| agent.may_serve(home));
    if agent.is_some() {
        crate::write_stdout(&format!(
            "{}\n",
            crate::relay_loop::finding_for(home, agent).text
        ))?;
    }
    if installed_here {
        return Ok(());
    }
    crate::write_stdout(&format!(
        "Hosts without a session-end event, including Codex, need a running relay for sources whose licences demand usage reporting. Run `{}`.\n",
        command(macos)
    ))?;
    if should_prompt(
        macos,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::env::var_os("CI").is_some_and(|value| !value.is_empty()),
        skip,
    ) && terminal_agrees(other_home.as_deref())?
        && let Err(reason) = crate::service::run(crate::service::ServiceCommand::Install {
            service: crate::service::ServiceName::Relay,
            listen: None,
            every: None,
        })
    {
        crate::write_stdout(&format!(
            "Managed enrolment succeeded; background relay installation failed: {reason}. Retry with `{}`.\n",
            command(macos)
        ))?;
    }
    Ok(())
}

fn terminal_agrees(other_home: Option<&Path>) -> Result<bool, String> {
    // Open the controlling terminal, never a pipe supplying command input.
    // Non-Unix platforms only receive the foreground-loop command.
    #[cfg(unix)]
    {
        let Ok(mut terminal) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        else {
            return Ok(false);
        };
        let reader = terminal.try_clone().map_err(|error| error.to_string())?;
        prompt(
            &mut std::io::BufReader::new(reader),
            &mut terminal,
            other_home,
        )
        .map_err(|error| format!("read relay installation answer: {error}"))
    }
    #[cfg(not(unix))]
    {
        let _ = other_home;
        Ok(false)
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn prompt(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    other_home: Option<&Path>,
) -> std::io::Result<bool> {
    writeln!(
        writer,
        "The background relay sends cleared records to the configured receiver every {} seconds and starts at login.",
        crate::relay_loop::DEFAULT_EVERY_SECS
    )?;
    if let Some(other_home) = other_home {
        writeln!(
            writer,
            "Moving it stops that home's background reporting; hosts without a session-end event there refuse sources whose licences demand usage reporting until another automatic relay runs."
        )?;
        write!(
            writer,
            "Move the background relay from {} to this home? [y/N] ",
            other_home.display()
        )?;
    } else {
        write!(
            writer,
            "Install it with `commonmeasure service install relay` now? [y/N] "
        )?;
    }
    writer.flush()?;
    let mut answer = String::new();
    // EOF and an unanswered prompt never authorise installation.
    reader.read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_follow_the_session_end_hosts_and_the_platform() {
        assert!(host_hint("chrome", false, true).is_none());
        for host in SESSION_END_HOSTS {
            assert!(host_hint(host, false, true).is_none());
        }
        for host in [
            "codex",
            "codex-mcp-client",
            "pi",
            "claude-desktop",
            "cursor",
            "copilot-cli",
            "vscode",
            "future-host",
        ] {
            assert!(
                host_hint(host, false, true)
                    .unwrap()
                    .contains("commonmeasure service install relay")
            );
            let hint = host_hint(host, false, false).unwrap();
            assert!(hint.contains("commonmeasure relay --every 300"));
            assert!(!hint.contains("service install"));
            assert!(
                !host_hint(host, true, true)
                    .unwrap()
                    .contains("service install")
            );
        }
    }

    #[test]
    fn unattended_conditions_never_prompt() {
        assert!(should_prompt(true, true, true, false, false));
        for (macos, stdin, stdout, ci, skip) in [
            (false, true, true, false, false),
            (true, false, true, false, false),
            (true, true, false, false, false),
            (true, true, true, true, false),
            (true, true, true, false, true),
        ] {
            assert!(!should_prompt(macos, stdin, stdout, ci, skip));
        }
    }

    #[test]
    fn moving_a_relay_names_the_previous_home_and_defaults_to_no() {
        let mut output = Vec::new();
        assert!(
            !prompt(
                &mut "\n".as_bytes(),
                &mut output,
                Some(Path::new("/other/edge"))
            )
            .unwrap()
        );
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Move the background relay from /other/edge to this home? [y/N]"));
        assert!(text.contains("stops that home's background reporting"));
        assert!(!text.contains("Install it with"));
    }

    #[test]
    fn installation_requires_an_explicit_terminal_answer() {
        for answer in ["y\n", "Y\n", "yes\n", "YES\n", "Yes\n"] {
            assert!(prompt(&mut answer.as_bytes(), &mut Vec::new(), None).unwrap());
        }
        for answer in ["", "\n", "n\n", "no\n", "maybe\n"] {
            assert!(!prompt(&mut answer.as_bytes(), &mut Vec::new(), None).unwrap());
        }
        struct FailedRead;
        impl std::io::Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("terminal closed"))
            }
        }
        assert!(
            prompt(
                &mut std::io::BufReader::new(FailedRead),
                &mut Vec::new(),
                None
            )
            .is_err()
        );
    }
}
