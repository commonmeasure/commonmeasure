//! The host registrations, driven through the real binary against a home
//! directory of the test's own: the files the hosts read, written and read
//! back, with everything the operator already had in them kept as it was.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

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
        .output()
        .expect("the binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_at(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("JSON")
}

/// `install claude` writes the four hooks and the MCP server, each naming
/// this binary by absolute path; a foreign hook and every other key survive;
/// `doctor` reads the registration back and runs the binary it names;
/// `uninstall` removes exactly what was written and the evidence stays.
#[test]
fn claude_registration_round_trip_touches_only_this_products_entries() {
    let home = tempfile::tempdir().expect("tempdir");
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = claude.join("settings.json");
    let state = home.path().join(".claude.json");
    std::fs::write(
        &settings,
        r#"{"model": "fable", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/usr/bin/say done"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(&state, r#"{"numStartups": 7}"#).unwrap();
    // Evidence that predates the registration, which nothing here may touch.
    let sessions = home.path().join("commonmeasure/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("s-old.ndjson"), "{}\n").unwrap();

    let output = run(home.path(), &["install", "claude"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("four hooks"), "{text}");
    assert!(
        text.contains("MCP server commonmeasure registered"),
        "{text}"
    );

    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    let written = json_at(&settings);
    assert_eq!(written["model"], "fable", "other settings survive");
    let stop = written["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2, "the foreign Stop hook and ours");
    assert_eq!(stop[0]["hooks"][0]["command"], "/usr/bin/say done");
    assert_eq!(
        stop[1]["hooks"][0]["command"],
        format!("\"{}\" hook stop", binary.display()),
        "the registration names the resolved absolute path"
    );
    assert_eq!(
        written["hooks"]["PostToolUse"][0]["matcher"],
        "WebFetch|WebSearch|mcp__.*"
    );
    for event in ["SessionStart", "UserPromptSubmit"] {
        assert_eq!(written["hooks"][event].as_array().unwrap().len(), 1);
    }
    let state_written = json_at(&state);
    assert_eq!(
        state_written["numStartups"], 7,
        "the host's own state survives"
    );
    assert_eq!(
        state_written["mcpServers"]["commonmeasure"],
        json!({"type": "stdio", "command": binary.to_string_lossy(), "args": ["mcp", "--host", "claude-code"]})
    );

    // A second install replaces rather than duplicates.
    let output = run(home.path(), &["install", "claude"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        json_at(&settings)["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let output = run(home.path(), &["doctor", "claude"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("claude-code  registered"), "{text}");
    assert!(
        text.contains("hooks: SessionStart, PostToolUse, UserPromptSubmit, Stop registered"),
        "{text}"
    );
    assert!(
        text.contains("mcp: server commonmeasure registered"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "binary {}: runs, reports commonmeasure {VERSION}",
            binary.display()
        )),
        "doctor runs the binary the registration names: {text}"
    );
    assert!(
        text.contains("recording:") && text.contains("is writable"),
        "{text}"
    );
    assert!(
        text.contains("policy:") && text.contains("absent; observe mode"),
        "{text}"
    );

    let output = run(home.path(), &["uninstall", "claude"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("4 hook handler(s) removed"), "{text}");
    assert!(text.contains("MCP server commonmeasure removed"), "{text}");
    let after = json_at(&settings);
    assert_eq!(after["model"], "fable");
    assert_eq!(
        after["hooks"]["Stop"],
        json!([{"hooks": [{"type": "command", "command": "/usr/bin/say done"}]}]),
        "the foreign hook is exactly as it was"
    );
    assert!(after["hooks"].get("PostToolUse").is_none());
    let state_after = json_at(&state);
    assert_eq!(state_after, json!({"numStartups": 7}));
    assert!(
        sessions.join("s-old.ndjson").exists(),
        "the evidence outlives the registration"
    );

    let output = run(home.path(), &["doctor", "claude"]);
    let text = stdout(&output);
    assert!(text.contains("claude-code  not registered"), "{text}");
    assert!(text.contains("hooks: none of this product"), "{text}");
}

/// An enabled plugin already carries the hooks; a direct registration
/// beside it would record every crossing twice, so install refuses and
/// writes nothing, and doctor names the plugin.
#[test]
fn an_enabled_plugin_refuses_install_and_is_named_by_doctor() {
    let home = tempfile::tempdir().expect("tempdir");
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(claude.join("plugins")).unwrap();
    std::fs::write(
        claude.join("settings.json"),
        r#"{"enabledPlugins": {"commonmeasure@commonmeasure": true}}"#,
    )
    .unwrap();
    std::fs::write(
        claude.join("plugins/installed_plugins.json"),
        r#"{"version": 2, "plugins": {"commonmeasure@commonmeasure": [{"scope": "user", "installPath": "/home/op/.claude/plugins/cache/commonmeasure/commonmeasure/0.2.0", "version": "0.2.0"}]}}"#,
    )
    .unwrap();

    let output = run(home.path(), &["install", "claude"]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("claude plugin disable commonmeasure@commonmeasure"),
        "{text}"
    );
    assert!(
        !home.path().join(".claude.json").exists(),
        "nothing was written"
    );

    let output = run(home.path(), &["doctor", "claude"]);
    let text = stdout(&output);
    assert!(
        text.contains("plugin commonmeasure@commonmeasure: installed at /home/op/.claude/plugins/cache/commonmeasure/commonmeasure/0.2.0, enabled"),
        "{text}"
    );
    assert!(text.contains("recorded twice"), "{text}");
}

/// A registration whose binary has gone is the state this machine was in:
/// every hook exits without recording and nothing says so. Doctor says so.
#[test]
fn doctor_names_a_registered_binary_that_no_longer_exists() {
    let home = tempfile::tempdir().expect("tempdir");
    let copy = home.path().join("bin/commonmeasure");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_commonmeasure"), &copy).unwrap();
    // The registration names the resolved path, so the report does too: on
    // macOS a temporary directory under `/var` resolves to `/private/var`.
    let copy = copy.canonicalize().unwrap();

    let output = run(
        home.path(),
        &["install", "claude", "--binary", copy.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let output = run(home.path(), &["doctor", "claude"]);
    let text = stdout(&output);
    assert!(
        text.contains(&format!(
            "binary {}: runs, reports commonmeasure {VERSION}",
            copy.display()
        )),
        "{text}"
    );

    std::fs::remove_file(&copy).unwrap();
    let output = run(home.path(), &["doctor", "claude"]);
    assert!(output.status.success(), "the report is the result");
    let text = stdout(&output);
    assert!(
        text.contains(&format!("binary {}: not found", copy.display())),
        "{text}"
    );
    assert!(text.contains("records nothing until"), "{text}");
}

/// The Codex registration is one table in the operator's own file: every
/// other table, key and comment comes back byte for byte, and uninstall
/// restores the original bytes exactly.
#[test]
fn codex_registration_is_one_table_and_leaves_the_rest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let codex = home.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    let config = codex.join("config.toml");
    let original =
        "# operator notes\nmodel = \"gpt-5\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";
    std::fs::write(&config, original).unwrap();

    let output = run(home.path(), &["install", "codex"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("[mcp_servers.commonmeasure] registered"),
        "{text}"
    );
    assert!(text.contains("mediated only"), "{text}");
    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.starts_with(original), "{written}");
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    assert!(
        written.contains(&format!("command = \"{}\"", binary.display())),
        "{written}"
    );
    assert!(
        written.contains("args = [\"mcp\", \"--host\", \"codex\"]"),
        "{written}"
    );

    let output = run(home.path(), &["doctor", "codex"]);
    let text = stdout(&output);
    assert!(text.contains("codex        registered"), "{text}");
    assert!(
        text.contains(&format!(
            "binary {}: runs, reports commonmeasure {VERSION}",
            binary.display()
        )),
        "{text}"
    );
    assert!(text.contains("hooks: none by design"), "{text}");

    let output = run(home.path(), &["uninstall", "codex"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    let text = stdout(&run(home.path(), &["doctor", "codex"]));
    assert!(text.contains("codex        not registered"), "{text}");
}

/// Pi has no MCP client, so the registration is an extension that is the
/// client: written under the agent directory naming the binary, read back
/// by doctor, removed by uninstall with the directory it made.
#[test]
fn pi_registration_writes_the_extension_and_removes_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let extension = home
        .path()
        .join(".pi/agent/extensions/commonmeasure/index.ts");
    let output = run(home.path(), &["install", "pi"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("pi: extension written at"), "{text}");
    assert!(text.contains("mediated only"), "{text}");
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    let written = std::fs::read_to_string(&extension).expect("the extension exists");
    assert!(
        written.contains(&format!("const BINARY = \"{}\";", binary.display())),
        "{written}"
    );
    assert!(written.contains("pi.registerTool("), "{written}");
    assert!(
        !written.contains("__COMMONMEASURE_BINARY__"),
        "the marker is substituted"
    );

    let text = stdout(&run(home.path(), &["doctor", "pi"]));
    assert!(text.contains("pi           registered"), "{text}");
    assert!(
        text.contains(&format!(
            "extension: {} registered, naming {}",
            extension.display(),
            binary.display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "binary {}: runs, reports commonmeasure {VERSION}",
            binary.display()
        )),
        "{text}"
    );
    assert!(text.contains("hooks: none by design"), "{text}");

    let output = run(home.path(), &["uninstall", "pi"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!extension.exists());
    assert!(
        !extension.parent().unwrap().exists(),
        "the directory the install made is removed with it"
    );
    assert!(
        home.path().join(".pi/agent/extensions").exists(),
        "the extensions directory itself is Pi's and stays"
    );
    let text = stdout(&run(home.path(), &["doctor", "pi"]));
    assert!(text.contains("pi           not registered"), "{text}");
    assert!(text.contains("extension: none at"), "{text}");
    let text = stdout(&run(home.path(), &["uninstall", "pi"]));
    assert!(text.contains("no extension at"), "{text}");
}

/// The agent directory override Pi honours is honoured here too.
#[test]
fn pi_registration_honours_the_agent_directory_override() {
    let home = tempfile::tempdir().expect("tempdir");
    let agent = home.path().join("elsewhere/agent");
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["install", "pi"])
        .env("HOME", home.path())
        .env("COMMONMEASURE_HOME", home.path().join("commonmeasure"))
        .env("PI_CODING_AGENT_DIR", &agent)
        .output()
        .expect("the binary runs");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        agent.join("extensions/commonmeasure/index.ts").exists(),
        "written under PI_CODING_AGENT_DIR"
    );
    assert!(!home.path().join(".pi").exists());
}

/// The state file is written first and restored when the settings write
/// fails, so a half-registration (a server with no hooks, or hooks with no
/// server) never remains.
#[cfg(unix)]
#[test]
fn a_failed_settings_write_restores_the_state_file() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().expect("tempdir");
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("settings.json"), "{}").unwrap();
    let state = home.path().join(".claude.json");
    let before = r#"{"numStartups": 7}"#;
    std::fs::write(&state, before).unwrap();
    // The settings file itself is writable, but its directory refuses the
    // temporary neighbour the atomic write needs.
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o555)).unwrap();

    let output = run(home.path(), &["install", "claude"]);
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("was restored, so nothing is registered"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(&state).unwrap(),
        before,
        "the state file is exactly as it was"
    );
    assert_eq!(
        std::fs::read_to_string(claude.join("settings.json")).unwrap(),
        "{}"
    );
}

/// The state this machine was found in: a plugin whose marketplace
/// directory has moved loads nothing and none of its hooks fires, and the
/// session says nothing. Doctor reads the recorded directories and says so;
/// an install path that has gone is reported the same way.
#[test]
fn doctor_names_a_plugin_whose_directories_have_gone() {
    let home = tempfile::tempdir().expect("tempdir");
    let plugins = home.path().join(".claude/plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let present = home.path().join("cache/commonmeasure/commonmeasure/0.2.0");
    std::fs::create_dir_all(&present).unwrap();
    std::fs::write(
        home.path().join(".claude/settings.json"),
        r#"{"enabledPlugins": {"commonmeasure@commonmeasure": true}}"#,
    )
    .unwrap();
    std::fs::write(
        plugins.join("installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"commonmeasure@commonmeasure": [{{"scope": "user", "installPath": "{}", "version": "0.2.0"}}]}}}}"#,
            present.display()
        ),
    )
    .unwrap();
    std::fs::write(
        plugins.join("known_marketplaces.json"),
        r#"{"commonmeasure": {"source": {"source": "directory", "path": "/var/home/op/contextops/contextops"}, "installLocation": "/var/home/op/contextops/contextops"}}"#,
    )
    .unwrap();

    let text = stdout(&run(home.path(), &["doctor", "claude"]));
    assert!(
        text.contains("its marketplace commonmeasure is recorded at /var/home/op/contextops/contextops, which does not exist"),
        "{text}"
    );
    assert!(text.contains("failed to load"), "{text}");
    assert!(
        !text.contains("does not exist, so the plugin loads nothing"),
        "the install path is present: {text}"
    );

    std::fs::remove_dir_all(&present).unwrap();
    let text = stdout(&run(home.path(), &["doctor", "claude"]));
    assert!(
        text.contains(&format!(
            "{} does not exist, so the plugin loads nothing",
            present.display()
        )),
        "{text}"
    );
}

/// A host name the binary does not know is refused by name; a home with no
/// registration at all reports every host as not registered and exits zero.
#[test]
fn an_unknown_host_is_refused_and_an_empty_home_reports_nothing_registered() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run(home.path(), &["install", "cursor"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown host \"cursor\""));

    let output = run(home.path(), &["doctor"]);
    assert!(output.status.success());
    let text = stdout(&output);
    for host in [
        "claude-code  not registered",
        "codex        not registered",
        "pi           not registered",
    ] {
        assert!(text.contains(host), "{text}");
    }
    assert!(text.contains(&format!("({VERSION})")), "{text}");
}
