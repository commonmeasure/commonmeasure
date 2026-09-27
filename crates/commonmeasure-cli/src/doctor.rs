//! `commonmeasure doctor`: the edge checked against the machine, as one
//! report of findings.
//!
//! The report is in sections. The Edge home first, because every host's
//! sessions share it: whether a session log can be written and whether the
//! policy loads, and on a managed edge which revision is in force. Then the
//! console, so the operator reading a policy finding knows where the page
//! that edits the policy is. Then the relay: the hosted service, the
//! background relay, the batches and the last delivery, and whether the
//! relay runs without a person. Then each host's registration. Each finding
//! carries the standing the code that established it gave it
//! (`commonmeasure_types::Finding`), and the report ends by counting the
//! ones that need the operator.
//!
//! Exits zero whatever it finds: the report is the result. `--json` prints
//! the same findings as a document (`docs/contracts/host-integration.md`
//! §Registration), for a script or a support thread; the text form is the
//! same words with a mark before each and, in a terminal, colour.
//!
//! Nothing here leaves the machine. The console is probed on loopback; the
//! one lookup, of the name `--resolve` gives, is made only when asked for.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use commonmeasure_harness::{HostSurface, home_dir, registration};
use commonmeasure_types::{Finding, Standing};
use serde_json::json;

use crate::report::{Palette, Tally};
use crate::{console, hosted, inspect, relay_loop, write_stdout};

/// The identifier of the JSON document `doctor --json` prints.
pub const CONTRACT: &str = "commonmeasure-doctor/v1";

/// Every host a registration is read back for, in the order the report
/// prints them.
const ALL_HOSTS: [HostSurface; 8] = [
    HostSurface::ClaudeCode,
    HostSurface::Codex,
    HostSurface::Pi,
    HostSurface::ClaudeDesktop,
    HostSurface::Cursor,
    HostSurface::CopilotCli,
    HostSurface::VsCode,
    HostSurface::Chrome,
];

/// One titled group of findings.
struct Section {
    id: &'static str,
    title: &'static str,
    findings: Vec<Finding>,
}

/// The whole report, before it is rendered one way or the other.
struct Report {
    version: &'static str,
    binary: String,
    home: PathBuf,
    console: Result<console::Located, String>,
    sections: Vec<Section>,
    hosts: Vec<registration::HostReport>,
    /// Set when one host was named: its absence is then something to act
    /// on, where in the full report it is a fact about the machine.
    asked: Option<&'static str>,
}

pub fn run(
    host: Option<&str>,
    resolve: Option<&str>,
    json: bool,
    palette: Palette,
) -> Result<(), String> {
    let report = gather(host, resolve)?;
    if json {
        return write_stdout(&format!(
            "{}\n",
            serde_json::to_string_pretty(&report.as_json()).map_err(|error| error.to_string())?
        ));
    }
    write_stdout(&report.render(palette))
}

fn gather(host: Option<&str>, resolve: Option<&str>) -> Result<Report, String> {
    let paths = registration::HostPaths::from_environment()?;
    let home = home_dir().map_err(|error| error.to_string())?;
    let asked = host.map(crate::host_named).transpose()?;
    let surfaces: Vec<HostSurface> = match asked {
        Some(surface) => vec![surface],
        None => ALL_HOSTS.to_vec(),
    };
    let version = env!("CARGO_PKG_VERSION");
    let console = console::locate(None);

    let mut edge = registration::home_findings(&home);
    edge.extend(managed_policy_finding(&home));

    let mut console_findings = vec![match &console {
        Ok(located) => located.finding(version),
        Err(error) => Finding::unknown(format!("console: {error}")),
    }];
    if let Ok(located) = &console
        && located.answers()
    {
        console_findings.push(Finding::note(format!(
            "policy page: {} shows the policy in force and edits the mode, a scope's denied \
             hosts and the attribution rules; `commonmeasure console open policy` opens it",
            located.page_url(console::Page::Policy)
        )));
    }

    let mut relay = vec![hosted::service_finding(&home), relay_loop::finding(&home)];
    relay.extend(commonmeasure_relay::egress_findings(
        &commonmeasure_relay::egress_report(&home),
    ));
    let now = chrono::Utc::now();
    relay.push(commonmeasure_relay::state::last_delivery(&home, now));
    relay.push(automatic_relay_finding(
        &home,
        registration::session_end_registered(&paths),
    ));

    let mut sections = vec![
        Section {
            id: "home",
            title: "Edge home",
            findings: edge,
        },
        Section {
            id: "console",
            title: "Console",
            findings: console_findings,
        },
        Section {
            id: "relay",
            title: "Relay",
            findings: relay,
        },
    ];
    if let Some(name) = resolve {
        sections.push(Section {
            id: "resolution",
            title: "Resolution",
            findings: vec![resolution_finding(name, &|url| {
                commonmeasure_http::resolve(url).map_err(|error| format!("{error:#}"))
            })],
        });
    }
    Ok(Report {
        version,
        binary: std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "unresolved".to_owned()),
        home,
        console,
        sections,
        hosts: surfaces
            .into_iter()
            .map(|surface| registration::doctor(surface, &paths))
            .collect(),
        asked: asked.map(HostSurface::id),
    })
}

impl Report {
    /// A host's own standing: registered is as it should be; not registered
    /// is a fact about the machine, unless this host is the one asked
    /// about, when its absence is the answer to act on.
    fn host_standing(&self, host: &registration::HostReport) -> Standing {
        if host.registered {
            Standing::Ok
        } else if self.asked == Some(host.host) {
            Standing::Attention
        } else {
            Standing::Note
        }
    }

    fn host_heading(&self, host: &registration::HostReport) -> Finding {
        Finding {
            standing: self.host_standing(host),
            text: format!(
                "{:<12} {}",
                host.host,
                if host.registered {
                    "registered"
                } else {
                    "not registered"
                }
            ),
        }
    }

    /// Every finding in the report, the host headings included, for the
    /// count it ends with.
    fn tally(&self) -> Tally {
        let headings: Vec<Finding> = self
            .hosts
            .iter()
            .map(|host| self.host_heading(host))
            .collect();
        Tally::of(
            self.sections
                .iter()
                .flat_map(|section| section.findings.iter())
                .chain(self.hosts.iter().flat_map(|host| host.findings.iter()))
                .chain(headings.iter()),
        )
    }

    fn registered(&self) -> usize {
        self.hosts.iter().filter(|host| host.registered).count()
    }

    /// The sentence the report ends with: how many hosts are registered,
    /// and what needs attention.
    fn summary(&self) -> Finding {
        let tally = self.tally();
        let hosts = match (self.asked, self.hosts.as_slice()) {
            (Some(_), [host]) => format!(
                "{} {}",
                host.host,
                if host.registered {
                    "registered"
                } else {
                    "not registered"
                }
            ),
            _ => format!(
                "{} of {} hosts registered",
                self.registered(),
                self.hosts.len()
            ),
        };
        Finding {
            standing: tally.standing(),
            text: format!("{hosts}; {}", tally.sentence()),
        }
    }

    fn render(&self, palette: Palette) -> String {
        let mut out = format!("{}\n", palette.heading("Common Measure doctor"));
        out.push_str(&palette.row(
            None,
            "binary",
            &format!("{} ({})", self.binary, self.version),
            8,
        ));
        out.push_str(&palette.row(None, "home", &self.home.display().to_string(), 8));
        for section in &self.sections {
            let _ = write!(out, "\n{}\n", palette.heading(section.title));
            out.push_str(&palette.findings(&section.findings, 2));
        }
        let _ = write!(out, "\n{}\n", palette.heading("Hosts"));
        for host in &self.hosts {
            out.push_str(&palette.finding(&self.host_heading(host), 2));
            out.push_str(&palette.findings(&host.findings, 4));
        }
        let _ = write!(out, "\n{}", palette.finding(&self.summary(), 0));
        out
    }

    fn as_json(&self) -> serde_json::Value {
        let tally = self.tally();
        let mut summary = tally.as_json();
        summary["hosts_registered"] = json!(self.registered());
        summary["hosts"] = json!(self.hosts.len());
        summary["standing"] = json!(tally.standing());
        summary["text"] = json!(self.summary().text);
        json!({
            "contract": CONTRACT,
            "generated_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "version": self.version,
            "binary": self.binary,
            "home": self.home.display().to_string(),
            "console": match &self.console {
                Ok(located) => located.as_json(self.version),
                Err(error) => json!({"unavailable": error}),
            },
            "sections": self.sections.iter().map(|section| json!({
                "id": section.id,
                "title": section.title,
                "findings": section.findings,
            })).collect::<Vec<_>>(),
            "hosts": self.hosts.iter().map(|host| json!({
                "host": host.host,
                "registered": host.registered,
                "standing": self.host_standing(host),
                "findings": host.findings,
            })).collect::<Vec<_>>(),
            "summary": summary,
        })
    }
}

/// What `doctor --resolve` says about one name: the addresses the system
/// resolver gives it, and what the privacy floor makes of them. The lookup is
/// made only when asked for, so `doctor` alone stays off the network. Two
/// ranges are named with their remedy: `198.18.0.0/15`, which a fake-IP
/// proxy answers every name with, and `100.64.0.0/10`, where Tailscale's
/// MagicDNS puts tailnet names. A proxy answering with fake addresses is the
/// operator's to put right; a name that is private is a fact the policy
/// decides on; a name that does not resolve says nothing about the floor.
fn resolution_finding(
    name: &str,
    resolve: &dyn Fn(&str) -> Result<Vec<SocketAddr>, String>,
) -> Finding {
    use commonmeasure_types::address;
    let url = if name.contains("://") {
        name.to_owned()
    } else {
        format!("https://{name}/")
    };
    let host = commonmeasure_harness::grounding::host_of(&url);
    let addresses = match resolve(&url) {
        Ok(addresses) => addresses,
        Err(error) => {
            return Finding::unknown(format!("resolution: {host} did not resolve: {error}"));
        }
    };
    let listed = addresses
        .iter()
        .map(|address| address.ip().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let ips = || addresses.iter().map(SocketAddr::ip);
    let (standing, finding) = if ips().any(address::is_fake_ip_range) {
        (
            Standing::Attention,
            "in 198.18.0.0/15, the range fake-IP proxies (Clash, Surge, sing-box and similar) \
             answer names with: while one does, a fetch by name is refused unless \
             \"allow_private_hosts\" is set or its prefix is named in \"record_internal_prefixes\", \
             and always on a hosted edge. Set the proxy to return real addresses to this machine \
             (Surge always-real-ip, Clash fake-ip-filter, a sing-box DNS rule)",
        )
    } else if ips().any(address::is_shared_range) {
        (
            Standing::Note,
            "in 100.64.0.0/10, shared address space used by Tailscale and carrier-grade NAT: a \
             fetch is refused unless its prefix is named in \"record_internal_prefixes\" or \
             \"allow_private_hosts\" is set, and always on a hosted edge",
        )
    } else if ips().any(address::is_private_ip) {
        (
            Standing::Note,
            "a local or private address: a fetch is refused unless its prefix is named in \
             \"record_internal_prefixes\" or \"allow_private_hosts\" is set, and always on a \
             hosted edge",
        )
    } else {
        (Standing::Ok, "public")
    };
    Finding {
        standing,
        text: format!("resolution: {host} resolves to {listed}, {finding}"),
    }
}

/// [`resolution_finding`]'s words alone.
#[cfg(test)]
fn resolution_line(
    name: &str,
    resolve: &dyn Fn(&str) -> Result<Vec<SocketAddr>, String>,
) -> String {
    resolution_finding(name, resolve).text
}

/// What `doctor` says about relaying without a person. Three things stop it,
/// and each stops it on its own: no receiver in `relay.json`, the
/// `relay/manual` marker the operator writes to review each run, and no
/// carrier. Three carriers relay by themselves: the `SessionEnd` hook of a
/// Claude Code registration (`session_end`), and a running hosted service's
/// or background relay's interval, each of which relays every session in the
/// home whatever its host. The two intervals count only while their process
/// holds its lock on the home, so a configured but stopped one reads as off.
/// The line names the first stop that applies, or every carrier in force,
/// and where the marker applies it names what else the marker does, because
/// a licence demanding usage reporting is refused while automatic delivery is
/// off (owner decision, 22 September 2026).
///
/// No receiver and the marker are the operator's own choices, so they are
/// notes; a receiver with nothing to carry to it needs the operator; a lock
/// that cannot be read is unknown.
fn automatic_relay_finding(home: &Path, session_end: bool) -> Finding {
    match commonmeasure_relay::config::RelayConfig::load(home) {
        Err(error) => return Finding::unknown(format!("automatic relay: off, {error}")),
        Ok(None) => return Finding::note("automatic relay: off, no receiver is configured"),
        Ok(Some(_)) => {}
    }
    carriers_finding(home, session_end)
}

/// [`automatic_relay_finding`]'s words alone.
#[cfg(test)]
fn automatic_relay_line(home: &Path, session_end: bool) -> String {
    automatic_relay_finding(home, session_end).text
}

/// [`automatic_relay_finding`] after the receiver: the stops and carriers.
fn carriers_finding(home: &Path, session_end: bool) -> Finding {
    use commonmeasure_harness::delivery::LockState;
    if let Some(reason) = commonmeasure_harness::delivery::withheld_reason(home) {
        return Finding::note(format!(
            "automatic relay: off, {reason}; a source whose licence demands usage reporting is \
             refused while the marker is there"
        ));
    }
    let service = hosted::ServiceConfig::read(home).ok().flatten();
    let service_state = service
        .as_ref()
        .map(|_| commonmeasure_harness::delivery::service_state(home));
    let loop_state = commonmeasure_harness::delivery::relay_loop_state(home);
    let mut intervals = Vec::new();
    // A lock this user cannot open says nothing either way, so the line does
    // not call that carrier stopped; sessions still treat it as not running
    // (`SessionDelivery::withheld_reason`).
    let mut unknown = Vec::new();
    match (&service, &service_state) {
        (Some(config), Some(LockState::Running)) => intervals.push(format!(
            "on the hosted service's interval (every {}s)",
            config.interval_seconds
        )),
        (_, Some(LockState::Unknown(reason))) => unknown.push(format!(
            "whether the hosted service relays this home cannot be read ({reason})"
        )),
        _ => {}
    }
    match &loop_state {
        LockState::Running => intervals.push(format!(
            "on the background relay's interval{}",
            relay_loop::Holder::read(home)
                .map(|holder| format!(" (every {}s)", holder.every_seconds))
                .unwrap_or_default()
        )),
        LockState::Unknown(reason) => unknown.push(format!(
            "whether a background relay holds this home cannot be read ({reason})"
        )),
        LockState::NotRunning => {}
    }
    let hook = "at each Claude Code session end (its SessionEnd hook)";
    let start = format!(
        "start the background relay (`commonmeasure relay --every {}`; `commonmeasure service \
         install relay` on macOS)",
        relay_loop::DEFAULT_EVERY_SECS
    );
    if !intervals.is_empty() {
        let carriers = session_end
            .then(|| hook.to_owned())
            .into_iter()
            .chain(intervals.iter().cloned())
            .collect::<Vec<_>>()
            .join(", and ");
        return Finding::ok(format!(
            "automatic relay: {carriers}, {} {}",
            if intervals.len() == 1 {
                "which relays"
            } else {
                "each of which relays"
            },
            "every session in this home whatever its host"
        ));
    }
    match (session_end, unknown.is_empty()) {
        (true, true) => Finding::ok(format!(
            "automatic relay: {hook}; no other local host sends the event, so with them {start} \
             or run `commonmeasure relay`, and a source whose licence demands usage reporting \
             is refused there"
        )),
        (true, false) => Finding::unknown(format!(
            "automatic relay: {hook}; {}, so with other local hosts {start} or run \
             `commonmeasure relay`, and a source whose licence demands usage reporting is \
             refused there",
            unknown.join("; ")
        )),
        (false, false) => Finding::unknown(format!(
            "automatic relay: unknown, no Claude Code registration sends SessionEnd and {}; \
             sessions here are treated as having no automatic delivery, so {start}, run \
             `commonmeasure relay`, or `commonmeasure install claude`",
            unknown.join(" and ")
        )),
        (false, true) => Finding::attention(format!(
            "automatic relay: off, no Claude Code registration sends SessionEnd{}; {start}, run \
             `commonmeasure relay`, or `commonmeasure install claude`",
            if service.is_some() {
                " and the hosted service is configured but not running"
            } else {
                ""
            }
        )),
    }
}

/// What `doctor` says about managed policy: nothing on a local edge, the
/// revision in force and its expiry on a managed one, and since when it has
/// been stale where the hub has not renewed it. A managed edge with no
/// revision activated yet, or one gone stale, needs the operator; a
/// management state that cannot be read is unknown.
fn managed_policy_finding(home: &Path) -> Option<Finding> {
    let management = commonmeasure_harness::managed::management(home, chrono::Utc::now());
    match management.mode.as_str() {
        "local" => None,
        "managed" => Some(
            match (management.applied_revision, management.applied_expires_at) {
                (Some(revision), Some(expires_at)) => {
                    let stale = crate::stale_note(&json!(management.stale_since));
                    let text = format!(
                        "managed policy: revision {revision} in force, expires {expires_at}{stale}"
                    );
                    if stale.is_empty() {
                        Finding::ok(text)
                    } else {
                        Finding::attention(text)
                    }
                }
                _ => Finding::attention(format!(
                    "managed policy: no desired revision activated yet; last sync {}",
                    management.desired["outcome"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| "none".to_owned())
                )),
            },
        ),
        _ => Some(Finding::unknown(format!(
            "managed policy: unavailable ({})",
            inspect::text(&management.desired["unavailable"])
        ))),
    }
}

#[cfg(test)]
mod tests {
    use commonmeasure_types::Standing;

    fn resolved_line(name: &str, answer: &str) -> String {
        let answer: std::net::SocketAddr = answer.parse().expect("an address");
        super::resolution_line(name, &move |_| Ok(vec![answer]))
    }

    /// The standing of each resolution answer: a fake-IP proxy is the
    /// operator's to fix, a private or shared address is the policy's
    /// business, a public one is fine, and no answer decides nothing.
    #[test]
    fn a_resolution_stands_by_what_answered() {
        let standing = |answer: &str| {
            let answer: std::net::SocketAddr = answer.parse().expect("an address");
            super::resolution_finding("www.gov.uk", &move |_| Ok(vec![answer])).standing
        };
        assert_eq!(standing("198.18.0.7:443"), Standing::Attention);
        assert_eq!(standing("100.64.0.7:443"), Standing::Note);
        assert_eq!(standing("10.0.0.7:443"), Standing::Note);
        assert_eq!(standing("151.101.0.144:443"), Standing::Ok);
        assert_eq!(
            super::resolution_finding("nowhere.example", &|_| Err("no answer".to_owned())).standing,
            Standing::Unknown
        );
    }

    /// A receiver nothing carries to needs the operator; no receiver, and
    /// the operator's own marker, are notes; a carrier in force is fine.
    #[test]
    fn the_automatic_relay_finding_stands_by_the_first_stop() {
        let home = tempfile::tempdir().expect("tempdir");
        let home = home.path();
        assert_eq!(
            super::automatic_relay_finding(home, true).standing,
            Standing::Note
        );
        std::fs::write(
            home.join("relay.json"),
            r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
        )
        .unwrap();
        assert_eq!(
            super::automatic_relay_finding(home, false).standing,
            Standing::Attention
        );
        assert_eq!(
            super::automatic_relay_finding(home, true).standing,
            Standing::Ok
        );
        std::fs::create_dir_all(home.join("relay")).unwrap();
        std::fs::write(home.join("relay/manual"), b"").unwrap();
        assert_eq!(
            super::automatic_relay_finding(home, true).standing,
            Standing::Note
        );
    }

    /// `doctor --resolve` names the fake-IP range and the proxy-side remedy,
    /// the tailnet range and how to allow one host, and says public for a
    /// public answer. The resolver is injected: the line is about what the
    /// floor makes of an answer, and a test makes no lookup.
    #[test]
    fn doctor_resolve_names_fake_ip_and_shared_space_answers() {
        let fake = resolved_line("www.gov.uk", "198.18.0.7:443");
        assert!(
            fake.starts_with("resolution: www.gov.uk resolves to 198.18.0.7, in 198.18.0.0/15"),
            "{fake}"
        );
        assert!(fake.contains("fake-IP proxies"), "{fake}");
        assert!(fake.contains("fake-ip-filter"), "{fake}");
        assert!(fake.contains("\"allow_private_hosts\" is set"), "{fake}");
        assert!(fake.contains("\"record_internal_prefixes\""), "{fake}");
        assert!(fake.contains("always on a hosted edge"), "{fake}");
        let tailnet = resolved_line("https://nas.tailnet.example/admin", "100.101.1.2:443");
        assert!(
            tailnet.contains("nas.tailnet.example resolves to 100.101.1.2, in 100.64.0.0/10"),
            "{tailnet}"
        );
        assert!(tailnet.contains("record_internal_prefixes"), "{tailnet}");
        let nat64 = resolved_line("v6.example", "[64:ff9b::a00:5]:443");
        assert!(nat64.contains("a local or private address"), "{nat64}");
        let public = resolved_line("www.gov.uk", "151.101.0.144:443");
        assert!(
            public.ends_with("resolves to 151.101.0.144, public"),
            "{public}"
        );
        let failed = super::resolution_line("nowhere.example", &|_| Err("no answer".to_owned()));
        assert_eq!(
            failed,
            "resolution: nowhere.example did not resolve: no answer"
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
        let hook = crate::service::testing::settle(
            || super::automatic_relay_line(home, true),
            |hook| hook.contains("no other local host sends the event"),
        );
        assert!(
            hook.contains("no other local host sends the event"),
            "{hook}"
        );
    }

    /// The background relay is a carrier for every host while it holds its
    /// lock, beside the hook and the hosted service.
    #[test]
    fn the_automatic_relay_line_counts_a_running_background_relay() {
        let home = tempfile::tempdir().expect("tempdir");
        let home = home.path();
        std::fs::write(
            home.join("relay.json"),
            r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
        )
        .unwrap();
        let lock = std::fs::File::create(home.join("relay-loop.lock")).unwrap();
        lock.lock().expect("held as a running loop holds it");
        std::fs::write(
            home.join("relay-loop.lock"),
            r#"{"pid":7,"every_seconds":300}"#,
        )
        .unwrap();
        assert_eq!(
            super::automatic_relay_line(home, true),
            "automatic relay: at each Claude Code session end (its SessionEnd hook), and on the \
             background relay's interval (every 300s), which relays every session in this home \
             whatever its host"
        );
        std::fs::write(
            home.join("hosted-service.json"),
            r#"{"origin":"https://edge.example","hosts":["chatgpt"],"interval_seconds":60}"#,
        )
        .unwrap();
        let service = std::fs::File::create(home.join("hosted-service.lock")).unwrap();
        service.lock().expect("held");
        let both = super::automatic_relay_line(home, false);
        assert!(
            both.starts_with(
                "automatic relay: on the hosted service's interval (every 60s), and on the \
                 background relay's interval (every 300s), each of which relays"
            ),
            "{both}"
        );
        drop(service);
        drop(lock);
        let off = crate::service::testing::settle(
            || super::automatic_relay_line(home, true),
            |off| off.contains("start the background relay (`commonmeasure relay --every 300`"),
        );
        assert!(
            off.contains("start the background relay (`commonmeasure relay --every 300`"),
            "{off}"
        );
    }
}
