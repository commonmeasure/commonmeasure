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
        .env_remove("COPILOT_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDATA")
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
        format!("\"{}\" hook stop --host claude-code", binary.display()),
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
    let plugin_dir = home.path().join("plugin-files");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        claude.join("plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"commonmeasure@commonmeasure": [{{"scope": "user", "installPath": "{}", "version": "0.3.0"}}]}}}}"#,
            plugin_dir.display()
        ),
    )
    .unwrap();

    // The plugin is enabled and its files are in place, so it loads: a
    // direct install is refused, and doctor reports the plugin route as the
    // registration with nothing to warn about, because there is no direct
    // registration beside it.
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
        text.contains(&format!(
            "plugin commonmeasure@commonmeasure: installed at {}, enabled",
            plugin_dir.display()
        )),
        "{text}"
    );
    assert!(text.contains("claude-code  registered"), "{text}");
    assert!(text.contains("this is the registration"), "{text}");
    assert!(!text.contains("recorded twice"), "{text}");

    // With the plugin's files gone it loads nothing: doctor says so and
    // does not count it, and the same predicate lets a direct install
    // proceed.
    std::fs::remove_dir_all(&plugin_dir).unwrap();
    let output = run(home.path(), &["doctor", "claude"]);
    let text = stdout(&output);
    assert!(
        text.contains("does not exist, so the plugin loads nothing"),
        "{text}"
    );
    assert!(text.contains("claude-code  not registered"), "{text}");
    let output = run(home.path(), &["install", "claude"]);
    assert!(output.status.success(), "{}", stderr(&output));
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
        text.contains("default_tools_approval_mode = \"approve\""),
        "install writes the approval mode Codex needs: {text}"
    );
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
        r#"{"commonmeasure": {"source": {"source": "directory", "path": "/var/home/op/code/commonmeasure"}, "installLocation": "/var/home/op/code/commonmeasure"}}"#,
    )
    .unwrap();

    let text = stdout(&run(home.path(), &["doctor", "claude"]));
    assert!(
        text.contains(
            "its marketplace is recorded at /var/home/op/code/commonmeasure, which does not exist"
        ),
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
    let output = run(home.path(), &["install", "windsurf"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown host \"windsurf\""));

    let output = run(home.path(), &["doctor"]);
    assert!(output.status.success());
    let text = stdout(&output);
    for host in [
        "claude-code  not registered",
        "codex        not registered",
        "pi           not registered",
        "claude-desktop not registered",
        "cursor       not registered",
        "copilot-cli  not registered",
        "vscode       not registered",
        "chrome       not registered",
    ] {
        assert!(text.contains(host), "{text}");
    }
    assert!(text.contains(&format!("({VERSION})")), "{text}");
}

/// Where the binary keeps Claude Desktop's file for a `HOME` on this
/// platform, mirroring `HostPaths::from_environment`.
fn claude_desktop_config(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Claude/claude_desktop_config.json")
    } else if cfg!(windows) {
        home.join("AppData/Roaming/Claude/claude_desktop_config.json")
    } else {
        home.join(".config/Claude/claude_desktop_config.json")
    }
}

/// `install claude-desktop` adds one `mcpServers` key to the application's
/// own file and keeps every other key; the written bytes are exactly the
/// file's other keys plus ours, and `uninstall` gives the original bytes
/// back. `doctor` reads the key and says the host has no hooks.
#[test]
fn claude_desktop_registration_is_one_key_and_leaves_the_rest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let config = claude_desktop_config(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // The file as this product's writer serialises it: the operator's keys
    // in their order, two spaces, a trailing newline; ours is appended.
    let original = "{\n  \"coworkUserFilesPath\": \"/home/op/Claude\",\n  \"preferences\": {\n    \"sidebarMode\": \"epitaxy\"\n  }\n}\n";
    std::fs::write(&config, original).unwrap();
    let binary = env!("CARGO_BIN_EXE_commonmeasure");

    let output = run(home.path(), &["install", "claude-desktop"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("claude-desktop: MCP server commonmeasure registered in"),
        "{text}"
    );
    assert!(
        text.contains("each that makes a call leaves its own session"),
        "{text}"
    );
    let expected = format!(
        "{{\n  \"coworkUserFilesPath\": \"/home/op/Claude\",\n  \"preferences\": {{\n    \"sidebarMode\": \"epitaxy\"\n  }},\n  \"mcpServers\": {{\n    \"commonmeasure\": {{\n      \"command\": \"{binary}\",\n      \"args\": [\n        \"mcp\",\n        \"--host\",\n        \"claude-desktop\"\n      ]\n    }}\n  }}\n}}\n"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), expected);

    let output = run(home.path(), &["doctor", "claude-desktop"]);
    let text = stdout(&output);
    assert!(text.contains("claude-desktop registered"), "{text}");
    assert!(text.contains(&format!("command {binary}")), "{text}");
    assert!(
        text.contains("hooks: none; Claude Desktop has no hook surface"),
        "{text}"
    );
    assert!(text.contains("servers: two per launch"), "{text}");

    // Uninstall removes the entry and nothing else: the operator's keys are
    // back as they were and the emptied `mcpServers` object stays, so the
    // round trip is content-exact and the file's bytes differ from the
    // original only by that empty object.
    let output = run(home.path(), &["uninstall", "claude-desktop"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let after = json_at(&config);
    assert_eq!(after["coworkUserFilesPath"], "/home/op/Claude");
    assert_eq!(after["preferences"]["sidebarMode"], "epitaxy");
    assert_eq!(after["mcpServers"], json!({}), "the emptied object stays");
    let output = run(home.path(), &["doctor", "claude-desktop"]);
    assert!(
        stdout(&output).contains("claude-desktop not registered"),
        "{}",
        stdout(&output)
    );

    // A file that already carried an empty `mcpServers` object, as the
    // application writes when its last server is removed, comes back byte
    // for byte, because this writer's format is the one the file was in.
    let empty_servers = "{\n  \"mcpServers\": {}\n}\n";
    std::fs::write(&config, empty_servers).unwrap();
    assert!(
        run(home.path(), &["install", "claude-desktop"])
            .status
            .success()
    );
    assert!(
        run(home.path(), &["uninstall", "claude-desktop"])
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), empty_servers);

    // A file that did not exist is created by install and left by
    // uninstall, holding the emptied object: nothing is ever deleted.
    std::fs::remove_file(&config).unwrap();
    assert!(
        run(home.path(), &["install", "claude-desktop"])
            .status
            .success()
    );
    assert!(
        run(home.path(), &["uninstall", "claude-desktop"])
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), empty_servers);
}

/// `install cursor` writes the server into `~/.cursor/mcp.json` and four
/// hooks into `~/.cursor/hooks.json`, each command naming the binary and
/// `--host cursor`; a foreign server and a foreign hook survive byte for
/// byte, and `uninstall` gives both original files back.
#[test]
fn cursor_registration_writes_the_server_and_four_hooks_and_leaves_the_rest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let cursor = home.path().join(".cursor");
    std::fs::create_dir_all(&cursor).unwrap();
    let mcp_original = "{\n  \"mcpServers\": {\n    \"linear\": {\n      \"url\": \"https://mcp.linear.app/mcp\"\n    }\n  }\n}\n";
    let hooks_original = "{\n  \"hooks\": {\n    \"postToolUse\": [\n      {\n        \"command\": \"./hooks/audit.sh\"\n      }\n    ]\n  },\n  \"version\": 1\n}\n";
    std::fs::write(cursor.join("mcp.json"), mcp_original).unwrap();
    std::fs::write(cursor.join("hooks.json"), hooks_original).unwrap();
    let binary = env!("CARGO_BIN_EXE_commonmeasure");

    let output = run(home.path(), &["install", "cursor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("cursor: MCP server commonmeasure registered in"),
        "{text}"
    );
    assert!(
        text.contains("four hooks (sessionStart, postToolUse, beforeSubmitPrompt, stop)"),
        "{text}"
    );

    let mcp = json_at(&cursor.join("mcp.json"));
    assert_eq!(
        mcp["mcpServers"]["linear"]["url"],
        "https://mcp.linear.app/mcp"
    );
    assert_eq!(
        mcp["mcpServers"]["commonmeasure"],
        json!({"type": "stdio", "command": binary, "args": ["mcp", "--host", "cursor"]})
    );
    let expected_mcp = format!(
        "{{\n  \"mcpServers\": {{\n    \"linear\": {{\n      \"url\": \"https://mcp.linear.app/mcp\"\n    }},\n    \"commonmeasure\": {{\n      \"type\": \"stdio\",\n      \"command\": \"{binary}\",\n      \"args\": [\n        \"mcp\",\n        \"--host\",\n        \"cursor\"\n      ]\n    }}\n  }}\n}}\n"
    );
    assert_eq!(
        std::fs::read_to_string(cursor.join("mcp.json")).unwrap(),
        expected_mcp
    );
    let hooks = json_at(&cursor.join("hooks.json"));
    assert_eq!(hooks["version"], 1);
    for (event, argument) in [
        ("sessionStart", "session-start"),
        ("postToolUse", "post-tool-use"),
        ("beforeSubmitPrompt", "user-prompt-submit"),
        ("stop", "stop"),
    ] {
        let entries = hooks["hooks"][event].as_array().unwrap();
        let ours: Vec<&Value> = entries
            .iter()
            .filter(|entry| entry["command"].as_str().unwrap().contains("commonmeasure"))
            .collect();
        assert_eq!(ours.len(), 1, "{event}");
        assert_eq!(
            ours[0]["command"],
            format!("\"{binary}\" hook {argument} --host cursor")
        );
    }
    assert_eq!(
        hooks["hooks"]["postToolUse"][0]["command"], "./hooks/audit.sh",
        "the foreign hook stands first, untouched"
    );

    let output = run(home.path(), &["doctor", "cursor"]);
    let text = stdout(&output);
    assert!(text.contains("cursor       registered"), "{text}");
    assert!(
        text.contains("hooks: sessionStart, postToolUse, beforeSubmitPrompt, stop registered in"),
        "{text}"
    );
    assert!(
        text.contains("mcp: server commonmeasure registered in"),
        "{text}"
    );

    // Both originals already held the objects install writes into, in this
    // writer's format, so the round trip is byte for byte.
    let output = run(home.path(), &["uninstall", "cursor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(cursor.join("mcp.json")).unwrap(),
        mcp_original
    );
    assert_eq!(
        std::fs::read_to_string(cursor.join("hooks.json")).unwrap(),
        hooks_original
    );
    assert!(
        stdout(&run(home.path(), &["doctor", "cursor"])).contains("cursor       not registered")
    );

    // Cursor's own empty files survive a round trip unchanged: an
    // `mcpServers` object with no server, and a hooks file with only its
    // version. Nothing is deleted; an object install added is left empty.
    let empty_servers = "{\n  \"mcpServers\": {}\n}\n";
    let version_only = "{\n  \"version\": 1\n}\n";
    std::fs::write(cursor.join("mcp.json"), empty_servers).unwrap();
    std::fs::write(cursor.join("hooks.json"), version_only).unwrap();
    assert!(run(home.path(), &["install", "cursor"]).status.success());
    assert!(run(home.path(), &["uninstall", "cursor"]).status.success());
    assert_eq!(
        std::fs::read_to_string(cursor.join("mcp.json")).unwrap(),
        empty_servers
    );
    assert_eq!(
        std::fs::read_to_string(cursor.join("hooks.json")).unwrap(),
        "{\n  \"version\": 1,\n  \"hooks\": {}\n}\n",
        "the hooks object install added stays, empty"
    );

    // Files that did not exist are created by install and left by
    // uninstall.
    std::fs::remove_file(cursor.join("mcp.json")).unwrap();
    std::fs::remove_file(cursor.join("hooks.json")).unwrap();
    assert!(run(home.path(), &["install", "cursor"]).status.success());
    assert!(run(home.path(), &["uninstall", "cursor"]).status.success());
    assert_eq!(
        std::fs::read_to_string(cursor.join("mcp.json")).unwrap(),
        empty_servers
    );
    assert_eq!(
        std::fs::read_to_string(cursor.join("hooks.json")).unwrap(),
        "{\n  \"version\": 1,\n  \"hooks\": {}\n}\n"
    );
}

/// One Copilot CLI hook entry as this writer serialises it inside the
/// hooks file, at the indentation of an event array's element.
fn copilot_hook_entry(binary: &str, argument: &str, matcher: Option<&str>) -> String {
    let matcher = matcher
        .map(|matcher| format!("        \"matcher\": \"{matcher}\",\n"))
        .unwrap_or_default();
    format!(
        "      {{\n        \"type\": \"command\",\n{matcher}        \"exec\": \"{binary}\",\n        \"args\": [\n          \"hook\",\n          \"{argument}\",\n          \"--host\",\n          \"copilot-cli\"\n        ]\n      }}"
    )
}

/// `install copilot` adds one `mcpServers` entry to the CLI's user file
/// beside the operator's server and writes four hooks, under the CLI's
/// camelCase event names, into a hook file of this product's own. The
/// written bytes are exact; `uninstall` gives the operator's file back byte
/// for byte and deletes the hook file, leaving the CLI's hooks directory.
#[test]
fn copilot_registration_writes_the_server_and_a_hook_file_and_leaves_the_rest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let copilot = home.path().join(".copilot");
    std::fs::create_dir_all(&copilot).unwrap();
    let mcp = copilot.join("mcp-config.json");
    let hooks = copilot.join("hooks/commonmeasure.json");
    let mcp_original = "{\n  \"mcpServers\": {\n    \"playwright\": {\n      \"type\": \"local\",\n      \"command\": \"npx\",\n      \"args\": [\n        \"@playwright/mcp@latest\"\n      ],\n      \"tools\": [\n        \"*\"\n      ]\n    }\n  }\n}\n";
    std::fs::write(&mcp, mcp_original).unwrap();
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    let binary = binary.to_str().unwrap();

    let output = run(home.path(), &["install", "copilot"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("copilot-cli: MCP server commonmeasure registered in"),
        "{text}"
    );
    assert!(
        text.contains("four hooks (sessionStart, postToolUse, userPromptSubmitted, agentStop)"),
        "{text}"
    );
    let expected_mcp = format!(
        "{{\n  \"mcpServers\": {{\n    \"playwright\": {{\n      \"type\": \"local\",\n      \"command\": \"npx\",\n      \"args\": [\n        \"@playwright/mcp@latest\"\n      ],\n      \"tools\": [\n        \"*\"\n      ]\n    }},\n    \"commonmeasure\": {{\n      \"type\": \"local\",\n      \"command\": \"{binary}\",\n      \"args\": [\n        \"mcp\",\n        \"--host\",\n        \"copilot-cli\"\n      ],\n      \"tools\": [\n        \"*\"\n      ]\n    }}\n  }}\n}}\n"
    );
    assert_eq!(std::fs::read_to_string(&mcp).unwrap(), expected_mcp);
    let expected_hooks = format!(
        "{{\n  \"version\": 1,\n  \"hooks\": {{\n    \"sessionStart\": [\n{}\n    ],\n    \"postToolUse\": [\n{}\n    ],\n    \"userPromptSubmitted\": [\n{}\n    ],\n    \"agentStop\": [\n{}\n    ]\n  }}\n}}\n",
        copilot_hook_entry(binary, "session-start", None),
        copilot_hook_entry(binary, "post-tool-use", Some("web_fetch|web_search")),
        copilot_hook_entry(binary, "user-prompt-submit", None),
        copilot_hook_entry(binary, "stop", None),
    );
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), expected_hooks);

    // A second install replaces rather than duplicates.
    assert!(run(home.path(), &["install", "copilot"]).status.success());
    assert_eq!(std::fs::read_to_string(&mcp).unwrap(), expected_mcp);
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), expected_hooks);

    let text = stdout(&run(home.path(), &["doctor", "copilot"]));
    assert!(text.contains("copilot-cli  registered"), "{text}");
    assert!(
        text.contains(
            "hooks: sessionStart, postToolUse, userPromptSubmitted, agentStop registered in"
        ),
        "{text}"
    );
    assert!(
        text.contains("mcp: server commonmeasure registered in"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "binary {binary}: runs, reports commonmeasure {VERSION}"
        )),
        "{text}"
    );
    assert!(
        !text.contains("name different binaries"),
        "one binary: {text}"
    );
    assert!(text.contains("GitHub Copilot app"), "{text}");

    let output = run(home.path(), &["uninstall", "copilot"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("4 hook handler(s) removed"), "{text}");
    assert_eq!(std::fs::read_to_string(&mcp).unwrap(), mcp_original);
    assert!(!hooks.exists(), "the hook file was this product's alone");
    assert!(
        copilot.join("hooks").is_dir(),
        "the hooks directory is the CLI's and stays"
    );
    assert!(
        stdout(&run(home.path(), &["doctor", "copilot"])).contains("copilot-cli  not registered")
    );

    // An entry someone else put into the hook file survives uninstall, and
    // the file stays for it.
    assert!(run(home.path(), &["install", "copilot"]).status.success());
    let mut document = json_at(&hooks);
    document["hooks"]["agentStop"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"type": "command", "bash": "./notify.sh"}));
    std::fs::write(&hooks, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    assert!(run(home.path(), &["uninstall", "copilot"]).status.success());
    assert_eq!(
        json_at(&hooks),
        json!({"version": 1, "hooks": {"agentStop": [{"type": "command", "bash": "./notify.sh"}]}})
    );

    // A file that did not exist is created by install and left by
    // uninstall holding the emptied object.
    std::fs::remove_file(&mcp).unwrap();
    assert!(run(home.path(), &["install", "copilot"]).status.success());
    assert!(run(home.path(), &["uninstall", "copilot"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&mcp).unwrap(),
        "{\n  \"mcpServers\": {}\n}\n"
    );
}

/// The configuration directory override the Copilot CLI honours is
/// honoured here too, for both files.
#[test]
fn copilot_registration_honours_the_configuration_directory_override() {
    let home = tempfile::tempdir().expect("tempdir");
    let elsewhere = home.path().join("elsewhere/copilot");
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["install", "copilot"])
        .env("HOME", home.path())
        .env("COMMONMEASURE_HOME", home.path().join("commonmeasure"))
        .env("COPILOT_HOME", &elsewhere)
        .output()
        .expect("the binary runs");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(elsewhere.join("mcp-config.json").exists());
    assert!(elsewhere.join("hooks/commonmeasure.json").exists());
    assert!(!home.path().join(".copilot").exists());
}

/// Where the binary keeps VS Code's user `mcp.json` for a `HOME` on this
/// platform, mirroring `HostPaths::from_environment`.
fn vscode_mcp(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code/User/mcp.json")
    } else if cfg!(windows) {
        home.join("AppData/Roaming/Code/User/mcp.json")
    } else {
        home.join(".config/Code/User/mcp.json")
    }
}

/// `install vscode` adds one `servers` entry to VS Code's user `mcp.json`
/// beside the operator's server and keeps `inputs`; the written bytes are
/// exact and `uninstall` gives the original bytes back. No hook is written
/// anywhere. A file VS Code wrote in its own tab indentation keeps its
/// content, and a file with a comment is refused whole.
#[test]
fn vscode_registration_is_one_server_entry_and_leaves_the_rest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let config = vscode_mcp(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = "{\n  \"servers\": {\n    \"github\": {\n      \"type\": \"http\",\n      \"url\": \"https://api.githubcopilot.com/mcp/\"\n    }\n  },\n  \"inputs\": []\n}\n";
    std::fs::write(&config, original).unwrap();
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    let binary = binary.to_str().unwrap();

    let output = run(home.path(), &["install", "vscode"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("vscode: MCP server commonmeasure registered in"),
        "{text}"
    );
    assert!(text.contains("no hook is registered"), "{text}");
    let expected = format!(
        "{{\n  \"servers\": {{\n    \"github\": {{\n      \"type\": \"http\",\n      \"url\": \"https://api.githubcopilot.com/mcp/\"\n    }},\n    \"commonmeasure\": {{\n      \"type\": \"stdio\",\n      \"command\": \"{binary}\",\n      \"args\": [\n        \"mcp\",\n        \"--host\",\n        \"vscode\"\n      ]\n    }}\n  }},\n  \"inputs\": []\n}}\n"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), expected);
    assert!(
        !home.path().join(".claude").exists() && !home.path().join(".github").exists(),
        "no hook file is written for VS Code"
    );

    let text = stdout(&run(home.path(), &["doctor", "vscode"]));
    assert!(text.contains("vscode       registered"), "{text}");
    assert!(text.contains(&format!("command {binary}")), "{text}");
    assert!(text.contains("hooks: none by decision"), "{text}");
    assert!(!text.contains("names the server too"), "{text}");

    // With the Copilot registration beside it, doctor says the Agent Host
    // can see the server from both files.
    assert!(run(home.path(), &["install", "copilot"]).status.success());
    let text = stdout(&run(home.path(), &["doctor", "vscode"]));
    assert!(text.contains("names the server too"), "{text}");
    assert!(run(home.path(), &["uninstall", "copilot"]).status.success());

    let output = run(home.path(), &["uninstall", "vscode"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    assert!(
        stdout(&run(home.path(), &["doctor", "vscode"])).contains("vscode       not registered")
    );

    // The file as `code --add-mcp` writes it, tab-indented: the content
    // comes back exactly, in this writer's formatting.
    let tabbed = "{\n\t\"servers\": {\n\t\t\"other\": {\n\t\t\t\"command\": \"/usr/bin/other\",\n\t\t\t\"args\": []\n\t\t}\n\t},\n\t\"inputs\": []\n}";
    std::fs::write(&config, tabbed).unwrap();
    assert!(run(home.path(), &["install", "vscode"]).status.success());
    assert!(run(home.path(), &["uninstall", "vscode"]).status.success());
    assert_eq!(
        json_at(&config),
        serde_json::from_str::<Value>(tabbed).unwrap()
    );

    // VS Code accepts comments here; this writer would drop them, so the
    // file is refused and left as it was.
    let commented = "{\n  // the operator's note\n  \"servers\": {}\n}\n";
    std::fs::write(&config, commented).unwrap();
    let output = run(home.path(), &["install", "vscode"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("allows comments"),
        "{}",
        stderr(&output)
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), commented);
}

/// Where a Chromium-family browser keeps its user data for a `HOME` on this
/// platform, mirroring `HostPaths::from_environment`.
fn browser_directory(home: &Path, mac: &str, linux: &str) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(mac)
    } else {
        home.join(".config").join(linux)
    }
}

/// `install chrome` writes one native messaging host manifest, byte for
/// byte, naming this binary and allowing only the Common Measure extension;
/// Brave gets the same file because its directory exists and Chromium gets
/// nothing because its does not. Another host's manifest beside ours is not
/// touched. `doctor` reads the manifest back and runs the binary it names;
/// `uninstall` removes exactly the files it wrote.
#[cfg(unix)]
#[test]
fn chrome_registration_writes_the_native_messaging_manifest_byte_for_byte() {
    let home = tempfile::tempdir().expect("tempdir");
    let chrome = browser_directory(home.path(), "Google/Chrome", "google-chrome");
    let brave = browser_directory(
        home.path(),
        "BraveSoftware/Brave-Browser",
        "BraveSoftware/Brave-Browser",
    );
    let chromium = browser_directory(home.path(), "Chromium", "chromium");
    let foreign = chrome.join("NativeMessagingHosts/com.example.other.json");
    std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    let foreign_bytes = "{\"name\": \"com.example.other\", \"path\": \"/usr/bin/other\"}";
    std::fs::write(&foreign, foreign_bytes).unwrap();
    std::fs::create_dir_all(&brave).unwrap();

    let output = run(home.path(), &["install", "chrome"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Chromium is not set up here"),
        "no directory is created for a browser not in use: {text}"
    );

    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_commonmeasure")).unwrap();
    let expected = format!(
        "{{\n  \"name\": \"ai.commonmeasure.browser\",\n  \"description\": \"Common Measure: records the sources browser AI answers show\",\n  \"path\": {},\n  \"type\": \"stdio\",\n  \"allowed_origins\": [\n    \"chrome-extension://hojjbnoeobjkjklcdhhnncmmojmcneig/\"\n  ]\n}}\n",
        serde_json::to_string(&binary.to_string_lossy()).unwrap()
    );
    let manifest = chrome.join("NativeMessagingHosts/ai.commonmeasure.browser.json");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), expected);
    assert_eq!(
        std::fs::read_to_string(brave.join("NativeMessagingHosts/ai.commonmeasure.browser.json"))
            .unwrap(),
        expected
    );
    assert!(!chromium.exists());
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), foreign_bytes);

    // A second install writes the same bytes again.
    assert!(run(home.path(), &["install", "chrome"]).status.success());
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), expected);

    let output = run(home.path(), &["doctor", "chrome"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("chrome       registered"), "{text}");
    assert!(
        text.contains("native messaging: host ai.commonmeasure.browser registered for Chrome"),
        "{text}"
    );
    assert!(
        text.contains("native messaging: no host ai.commonmeasure.browser for Chromium"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "binary {}: runs, reports commonmeasure {VERSION}",
            binary.display()
        )),
        "{text}"
    );

    // A browser answer surface is recorded under its own name and is not a
    // registration of its own.
    let output = run(home.path(), &["install", "chatgpt-web"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("commonmeasure install chrome"),
        "{}",
        stderr(&output)
    );

    let output = run(home.path(), &["uninstall", "chrome"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!manifest.exists());
    assert!(
        !brave
            .join("NativeMessagingHosts/ai.commonmeasure.browser.json")
            .exists()
    );
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), foreign_bytes);
    assert!(manifest.parent().unwrap().is_dir(), "the directory stays");
    assert!(
        stdout(&run(home.path(), &["doctor", "chrome"])).contains("chrome       not registered")
    );
}
