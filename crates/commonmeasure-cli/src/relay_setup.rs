//! Relay guidance and the explicit installation offer after managed enrolment.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use commonmeasure_harness::delivery::{SESSION_END_HOSTS, relay_loop_running};

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
    if SESSION_END_HOSTS.contains(&host) {
        return None;
    }
    Some(if running {
        format!(
            "{host}: a background relay holds this home; sources whose licences demand usage reporting still require reporting consent and a configured receiver"
        )
    } else {
        format!(
            "{host}: sources whose licences demand usage reporting are refused without an automatic relay; run `{}`. Reporting consent and a configured receiver are also required",
            command(macos)
        )
    })
}

/// A missing terminal is a deferred choice, as in the installer's consent
/// prompt. Captured output must never hold an unattended enrolment open.
pub(crate) fn offer(home: &Path) -> Result<(), String> {
    if relay_loop_running(home) {
        return Ok(());
    }
    let macos = cfg!(target_os = "macos");
    if macos
        && crate::service::relay_context()
            .ok()
            .and_then(|context| crate::service::relay_agent(&context))
            .is_some_and(|agent| agent.may_serve(home))
    {
        return crate::write_stdout(&format!("{}\n", crate::relay_loop::finding(home).text));
    }
    crate::write_stdout(&format!(
        "Hosts without a session-end event, including Codex, need a running relay for sources whose licences demand usage reporting. Run `{}`.\n",
        command(macos)
    ))?;
    if macos && std::io::stdout().is_terminal() && terminal_agrees()? {
        crate::service::run(crate::service::ServiceCommand::Install {
            service: crate::service::ServiceName::Relay,
            listen: None,
            every: None,
        })?;
    }
    Ok(())
}

fn terminal_agrees() -> Result<bool, String> {
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
        prompt(&mut std::io::BufReader::new(reader), &mut terminal)
            .map_err(|error| format!("read relay installation answer: {error}"))
    }
    #[cfg(not(unix))]
    Ok(false)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn prompt(reader: &mut impl BufRead, writer: &mut impl Write) -> std::io::Result<bool> {
    writeln!(
        writer,
        "The background relay sends cleared records to the configured receiver every {} seconds and starts at login.",
        crate::relay_loop::DEFAULT_EVERY_SECS
    )?;
    write!(
        writer,
        "Install it with `commonmeasure service install relay` now? [y/N] "
    )?;
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
            "chrome",
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
    fn installation_requires_an_explicit_terminal_answer() {
        for answer in ["y\n", "Y\n", "yes\n", "YES\n", "Yes\n"] {
            assert!(prompt(&mut answer.as_bytes(), &mut Vec::new()).unwrap());
        }
        for answer in ["", "\n", "n\n", "no\n", "maybe\n"] {
            assert!(!prompt(&mut answer.as_bytes(), &mut Vec::new()).unwrap());
        }
        struct FailedRead;
        impl std::io::Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("terminal closed"))
            }
        }
        assert!(prompt(&mut std::io::BufReader::new(FailedRead), &mut Vec::new()).is_err());
    }
}
