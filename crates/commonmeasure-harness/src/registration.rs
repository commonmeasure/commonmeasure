//! The host registrations the binary writes, verifies and removes.
//!
//! The binary on `PATH` is the product; each host holds a thin registration
//! naming it. `install` resolves the binary once and writes its absolute
//! path into the host's own registration surface, because a host's hook
//! environment does not share the login shell's `PATH`; `uninstall` removes
//! exactly those entries and nothing else in the file; `doctor` reads the
//! surfaces back and reports what they name, whether that binary runs, and
//! what version it reports. Replacing the file at the registered path
//! changes what the next session runs, with no reinstall.
//!
//! One rule for every host: the registration touches only the entries that
//! name this product. Every other key, table and comment in a host's
//! configuration is the operator's and keeps its value; the two Claude Code
//! JSON files are rewritten whole, so their keys come back in sorted order
//! and their formatting is this writer's, while the Codex TOML keeps its
//! bytes. The evidence in the operator home is never touched by any of this.
//!
//! Surfaces, per host:
//!
//! - Claude Code: the four hooks in the user settings file
//!   (`~/.claude/settings.json`, or `$CLAUDE_CONFIG_DIR/settings.json`) and
//!   the MCP server at user scope in the host's state file
//!   (`~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json`). The state
//!   file is written first; a settings write that then fails restores the
//!   state file, so the two never disagree.
//! - Codex: one `[mcp_servers.commonmeasure]` table in
//!   `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`). Mediated only:
//!   Codex's hooks documentation states that its hosted web search does not
//!   pass through the local tool path and fires no hook, and its shell
//!   reaches the web as command text, so there is nothing an observed
//!   matcher could witness.
//! - Pi: one extension, `extensions/commonmeasure/index.ts` under the agent
//!   directory (`~/.pi/agent`, or `$PI_CODING_AGENT_DIR`). Pi has no MCP
//!   client, so the extension is the client: it spawns `mcp --host pi` from
//!   the binary named in it and registers the server's tools with Pi under
//!   their own names. Mediated only: Pi has no web tool of its own to
//!   observe.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::hook::HostSurface;
use crate::policy::PolicyDocument;

/// The server name every host sees, which is also the plugin's name: a
/// session with either registration spells the tools `context_fetch`,
/// `context_search` and `context_status` under `commonmeasure`.
pub const SERVER_NAME: &str = "commonmeasure";

/// The plugin identities whose enabled state means the plugin's hooks fire.
/// A direct registration beside an enabled plugin records every crossing
/// twice, so `install claude` refuses while one is enabled.
const PLUGIN_PREFIXES: [&str; 2] = ["commonmeasure@", "contextops@"];

/// The observed path's hooks: the host event, the event name the `hook`
/// subcommand takes, and the tool matcher where the event has one.
pub const CLAUDE_HOOKS: [(&str, &str, Option<&str>); 4] = [
    ("SessionStart", "session-start", None),
    (
        "PostToolUse",
        "post-tool-use",
        Some("WebFetch|WebSearch|mcp__.*"),
    ),
    ("UserPromptSubmit", "user-prompt-submit", None),
    ("Stop", "stop", None),
];

/// Where each host keeps its registration, resolved from the environment
/// once so every command in one invocation reads and writes the same files.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub claude_settings: PathBuf,
    pub claude_state: PathBuf,
    pub claude_plugins: PathBuf,
    pub codex_config: PathBuf,
    pub pi_extension: PathBuf,
}

/// The extension `install pi` writes, with the binary's path substituted
/// where the marker stands.
const PI_EXTENSION: &str = include_str!("pi_extension.ts");
const PI_BINARY_MARKER: &str = "__COMMONMEASURE_BINARY__";

impl HostPaths {
    /// `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are the hosts' own overrides and
    /// are honoured as the hosts honour them; otherwise the defaults under
    /// `HOME`.
    pub fn from_environment() -> Result<Self, String> {
        let home = std::env::var("HOME")
            .ok()
            .filter(|home| !home.trim().is_empty())
            .map(PathBuf::from)
            .ok_or("HOME is not set, so no host configuration directory can be resolved")?;
        let claude_dir = std::env::var("CLAUDE_CONFIG_DIR")
            .ok()
            .filter(|dir| !dir.trim().is_empty())
            .map(PathBuf::from);
        let (claude_settings, claude_state, claude_plugins) = match claude_dir {
            Some(dir) => (
                dir.join("settings.json"),
                dir.join(".claude.json"),
                dir.join("plugins"),
            ),
            None => (
                home.join(".claude/settings.json"),
                home.join(".claude.json"),
                home.join(".claude/plugins"),
            ),
        };
        let codex_home = std::env::var("CODEX_HOME")
            .ok()
            .filter(|dir| !dir.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let pi_agent = std::env::var("PI_CODING_AGENT_DIR")
            .ok()
            .filter(|dir| !dir.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi/agent"));
        Ok(Self {
            claude_settings,
            claude_state,
            claude_plugins,
            codex_config: codex_home.join("config.toml"),
            pi_extension: pi_agent.join("extensions/commonmeasure/index.ts"),
        })
    }
}

/// The binary the registration names: absolute, existing, and without the
/// characters a shell-quoted hook command could not carry.
pub fn resolve_binary(requested: Option<&Path>) -> Result<PathBuf, String> {
    let path = match requested {
        Some(path) => path.to_path_buf(),
        None => std::env::current_exe()
            .map_err(|error| format!("cannot resolve the running binary's path: {error}"))?,
    };
    let path = std::fs::canonicalize(&path)
        .map_err(|error| format!("{} cannot be resolved: {error}", path.display()))?;
    let text = path.to_string_lossy();
    if text.contains('"') || text.contains('\n') || text.contains('$') || text.contains('`') {
        return Err(format!(
            "{} contains a character a hook command cannot carry (a double quote, dollar, \
             backtick or newline); place the binary at another path",
            path.display()
        ));
    }
    Ok(path)
}

fn shell_quoted(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy())
}

/// Whether a hook command line is one of this product's: its program is the
/// binary or the plugin's launcher, and its first argument is `hook`.
fn is_our_hook_command(command: &str) -> bool {
    let command = command.trim_start();
    let (program, rest) = if let Some(quoted) = command.strip_prefix('"') {
        match quoted.find('"') {
            Some(end) => (&quoted[..end], &quoted[end + 1..]),
            None => return false,
        }
    } else {
        match command.find(char::is_whitespace) {
            Some(end) => (&command[..end], &command[end..]),
            None => (command, ""),
        }
    };
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let ours = matches!(
        name,
        "commonmeasure" | "commonmeasure.exe" | "commonmeasure-launch"
    );
    ours && rest.split_whitespace().next() == Some("hook")
}

/// The program a hook command names, for the doctor's report.
fn program_of(command: &str) -> String {
    let command = command.trim_start();
    if let Some(quoted) = command.strip_prefix('"') {
        quoted.split('"').next().unwrap_or_default().to_owned()
    } else {
        command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => {
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
            if !value.is_object() {
                return Err(format!(
                    "{} does not hold a JSON object, so nothing is written to it",
                    path.display()
                ));
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Write the whole file through a temporary neighbour and a rename, so a
/// host reading the file mid-write sees the old bytes or the new ones.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!(
        "{}.commonmeasure-tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
    ));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("cannot replace {}: {error}", path.display())
    })
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    text.push('\n');
    write_atomically(path, text.as_bytes())
}

/// Remove this product's handlers from one event's hook entries, keeping
/// every other handler where it stands. Returns how many were removed.
fn strip_our_hooks(entries: &mut Vec<Value>) -> usize {
    let mut removed = 0;
    for entry in entries.iter_mut() {
        if let Some(handlers) = entry["hooks"].as_array_mut() {
            let before = handlers.len();
            handlers
                .retain(|handler| !handler["command"].as_str().is_some_and(is_our_hook_command));
            removed += before - handlers.len();
        }
    }
    entries.retain(|entry| {
        entry["hooks"]
            .as_array()
            .is_none_or(|handlers| !handlers.is_empty())
    });
    removed
}

fn enabled_plugin(settings: &Value) -> Option<String> {
    settings["enabledPlugins"].as_object().and_then(|plugins| {
        plugins
            .iter()
            .find(|(name, enabled)| {
                PLUGIN_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                    && **enabled == Value::Bool(true)
            })
            .map(|(name, _)| name.clone())
    })
}

/// Register the product with one host. Returns one line per surface
/// written, for the command to print.
pub fn install(
    surface: HostSurface,
    binary: &Path,
    paths: &HostPaths,
) -> Result<Vec<String>, String> {
    match surface {
        HostSurface::ClaudeCode => install_claude(binary, paths),
        HostSurface::Codex => install_codex(binary, paths),
        HostSurface::Pi => install_pi(binary, paths),
    }
}

/// Remove the product's registration from one host. Returns one line per
/// surface, saying what was removed or that nothing was registered.
pub fn uninstall(surface: HostSurface, paths: &HostPaths) -> Result<Vec<String>, String> {
    match surface {
        HostSurface::ClaudeCode => uninstall_claude(paths),
        HostSurface::Codex => uninstall_codex(paths),
        HostSurface::Pi => uninstall_pi(paths),
    }
}

/// The binary's path as a JSON string literal, which is also a TypeScript
/// string literal; [`resolve_binary`] has already refused the characters a
/// hook command cannot carry.
fn pi_extension_for(binary: &Path) -> String {
    let literal = serde_json::to_string(&binary.to_string_lossy()).expect("a string serialises");
    PI_EXTENSION.replace(&format!("\"{PI_BINARY_MARKER}\""), &literal)
}

fn install_pi(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    write_atomically(&paths.pi_extension, pi_extension_for(binary).as_bytes())?;
    Ok(vec![
        format!(
            "pi: extension written at {}, naming {}; Pi loads it from its extensions directory \
             at the next session start",
            paths.pi_extension.display(),
            binary.display()
        ),
        "pi: mediated only. Pi has no MCP client and no web tool of its own, so the extension \
         is the client for the mediated tools and nothing is observed"
            .to_owned(),
    ])
}

fn uninstall_pi(paths: &HostPaths) -> Result<Vec<String>, String> {
    match std::fs::remove_file(&paths.pi_extension) {
        Ok(()) => {
            // The directory is this product's own; remove it when nothing
            // else was put there.
            if let Some(directory) = paths.pi_extension.parent() {
                let _ = std::fs::remove_dir(directory);
            }
            Ok(vec![
                format!("pi: extension {} removed", paths.pi_extension.display()),
                "the evidence in the operator home is untouched".to_owned(),
            ])
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![
            format!("pi: no extension at {}", paths.pi_extension.display()),
            "the evidence in the operator home is untouched".to_owned(),
        ]),
        Err(error) => Err(format!(
            "cannot remove {}: {error}",
            paths.pi_extension.display()
        )),
    }
}

/// The binary a written Pi extension names: the one string literal on its
/// `const BINARY` line.
fn pi_extension_binary(text: &str) -> Option<String> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("const BINARY = "))?;
    let literal = line
        .trim_start()
        .strip_prefix("const BINARY = ")?
        .trim_end_matches(';');
    serde_json::from_str::<String>(literal).ok()
}

fn install_claude(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut settings = read_json(&paths.claude_settings)?;
    if let Some(plugin) = enabled_plugin(&settings) {
        return Err(format!(
            "the plugin {plugin} is enabled in {}. A direct registration beside it would \
             record every crossing twice when the plugin loads. Disable it first: claude plugin \
             disable {plugin}. Nothing was written.",
            paths.claude_settings.display()
        ));
    }
    let quoted = shell_quoted(binary);
    let hooks = settings
        .as_object_mut()
        .expect("read_json returns an object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return Err(format!(
            "{} holds a \"hooks\" value that is not an object, so nothing is written to it",
            paths.claude_settings.display()
        ));
    }
    for (event, argument, matcher) in CLAUDE_HOOKS {
        let entries = hooks
            .as_object_mut()
            .expect("checked above")
            .entry(event)
            .or_insert_with(|| json!([]));
        let Some(entries) = entries.as_array_mut() else {
            return Err(format!(
                "{} holds a \"hooks.{event}\" value that is not an array, so nothing is \
                 written to it",
                paths.claude_settings.display()
            ));
        };
        strip_our_hooks(entries);
        let mut entry = json!({
            "hooks": [{"type": "command", "command": format!("{quoted} hook {argument}")}]
        });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        entries.push(entry);
    }
    // The state file (the MCP server) goes first and is restored if the
    // settings write (the hooks) then fails, so the two surfaces never
    // disagree: hooks with no server would record without mediating.
    let previous_state = match std::fs::read(&paths.claude_state) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "cannot read {}: {error}",
                paths.claude_state.display()
            ));
        }
    };
    let mut state = read_json(&paths.claude_state)?;
    let servers = state
        .as_object_mut()
        .expect("read_json returns an object")
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        return Err(format!(
            "{} holds an \"mcpServers\" value that is not an object, so nothing is written to it",
            paths.claude_state.display()
        ));
    }
    servers[SERVER_NAME] = json!({
        "type": "stdio",
        "command": binary.to_string_lossy(),
        "args": ["mcp", "--host", "claude-code"],
    });
    write_json(&paths.claude_state, &state)?;
    if let Err(error) = write_json(&paths.claude_settings, &settings) {
        let restored = match previous_state {
            Some(bytes) => std::fs::write(&paths.claude_state, bytes),
            None => std::fs::remove_file(&paths.claude_state),
        };
        return Err(match restored {
            Ok(()) => format!(
                "{error}; {} was restored, so nothing is registered",
                paths.claude_state.display()
            ),
            Err(restore_error) => format!(
                "{error}; and {} could not be restored ({restore_error}), so the MCP server is \
                 registered with no hooks: run commonmeasure uninstall claude",
                paths.claude_state.display()
            ),
        });
    }

    Ok(vec![
        format!(
            "claude-code: four hooks (SessionStart, PostToolUse, UserPromptSubmit, Stop) \
             registered in {}, each naming {}",
            paths.claude_settings.display(),
            binary.display()
        ),
        format!(
            "claude-code: MCP server {SERVER_NAME} registered at user scope in {}, naming the \
             same binary",
            paths.claude_state.display()
        ),
        "claude-code: a running session picks this up on its next start".to_owned(),
    ])
}

fn uninstall_claude(paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut settings = read_json(&paths.claude_settings)?;
    let mut removed = 0;
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        for (event, _, _) in CLAUDE_HOOKS {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                removed += strip_our_hooks(entries);
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|e| !e.is_empty()));
        if hooks.is_empty() {
            settings.as_object_mut().expect("object").remove("hooks");
        }
    }
    if removed > 0 {
        write_json(&paths.claude_settings, &settings)?;
        lines.push(format!(
            "claude-code: {removed} hook handler(s) removed from {}",
            paths.claude_settings.display()
        ));
    } else {
        lines.push(format!(
            "claude-code: no hook of this product in {}",
            paths.claude_settings.display()
        ));
    }

    let mut state = read_json(&paths.claude_state)?;
    let had_server = state["mcpServers"]
        .as_object_mut()
        .and_then(|servers| servers.remove(SERVER_NAME))
        .is_some();
    if had_server {
        if state["mcpServers"]
            .as_object()
            .is_some_and(|servers| servers.is_empty())
        {
            state.as_object_mut().expect("object").remove("mcpServers");
        }
        write_json(&paths.claude_state, &state)?;
        lines.push(format!(
            "claude-code: MCP server {SERVER_NAME} removed from {}",
            paths.claude_state.display()
        ));
    } else {
        lines.push(format!(
            "claude-code: no MCP server {SERVER_NAME} in {}",
            paths.claude_state.display()
        ));
    }
    lines.push("the evidence in the operator home is untouched".to_owned());
    Ok(lines)
}

fn read_toml(path: &Path) -> Result<toml_edit::DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| format!("{} is not valid TOML: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(toml_edit::DocumentMut::new())
        }
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn install_codex(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut document = read_toml(&paths.codex_config)?;
    let servers = document
        .as_table_mut()
        .entry("mcp_servers")
        .or_insert_with(|| {
            // Implicit: `[mcp_servers]` alone is not written, only the
            // server's own table header.
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            toml_edit::Item::Table(table)
        });
    let Some(servers) = servers.as_table_mut() else {
        return Err(format!(
            "{} holds an mcp_servers value that is not a table, so nothing is written to it",
            paths.codex_config.display()
        ));
    };
    let mut server = toml_edit::Table::new();
    server["command"] = toml_edit::value(binary.to_string_lossy().as_ref());
    let mut args = toml_edit::Array::new();
    for argument in ["mcp", "--host", "codex"] {
        args.push(argument);
    }
    server["args"] = toml_edit::value(args);
    servers[SERVER_NAME] = toml_edit::Item::Table(server);
    write_atomically(&paths.codex_config, document.to_string().as_bytes())?;
    Ok(vec![
        format!(
            "codex: [mcp_servers.{SERVER_NAME}] registered in {}, naming {}",
            paths.codex_config.display(),
            binary.display()
        ),
        "codex: mediated only. Codex's hosted web search fires no hook and its shell reaches \
         the web as command text, so no observed matcher is registered"
            .to_owned(),
    ])
}

fn uninstall_codex(paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut document = read_toml(&paths.codex_config)?;
    let removed = document
        .as_table_mut()
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_mut)
        .map(|servers| servers.remove(SERVER_NAME).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok(vec![
            format!(
                "codex: no [mcp_servers.{SERVER_NAME}] in {}",
                paths.codex_config.display()
            ),
            "the evidence in the operator home is untouched".to_owned(),
        ]);
    }
    if document["mcp_servers"]
        .as_table()
        .is_some_and(toml_edit::Table::is_empty)
    {
        document.as_table_mut().remove("mcp_servers");
    }
    write_atomically(&paths.codex_config, document.to_string().as_bytes())?;
    Ok(vec![
        format!(
            "codex: [mcp_servers.{SERVER_NAME}] removed from {}",
            paths.codex_config.display()
        ),
        "the evidence in the operator home is untouched".to_owned(),
    ])
}

/// What one host's registration says, checked against the machine.
#[derive(Debug, Clone)]
pub struct HostReport {
    pub host: &'static str,
    pub registered: bool,
    pub lines: Vec<String>,
}

/// What a registered binary path does when run: its reported version, or
/// why it could not be run.
fn binary_line(path: &str) -> String {
    let file = Path::new(path);
    if !file.exists() {
        return format!(
            "binary {path}: not found; every session registered against it records nothing \
             until it is restored or the registration is rewritten (commonmeasure install)"
        );
    }
    match std::process::Command::new(file).arg("--version").output() {
        Ok(output) if output.status.success() => format!(
            "binary {path}: runs, reports {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ),
        Ok(output) => format!(
            "binary {path}: exits {} on --version",
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "by signal".to_owned())
        ),
        Err(error) => format!("binary {path}: cannot run: {error}"),
    }
}

/// The operator home's state, the same for every host: whether a session
/// log can be written, and what the policy file says.
fn home_lines(home: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let sessions = home.join("sessions");
    let probe = sessions.join(".doctor-probe");
    let writable = std::fs::create_dir_all(&sessions)
        .and_then(|()| std::fs::write(&probe, b""))
        .and_then(|()| std::fs::remove_file(&probe));
    lines.push(match writable {
        Ok(()) => format!("recording: {} is writable", sessions.display()),
        Err(error) => format!(
            "recording: {} cannot be written ({error}); hooks exit zero and record nothing",
            sessions.display()
        ),
    });
    lines.push(match PolicyDocument::read(home) {
        Ok(document) if !document.declared() => format!(
            "policy: {} absent; observe mode, refusing nothing",
            home.join("policy.json").display()
        ),
        Ok(document) => {
            let resolved = document.resolve(None);
            format!(
                "policy: {} loads; top-level mode {:?}, {} scope(s), {} principal(s)",
                home.join("policy.json").display(),
                resolved.mode(),
                document.scopes().len(),
                document.principals().len()
            )
        }
        Err(error) => format!(
            "policy: cannot load; the mediated tools refuse every crossing until it is fixed: \
             {error}"
        ),
    });
    lines
}

fn plugin_lines(paths: &HostPaths, settings: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let installed = paths.claude_plugins.join("installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(&installed) else {
        return lines;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return lines;
    };
    let Some(plugins) = value["plugins"].as_object() else {
        return lines;
    };
    let marketplaces =
        std::fs::read_to_string(paths.claude_plugins.join("known_marketplaces.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null);
    for (name, installs) in plugins {
        if !PLUGIN_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let enabled = settings["enabledPlugins"][name] == Value::Bool(true);
        let path = installs
            .as_array()
            .and_then(|installs| installs.first())
            .and_then(|install| install["installPath"].as_str());
        lines.push(format!(
            "plugin {name}: installed at {}, {} in {}",
            path.unwrap_or("an unrecorded path"),
            if enabled { "enabled" } else { "disabled" },
            paths.claude_settings.display()
        ));
        if let Some(path) = path
            && !Path::new(path).is_dir()
        {
            lines.push(format!(
                "plugin {name}: {path} does not exist, so the plugin loads nothing and none of \
                 its hooks fires"
            ));
        }
        // The marketplace the plugin was installed from must still exist
        // where Claude Code recorded it: a moved directory makes the host
        // report the plugin as failed to load, and every hook stays silent.
        if let Some(marketplace) = name.split('@').nth(1) {
            let location = marketplaces[marketplace]["installLocation"].as_str();
            if let Some(location) = location
                && !Path::new(location).is_dir()
            {
                lines.push(format!(
                    "plugin {name}: its marketplace {marketplace} is recorded at {location}, \
                     which does not exist; Claude Code reports the plugin as failed to load \
                     and none of its hooks fires"
                ));
            }
        }
        if enabled {
            lines.push(format!(
                "plugin {name}: while it loads, its hooks fire beside any direct registration \
                 and a crossing is recorded twice; keep one of the two"
            ));
        }
    }
    lines
}

/// Read one host's registration back and check it against the machine.
pub fn doctor(surface: HostSurface, paths: &HostPaths, home: &Path) -> HostReport {
    match surface {
        HostSurface::ClaudeCode => doctor_claude(paths, home),
        HostSurface::Codex => doctor_codex(paths, home),
        HostSurface::Pi => doctor_pi(paths, home),
    }
}

fn doctor_pi(paths: &HostPaths, home: &Path) -> HostReport {
    let mut lines = Vec::new();
    let binary = match std::fs::read_to_string(&paths.pi_extension) {
        Ok(text) => match pi_extension_binary(&text) {
            Some(binary) => Some(binary),
            None => {
                lines.push(format!(
                    "extension: {} exists but names no binary; run commonmeasure install pi",
                    paths.pi_extension.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            lines.push(format!(
                "extension: none at {}",
                paths.pi_extension.display()
            ));
            None
        }
        Err(error) => {
            lines.push(format!(
                "extension: cannot read {}: {error}",
                paths.pi_extension.display()
            ));
            None
        }
    };
    if let Some(binary) = &binary {
        lines.push(format!(
            "extension: {} registered, naming {binary}",
            paths.pi_extension.display()
        ));
        lines.push(binary_line(binary));
    }
    lines.push(
        "hooks: none by design; Pi has no web tool of its own, so crossings are mediated or \
         nothing"
            .to_owned(),
    );
    lines.extend(home_lines(home));
    HostReport {
        host: "pi",
        registered: binary.is_some(),
        lines,
    }
}

fn doctor_claude(paths: &HostPaths, home: &Path) -> HostReport {
    let mut lines = Vec::new();
    let mut binaries: Vec<String> = Vec::new();
    let settings = match read_json(&paths.claude_settings) {
        Ok(settings) => settings,
        Err(error) => {
            return HostReport {
                host: "claude-code",
                registered: false,
                lines: vec![format!("hooks: {error}")],
            };
        }
    };
    let mut hooked = Vec::new();
    for (event, _, _) in CLAUDE_HOOKS {
        let commands: Vec<String> = settings["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["hooks"].as_array())
            .flatten()
            .filter_map(|handler| handler["command"].as_str())
            .filter(|command| is_our_hook_command(command))
            .map(program_of)
            .collect();
        if !commands.is_empty() {
            hooked.push(event);
        }
        for program in commands {
            if !binaries.contains(&program) {
                binaries.push(program);
            }
        }
    }
    if hooked.is_empty() {
        lines.push(format!(
            "hooks: none of this product in {}",
            paths.claude_settings.display()
        ));
    } else {
        lines.push(format!(
            "hooks: {} registered in {}{}",
            hooked.join(", "),
            paths.claude_settings.display(),
            if hooked.len() < CLAUDE_HOOKS.len() {
                format!(
                    " (missing {})",
                    CLAUDE_HOOKS
                        .iter()
                        .map(|(event, _, _)| *event)
                        .filter(|event| !hooked.contains(event))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            }
        ));
    }
    let server = read_json(&paths.claude_state)
        .ok()
        .map(|state| state["mcpServers"][SERVER_NAME].clone())
        .filter(|server| server.is_object());
    match &server {
        Some(server) => {
            let command = server["command"].as_str().unwrap_or_default().to_owned();
            lines.push(format!(
                "mcp: server {SERVER_NAME} registered at user scope in {}, command {command}",
                paths.claude_state.display()
            ));
            if !binaries.contains(&command) {
                binaries.push(command);
            }
        }
        None => lines.push(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.claude_state.display()
        )),
    }
    if binaries.len() > 1 {
        lines.push(
            "the hooks and the MCP server name different binaries; a session records under one \
             version and mediates under another"
                .to_owned(),
        );
    }
    for binary in &binaries {
        lines.push(binary_line(binary));
    }
    lines.extend(plugin_lines(paths, &settings));
    lines.extend(home_lines(home));
    HostReport {
        host: "claude-code",
        registered: !hooked.is_empty() || server.is_some(),
        lines,
    }
}

fn doctor_codex(paths: &HostPaths, home: &Path) -> HostReport {
    let mut lines = Vec::new();
    let command = match read_toml(&paths.codex_config) {
        Ok(document) => document
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table)
            .and_then(|servers| servers.get(SERVER_NAME))
            .and_then(|server| server.get("command"))
            .and_then(toml_edit::Item::as_str)
            .map(str::to_owned),
        Err(error) => {
            return HostReport {
                host: "codex",
                registered: false,
                lines: vec![format!("mcp: {error}")],
            };
        }
    };
    match &command {
        Some(command) => {
            lines.push(format!(
                "mcp: [mcp_servers.{SERVER_NAME}] registered in {}, command {command}",
                paths.codex_config.display()
            ));
            lines.push(binary_line(command));
        }
        None => lines.push(format!(
            "mcp: no [mcp_servers.{SERVER_NAME}] in {}",
            paths.codex_config.display()
        )),
    }
    lines.push(
        "hooks: none by design; Codex's hosted web search fires no hook and its shell reaches \
         the web as command text, so crossings are mediated or nothing"
            .to_owned(),
    );
    lines.extend(home_lines(home));
    HostReport {
        host: "codex",
        registered: command.is_some(),
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(directory: &Path) -> HostPaths {
        HostPaths {
            claude_settings: directory.join(".claude/settings.json"),
            claude_state: directory.join(".claude.json"),
            claude_plugins: directory.join(".claude/plugins"),
            codex_config: directory.join(".codex/config.toml"),
            pi_extension: directory.join(".pi/agent/extensions/commonmeasure/index.ts"),
        }
    }

    /// The written extension names the binary as one string literal, which
    /// is what the doctor reads back, and carries no marker.
    #[test]
    fn the_pi_extension_names_the_binary_and_reads_it_back() {
        let text = pi_extension_for(Path::new("/opt/common measure/commonmeasure"));
        assert!(!text.contains(PI_BINARY_MARKER));
        assert!(text.contains("const BINARY = \"/opt/common measure/commonmeasure\";"));
        assert_eq!(
            pi_extension_binary(&text).as_deref(),
            Some("/opt/common measure/commonmeasure")
        );
        assert!(text.contains("pi.registerTool("));
        assert!(text.contains("[\"mcp\", \"--host\", \"pi\"]"));
    }

    /// Only a command whose program is this product and whose first
    /// argument is `hook` is ours; a foreign hook that happens to mention
    /// the name is not.
    #[test]
    fn our_hook_commands_are_recognised_by_program_and_first_argument() {
        assert!(is_our_hook_command(
            "\"/home/op/.local/bin/commonmeasure\" hook post-tool-use"
        ));
        assert!(is_our_hook_command(
            "/opt/plugin/bin/commonmeasure-launch hook stop"
        ));
        assert!(is_our_hook_command("C:/bin/commonmeasure.exe hook stop"));
        assert!(!is_our_hook_command(
            "/usr/bin/notify-send commonmeasure hook"
        ));
        assert!(!is_our_hook_command("commonmeasure session"));
        assert!(!is_our_hook_command("\"unterminated hook"));
        assert_eq!(
            program_of("\"/a b/commonmeasure\" hook stop"),
            "/a b/commonmeasure"
        );
    }

    /// The registration touches its own entries and nothing else: a foreign
    /// hook on the same event, other settings keys and other MCP servers are
    /// written back as read, and a second install does not duplicate.
    #[test]
    fn install_and_uninstall_touch_only_this_products_entries() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".claude")).unwrap();
        std::fs::write(
            &paths.claude_settings,
            r#"{"model": "fable", "hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "/usr/bin/lint"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            &paths.claude_state,
            r#"{"numStartups": 3, "mcpServers": {"other": {"type": "stdio", "command": "/usr/bin/other"}}}"#,
        )
        .unwrap();
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();

        install_claude(&binary, &paths).expect("installs");
        install_claude(&binary, &paths).expect("installs again");
        let settings = read_json(&paths.claude_settings).unwrap();
        assert_eq!(settings["model"], "fable");
        let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2, "the foreign entry and exactly one of ours");
        assert_eq!(post[0]["hooks"][0]["command"], "/usr/bin/lint");
        assert_eq!(post[1]["matcher"], "WebFetch|WebSearch|mcp__.*");
        assert_eq!(
            post[1]["hooks"][0]["command"],
            format!("\"{}\" hook post-tool-use", binary.display())
        );
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let state = read_json(&paths.claude_state).unwrap();
        assert_eq!(state["numStartups"], 3);
        assert_eq!(state["mcpServers"]["other"]["command"], "/usr/bin/other");
        assert_eq!(
            state["mcpServers"][SERVER_NAME]["args"],
            json!(["mcp", "--host", "claude-code"])
        );

        uninstall_claude(&paths).expect("uninstalls");
        let settings = read_json(&paths.claude_settings).unwrap();
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            1
        );
        assert!(settings["hooks"].get("Stop").is_none());
        let state = read_json(&paths.claude_state).unwrap();
        assert!(state["mcpServers"].get(SERVER_NAME).is_none());
        assert_eq!(state["mcpServers"]["other"]["command"], "/usr/bin/other");
    }

    /// An enabled plugin means the plugin's own hooks fire; a second
    /// registration beside it would record every crossing twice.
    #[test]
    fn an_enabled_plugin_refuses_the_direct_registration() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".claude")).unwrap();
        std::fs::write(
            &paths.claude_settings,
            r#"{"enabledPlugins": {"commonmeasure@commonmeasure": true}}"#,
        )
        .unwrap();
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();
        let error = install_claude(&binary, &paths).expect_err("refuses");
        assert!(error.contains("claude plugin disable commonmeasure@commonmeasure"));
        assert!(!paths.claude_state.exists(), "nothing was written");
    }

    /// The Codex table is one table in the operator's file: every other
    /// table, key and comment comes back byte for byte, and removing the
    /// table restores the original bytes exactly.
    #[test]
    fn the_codex_table_is_added_and_removed_without_touching_the_rest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".codex")).unwrap();
        let original = "# operator notes\nmodel = \"gpt-5\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";
        std::fs::write(&paths.codex_config, original).unwrap();
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();

        install_codex(&binary, &paths).expect("installs");
        let text = std::fs::read_to_string(&paths.codex_config).unwrap();
        assert!(text.starts_with(original), "{text}");
        assert!(text.contains("[mcp_servers.commonmeasure]"), "{text}");
        assert!(
            text.contains("args = [\"mcp\", \"--host\", \"codex\"]"),
            "{text}"
        );
        assert!(
            !text.contains("\n[mcp_servers]\n"),
            "the parent table stays implicit: {text}"
        );

        uninstall_codex(&paths).expect("uninstalls");
        assert_eq!(
            std::fs::read_to_string(&paths.codex_config).unwrap(),
            original
        );
    }
}
