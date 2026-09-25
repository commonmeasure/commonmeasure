//! Whether reports leave this operator home without a person.
//!
//! A licence's reporting demand is met only where the events will actually
//! be sent: the scope clears telemetry egress, a receiver is configured, and
//! delivery happens by itself (owner decision, 22 September 2026). The last
//! of those three is what this module answers.
//!
//! Two things relay without a person: the session-end hook, on a host whose
//! registration sends a session-end event, and the hosted service's interval
//! relay, which relays every session in its home, stdio sessions included.
//! The ruling reads which one a session has from the session itself (the
//! `--host` word, the client's `initialize` name, and whether the transport
//! runs an interval relay) and from the home's own state (the marker, and
//! whether a hosted service holds the home's lock), never from host files
//! under `$HOME`: whether the hook is actually installed is `doctor`'s check,
//! and reading it here would make a licence ruling depend on the developer's
//! machine.
//!
//! The operator switches automatic delivery off by writing the marker file
//! `relay/manual` in the home. It is a file rather than a `relay.json` field
//! because that configuration refuses unknown fields, so a field would stop
//! an older binary reading the file at all. The marker's content is not read;
//! its presence is the whole signal.

use std::path::{Path, PathBuf};

/// The marker's path under the operator home, as the operator writes it.
pub const MANUAL_MARKER: &str = "relay/manual";

/// Where the marker lives in `home`.
pub fn manual_marker(home: &Path) -> PathBuf {
    home.join("relay").join("manual")
}

/// Why nothing would be delivered without a person, or `None` where
/// automatic delivery is in force. The reason names the marker, because the
/// marker is the only thing an operator has to remove to restore delivery.
pub fn withheld_reason(home: &Path) -> Option<String> {
    let path = manual_marker(home);
    path.exists().then(|| {
        format!(
            "the marker file {} switches automatic delivery off, so reports leave only when \
             someone runs `commonmeasure relay`",
            path.display()
        )
    })
}

/// Whether the session-end relay and the hosted service may deliver.
pub fn automatic(home: &Path) -> bool {
    withheld_reason(home).is_none()
}

/// The `--host` words whose registration sends a session-end event, which
/// starts the relay. Claude Code's hook table (`registration::CLAUDE_HOOKS`)
/// registers `SessionEnd`; Cursor's and the Copilot CLI's
/// (`registration::CURSOR_HOOKS`, `registration::COPILOT_HOOKS`) register
/// no session end, and Codex, Pi, Claude Desktop and VS Code register no
/// hook that runs at one. A test in `registration` holds this list to those
/// tables.
pub const SESSION_END_HOSTS: [&str; 1] = ["claude-code"];

/// The name Claude Code gives itself in `initialize`. Every Claude Code
/// session recorded on a development machine announced
/// `{"name": "claude-code", "title": "Claude Code"}`: 10 of 10
/// `client_identified` records, Claude Code 2.1.270 to 2.1.278, September
/// 2026 (`docs/contracts/session-evidence.md` §Client identity).
pub const CLAUDE_CODE_CLIENT: &str = "claude-code";

/// Client names from `initialize` known to belong to a host other than
/// Claude Code (`docs/contracts/session-evidence.md` §Client identity). They
/// only make the refusal more precise: any name but [`CLAUDE_CODE_CLIENT`]
/// is refused under the `claude-code` host word.
const OTHER_CLIENTS: [&str; 3] = ["codex-mcp-client", "claude-ai", "cursor-vscode"];

/// Claude Desktop's local agent client names itself after the server.
const OTHER_CLIENT_PREFIXES: [&str; 1] = ["local-agent-mode-"];

/// The lock a running hosted service holds on its home, as the service
/// takes it (`commonmeasure hosted service`).
pub const SERVICE_LOCK_FILE: &str = "hosted-service.lock";

/// Whether a hosted service holds `home`'s lock now, and so relays every
/// session in the home on its interval. Takes the lock for an instant when
/// nobody holds it, so a service starting in exactly that instant is refused
/// once and is started again by its supervisor. A service configured but
/// stopped holds nothing and reads as not running.
pub fn service_running(home: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .open(home.join(SERVICE_LOCK_FILE))
    else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// How a session's reports would leave without a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionDelivery<'a> {
    /// The `--host` word, or the hosted endpoint's.
    pub host: &'a str,
    /// The client's own name from `initialize`, where it sent one.
    pub client: Option<&'a str>,
    /// The transport runs an interval relay over this home: the hosted
    /// service.
    pub interval_relay: bool,
}

impl SessionDelivery<'_> {
    /// Why nothing from this session would be delivered without a person, or
    /// `None` where automatic delivery is in force. The marker is checked
    /// first because it applies to every carrier.
    pub fn withheld_reason(&self, home: &Path) -> Option<String> {
        if let Some(reason) = withheld_reason(home) {
            return Some(reason);
        }
        // A running hosted service relays every session in its home on its
        // interval, whatever host or client this session has.
        if self.interval_relay || service_running(home) {
            return None;
        }
        if !SESSION_END_HOSTS.contains(&self.host) {
            return Some(format!(
                "no automatic delivery; this host ({}) sends no session-end event, so reports \
                 leave only when someone runs `commonmeasure relay`",
                self.host
            ));
        }
        // `--host` defaults to `claude-code`, so the host word alone does not
        // say the client is Claude Code: a hand-written registration without
        // `--host` (Goose, or any client not yet captured) reads as Claude
        // Code by it. Only the name Claude Code announces counts. The failure
        // modes are not symmetric: a name missing here refuses visibly and is
        // fixed by one string, where a name missing from a blocklist would
        // release content with nothing relaying its report. A session with no
        // `clientInfo` trusts the host word; MCP clients must send it, so
        // that arises only in tests.
        match self.client {
            None | Some(CLAUDE_CODE_CLIENT) => None,
            Some(client) if is_other_client(client) => Some(format!(
                "no automatic delivery; the client {client} is not Claude Code and sends no \
                 session-end event, although the server was started for {}, so reports leave \
                 only when someone runs `commonmeasure relay`",
                self.host
            )),
            Some(client) => Some(format!(
                "no automatic delivery; the client {client} does not announce itself as Claude \
                 Code ({CLAUDE_CODE_CLIENT}), the one client known to send a session-end event, \
                 although the server was started for {}, so reports leave only when someone \
                 runs `commonmeasure relay`",
                self.host
            )),
        }
    }
}

fn is_other_client(name: &str) -> bool {
    OTHER_CLIENTS.contains(&name)
        || OTHER_CLIENT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_switches_automatic_delivery_off_and_names_itself() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(automatic(home.path()));
        assert!(withheld_reason(home.path()).is_none());
        std::fs::create_dir_all(home.path().join("relay")).unwrap();
        std::fs::write(manual_marker(home.path()), b"").unwrap();
        assert!(!automatic(home.path()));
        let reason = withheld_reason(home.path()).expect("the marker is a reason");
        assert!(reason.contains("manual"), "{reason}");
    }

    fn session<'a>(
        host: &'a str,
        client: Option<&'a str>,
        interval_relay: bool,
    ) -> SessionDelivery<'a> {
        SessionDelivery {
            host,
            client,
            interval_relay,
        }
    }

    #[test]
    fn only_a_session_end_host_or_an_interval_relay_delivers_by_itself() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(
            session("claude-code", None, false)
                .withheld_reason(home.path())
                .is_none()
        );
        assert!(
            session("claude-code", Some("claude-code"), false)
                .withheld_reason(home.path())
                .is_none()
        );
        for host in [
            "codex",
            "pi",
            "claude-desktop",
            "cursor",
            "copilot-cli",
            "vscode",
        ] {
            let reason = session(host, None, false)
                .withheld_reason(home.path())
                .unwrap_or_else(|| panic!("{host} sends no session-end event"));
            assert!(
                reason.contains("no automatic delivery")
                    && reason.contains(&format!("this host ({host})"))
                    && reason.contains("sends no session-end event"),
                "{reason}"
            );
        }
        for host in [
            "claude-connector",
            "chatgpt",
            "m365-copilot",
            "copilot-cloud-agent",
        ] {
            assert!(
                session(host, None, true)
                    .withheld_reason(home.path())
                    .is_none(),
                "{host}"
            );
            assert!(
                session(host, None, false)
                    .withheld_reason(home.path())
                    .is_some(),
                "{host}"
            );
        }
    }

    #[test]
    fn a_known_other_client_under_the_default_host_word_is_not_automatic() {
        let home = tempfile::tempdir().expect("tempdir");
        for client in [
            "codex-mcp-client",
            "claude-ai",
            "cursor-vscode",
            "local-agent-mode-commonmeasure",
        ] {
            let reason = session("claude-code", Some(client), false)
                .withheld_reason(home.path())
                .unwrap_or_else(|| panic!("{client} sends no session-end event"));
            assert!(reason.contains(client), "{reason}");
        }
    }

    #[test]
    fn an_unknown_client_under_the_default_host_word_is_refused_and_named() {
        let home = tempfile::tempdir().expect("tempdir");
        // Goose has no installer entry and is registered by hand without
        // `--host`, so its sessions carry the default host word.
        for client in ["goose", "junie-client", "Claude Code"] {
            let reason = session("claude-code", Some(client), false)
                .withheld_reason(home.path())
                .unwrap_or_else(|| panic!("{client} is not Claude Code's announced name"));
            assert!(
                reason.contains(&format!("the client {client} does not announce itself"))
                    && reason.contains("claude-code"),
                "{reason}"
            );
        }
        assert!(
            session("claude-code", Some(CLAUDE_CODE_CLIENT), false)
                .withheld_reason(home.path())
                .is_none()
        );
    }

    #[test]
    fn a_running_hosted_service_delivers_for_every_session_in_its_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let codex = session("codex", Some("codex-mcp-client"), false);
        let goose = session("claude-code", Some("goose"), false);
        assert!(!service_running(home.path()));
        assert!(codex.withheld_reason(home.path()).is_some());
        // A configured but stopped service leaves the lock file unheld.
        let path = home.path().join(SERVICE_LOCK_FILE);
        let file = std::fs::File::create(&path).expect("lock file");
        assert!(!service_running(home.path()));
        assert!(goose.withheld_reason(home.path()).is_some());
        file.lock().expect("held as the service holds it");
        assert!(service_running(home.path()));
        assert!(codex.withheld_reason(home.path()).is_none());
        assert!(goose.withheld_reason(home.path()).is_none());
        // The marker still outranks the service.
        std::fs::create_dir_all(home.path().join("relay")).unwrap();
        std::fs::write(manual_marker(home.path()), b"").unwrap();
        let reason = codex.withheld_reason(home.path()).expect("the marker");
        assert!(reason.contains("manual"), "{reason}");
        drop(file);
        std::fs::remove_file(manual_marker(home.path())).unwrap();
        assert!(!service_running(home.path()));
        assert!(codex.withheld_reason(home.path()).is_some());
    }

    #[test]
    fn the_marker_withholds_whatever_the_carrier() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(home.path().join("relay")).unwrap();
        std::fs::write(manual_marker(home.path()), b"").unwrap();
        for delivery in [
            session("claude-code", None, false),
            session("claude-connector", None, true),
        ] {
            let reason = delivery.withheld_reason(home.path()).expect("the marker");
            assert!(reason.contains("manual"), "{reason}");
        }
    }
}
