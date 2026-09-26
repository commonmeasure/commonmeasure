//! The report `doctor` prints and the `console` command, driven through
//! the real binary against a home of the test's own: the text form a person
//! reads through a pipe, the JSON a script reads, the colour switch, and
//! the console found (or not) on a real loopback socket.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use serde_json::Value;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the binary with `HOME` pointed at the test's directory and the
/// operator home beside it, and no host override in the environment.
fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("HOME", home)
        .env("COMMONMEASURE_HOME", home.join("commonmeasure"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("COPILOT_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDATA")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .expect("the binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A console started on an OS-chosen port, stopped when dropped.
struct Console {
    child: Child,
    /// `host:port`, as `--listen` takes it.
    listen: String,
}

impl Console {
    fn start(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("COMMONMEASURE_HOME", home.join("commonmeasure"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary starts");
        let output = child.stdout.as_mut().expect("stdout");
        let mut lines = BufReader::new(output).lines();
        let listen = loop {
            let line = lines
                .next()
                .expect("serve announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on http://") {
                break address.trim().trim_end_matches('/').to_owned();
            }
        };
        Self { child, listen }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Through a pipe the report is plain: a heading per section, one finding
/// per line with its mark and no escape codes, the home's findings once
/// rather than under every host, and a closing count.
#[test]
fn the_doctor_report_is_sectioned_marked_and_plain_through_a_pipe() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(home.path(), &["doctor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains('\u{1b}'),
        "no escape codes in a pipe: {text}"
    );
    for heading in [
        "Common Measure doctor",
        "Edge home",
        "Console",
        "Relay",
        "Hosts",
    ] {
        assert!(
            text.contains(&format!("\n{heading}\n")) || text.starts_with(heading),
            "{text}"
        );
    }
    assert!(text.contains(&format!("({VERSION})")), "{text}");
    assert_eq!(
        text.matches("recording: ").count(),
        1,
        "the home's findings are printed once: {text}"
    );
    assert!(text.contains("  ✓ recording: "), "{text}");
    assert!(
        text.contains("  · policy: ") && text.contains("absent; observe mode"),
        "{text}"
    );
    assert!(
        text.contains("  · console: nothing answers on http://"),
        "{text}"
    );
    assert!(text.contains("  · claude-code  not registered"), "{text}");
    assert!(
        text.contains("    · hooks: none of this product in"),
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("0 of 8 hosts registered; nothing needs attention"),
        "{text}"
    );
}

/// `--json` prints the same findings as a document a script can read:
/// the contract, the sections, the hosts, the console and the summary,
/// with the standings as words.
#[test]
fn doctor_json_is_a_document_of_the_same_findings() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(home.path(), &["doctor", "codex", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let document: Value = serde_json::from_str(&stdout(&output)).expect("JSON");
    assert_eq!(document["contract"], "commonmeasure-doctor/v1");
    assert_eq!(document["version"], VERSION);
    assert_eq!(
        document["home"],
        home.path().join("commonmeasure").display().to_string()
    );
    let sections: Vec<&str> = document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| section["id"].as_str().unwrap())
        .collect();
    assert_eq!(sections, ["home", "console", "relay"]);
    let recording = &document["sections"][0]["findings"][0];
    assert_eq!(recording["standing"], "ok");
    assert!(
        recording["text"]
            .as_str()
            .unwrap()
            .starts_with("recording: "),
        "{recording}"
    );
    assert_eq!(document["console"]["answering"], false);
    assert!(
        document["console"]["pages"]["policy"]
            .as_str()
            .unwrap()
            .ends_with("/app/policy"),
        "{}",
        document["console"]
    );
    let hosts = document["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["host"], "codex");
    assert_eq!(hosts[0]["registered"], false);
    // The host asked about and not registered is the answer to act on.
    assert_eq!(hosts[0]["standing"], "attention");
    assert!(
        hosts[0]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["text"]
                .as_str()
                .unwrap()
                .starts_with("mcp: no [mcp_servers.commonmeasure]")),
        "{}",
        hosts[0]
    );
    assert_eq!(document["summary"]["hosts"], 1);
    assert_eq!(document["summary"]["hosts_registered"], 0);
    assert_eq!(document["summary"]["attention"], 1);
    assert_eq!(document["summary"]["standing"], "attention");
    assert_eq!(
        document["summary"]["text"],
        "codex not registered; 1 finding needs attention"
    );
}

/// `--color` decides the escape codes whatever stdout is; the words are
/// the same either way.
#[test]
fn the_colour_switch_adds_and_removes_escape_codes_only() {
    let home = tempfile::tempdir().expect("tempdir");
    let coloured = stdout(&run(home.path(), &["doctor", "pi", "--color", "always"]));
    let plain = stdout(&run(home.path(), &["doctor", "pi", "--color", "never"]));
    assert!(coloured.contains("\u{1b}[1m"), "{coloured}");
    assert!(!plain.contains('\u{1b}'), "{plain}");
    let stripped = strip_ansi(&coloured);
    assert_eq!(stripped, plain);
    // The same switch serves status and credentials.
    let status = stdout(&run(home.path(), &["status", "--color", "always"]));
    assert!(status.contains("\u{1b}["), "{status}");
    assert!(
        strip_ansi(&status).contains("deployment mode   local"),
        "{status}"
    );
    let credentials = stdout(&run(home.path(), &["credentials", "--color", "never"]));
    assert!(!credentials.contains('\u{1b}'), "{credentials}");
    assert!(
        credentials.contains("· exa          unavailable (EXA_API_KEY is not set)"),
        "{credentials}"
    );
}

/// The console command finds a running console by address, reports its
/// version, and names every page under it; `doctor` reports the same
/// console and points at its Policy page.
#[test]
fn the_console_command_finds_a_running_console_and_doctor_points_at_its_policy_page() {
    let home = tempfile::tempdir().expect("tempdir");
    let console = Console::start(home.path());
    let output = run(home.path(), &["console", "--listen", &console.listen]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with(&format!(
            "✓ console: http://{} answers, Common Measure {VERSION}, pid {}",
            console.listen,
            console.child.id()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("policy    http://{}/app/policy", console.listen)),
        "{text}"
    );
    assert!(
        text.contains("open one with `commonmeasure console open <page>`"),
        "{text}"
    );

    let json: Value = serde_json::from_str(&stdout(&run(
        home.path(),
        &["console", "--listen", &console.listen, "--json"],
    )))
    .expect("JSON");
    assert_eq!(json["answering"], true);
    assert_eq!(json["version"], VERSION);
    assert_eq!(json["standing"], "ok");
    assert_eq!(
        json["pages"]["record"],
        format!("http://{}/app/record", console.listen)
    );

    // `doctor` looks at the service's address or the default, not at this
    // ephemeral port, so it is told with the same environment `serve` read:
    // nothing here, so it reports nothing answering there. The pointer to
    // the Policy page is printed only where a console answers, which the
    // console command shows above.
    let doctor = stdout(&run(home.path(), &["doctor", "pi"]));
    assert!(
        doctor.contains("console: nothing answers on http://127.0.0.1:4173"),
        "{doctor}"
    );
    assert!(!doctor.contains("policy page:"), "{doctor}");
}

/// `open` never starts a console: with none answering it refuses and names
/// what does, and nothing is handed to the desktop.
#[test]
fn console_open_refuses_where_no_console_answers() {
    let home = tempfile::tempdir().expect("tempdir");
    // A loopback port nothing listens on, held closed by binding and
    // dropping it.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = free.local_addr().unwrap().to_string();
    drop(free);
    let output = run(
        home.path(),
        &["console", "open", "policy", "--listen", &listen],
    );
    assert!(!output.status.success());
    let reason = stderr(&output);
    assert!(
        reason.contains(&format!("nothing answers on http://{listen}"))
            && reason.contains("nothing to open"),
        "{reason}"
    );
    assert!(
        reason.contains("`commonmeasure serve` starts it"),
        "{reason}"
    );
    let listing = stdout(&run(home.path(), &["console", "--listen", &listen]));
    assert!(
        listing.starts_with("· console: nothing answers on"),
        "{listing}"
    );
}

/// A non-loopback address is refused as `serve` refuses it: the console
/// is loopback and the command does not probe the network.
#[test]
fn the_console_command_refuses_an_address_off_this_machine() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(home.path(), &["console", "--listen", "0.0.0.0:4173"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("reachable from outside this machine"),
        "{}",
        stderr(&output)
    );
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
