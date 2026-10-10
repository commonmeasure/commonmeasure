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
//! - Claude Code: the five hooks in the user settings file
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
//! - Claude Desktop: one `mcpServers` entry in its configuration file.
//!   Mediated only: it has no hook surface.
//! - Cursor: one `mcpServers` entry in `~/.cursor/mcp.json` and four hooks
//!   in `~/.cursor/hooks.json`.
//! - Copilot CLI: one `mcpServers` entry in `~/.copilot/mcp-config.json`
//!   (or `$COPILOT_HOME/mcp-config.json`), which the GitHub Copilot app and
//!   VS Code's Agent Host also read, and one hook file of this product's
//!   own, `hooks/commonmeasure.json` in the same directory, holding four
//!   hooks under the CLI's camelCase event names.
//! - VS Code: one `servers` entry in the user `mcp.json`. No hooks: VS Code
//!   runs hooks only through the Copilot Chat extension, which also loads
//!   Claude Code's hook files and ignores their matchers.
//! - Chrome: one native messaging host manifest,
//!   `ai.commonmeasure.browser.json`, in the browser's user-level
//!   `NativeMessagingHosts` directory, naming the binary and the Common
//!   Measure extension as the only origin allowed to start it. Chromium and
//!   Brave read the same manifest from their own directories, where it is
//!   written only if that browser's directory already exists. Observed only:
//!   the extension reads what the page shows and cannot refuse anything.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::hook::HostSurface;
use crate::policy::PolicyDocument;
use commonmeasure_types::Finding;

/// The server name every host sees, which is also the plugin's name: a
/// session with either registration spells the tools `context_fetch`,
/// `context_search` and `context_status` under `commonmeasure`.
pub const SERVER_NAME: &str = "commonmeasure";

/// The plugin identities whose enabled state means the plugin's hooks fire.
/// A direct registration beside an enabled plugin records every crossing
/// twice, so `install claude` refuses while one is enabled.
const PLUGIN_PREFIXES: [&str; 2] = ["commonmeasure@", "contextops@"];

/// The approval mode `install codex` writes on the server's table. Codex
/// asks before every MCP tool call unless the table says otherwise, and its
/// non-interactive runs (`codex exec`) refuse a call that would ask: with
/// no mode set they answer "MCP tool call requires approval, but approval
/// policy is never". `approve` approves every call on this server. The
/// `auto` mode hands the call to Codex's automatic reviewer, which refused
/// the mediated tools when tried, so it is not the value written.
pub const CODEX_APPROVAL_MODE: &str = "approve";

/// The observed path's hooks: the host event, the event name the `hook`
/// subcommand takes, and the tool matcher where the event has one.
pub const CLAUDE_HOOKS: [(&str, &str, Option<&str>); 5] = [
    ("SessionStart", "session-start", None),
    (
        "PostToolUse",
        "post-tool-use",
        Some("WebFetch|WebSearch|mcp__.*"),
    ),
    ("UserPromptSubmit", "user-prompt-submit", None),
    ("Stop", "stop", None),
    // The host's report that the session ended, recorded as
    // `session_ended` with the reason word the host gave.
    ("SessionEnd", "session-end", None),
];

/// The mediated tools `install claude` pre-approves in `permissions.allow`
/// of the user settings it writes, so a session does not ask before each
/// fetch or search: these two and no other, and no wildcard (owner decision,
/// 5 October 2026). `context_enrol` changes what may leave the machine, so
/// it keeps asking, as `context_status` does.
pub const CLAUDE_PRE_APPROVED: [&str; 2] = [
    "mcp__commonmeasure__context_fetch",
    "mcp__commonmeasure__context_search",
];

/// The file beside the Claude Code settings in which `install claude`
/// records which of [`CLAUDE_PRE_APPROVED`] it added. A permission entry is
/// a bare string, so nothing in the settings file says who wrote it; the
/// record is what lets `uninstall claude` remove what was added and keep an
/// entry the operator had written before.
const CLAUDE_PERMISSIONS_RECORD: &str = "commonmeasure-permissions.json";

/// Where each host keeps its registration, resolved from the environment
/// once so every command in one invocation reads and writes the same files.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub claude_settings: PathBuf,
    pub claude_state: PathBuf,
    pub claude_plugins: PathBuf,
    pub codex_config: PathBuf,
    /// User-scoped skill directory documented by current Codex hosts.
    pub codex_skill: PathBuf,
    pub pi_extension: PathBuf,
    /// Claude Desktop's configuration file, the one its Developer settings
    /// open: `mcpServers` is its only surface for a local server.
    pub claude_desktop_config: PathBuf,
    /// Cursor's global MCP file and its global hooks file, both under the
    /// user's `.cursor` directory.
    pub cursor_mcp: PathBuf,
    pub cursor_hooks: PathBuf,
    /// The Copilot CLI's user MCP file, and the hook file this product
    /// writes in the CLI's user hooks directory, which the CLI loads whole.
    pub copilot_mcp: PathBuf,
    pub copilot_hooks: PathBuf,
    /// VS Code's user-profile `mcp.json`, the default profile's.
    pub vscode_mcp: PathBuf,
    /// The Chromium-family browsers whose user-level native messaging
    /// directory the Chrome registration writes into, Chrome first.
    pub native_messaging: Vec<NativeMessagingDirectory>,
}

/// One browser's user-level native messaging directory.
#[derive(Debug, Clone)]
pub struct NativeMessagingDirectory {
    /// The browser, as the operator knows it.
    pub browser: &'static str,
    /// The browser's own user data directory. The manifest is written for a
    /// browser other than Chrome only when this exists, so the registration
    /// creates no directory for a browser the operator does not use.
    pub application: PathBuf,
    /// Whether the manifest is written even when `application` is absent.
    pub always: bool,
}

impl NativeMessagingDirectory {
    /// Chrome looks for user-level hosts in `NativeMessagingHosts` under its
    /// user data directory.
    pub fn manifest(&self) -> PathBuf {
        self.application
            .join("NativeMessagingHosts")
            .join(format!("{NATIVE_HOST}.json"))
    }
}

/// The native messaging host name the extension connects to. Chrome allows
/// lowercase letters, digits, dots and underscores.
pub const NATIVE_HOST: &str = "ai.commonmeasure.browser";

/// The Common Measure extension's id, fixed by the public key in
/// `browser/manifest.json`: the first 32 hexadecimal digits of the key's
/// SHA-256, each digit mapped from `0-f` to `a-p`. An unpacked extension
/// without a key gets an id from its directory path, which the native
/// messaging manifest could not name in advance.
pub const EXTENSION_ID: &str = "hojjbnoeobjkjklcdhhnncmmojmcneig";

/// The observed path's hooks for Cursor: Cursor's event name and the event
/// name the `hook` subcommand takes. The first four moments of Claude Code's
/// (`CLAUDE_HOOKS`), under Cursor's names; Cursor's `sessionEnd` is not
/// registered. `postToolUse` fires for every
/// tool, and the payload reader tells our own tools from the rest.
pub const CURSOR_HOOKS: [(&str, &str); 4] = [
    ("sessionStart", "session-start"),
    ("postToolUse", "post-tool-use"),
    ("beforeSubmitPrompt", "user-prompt-submit"),
    ("stop", "stop"),
];

/// The observed path's hooks for the Copilot CLI: the CLI's camelCase event
/// name, the event name the `hook` subcommand takes, and the tool matcher
/// where the event has one. The first four moments of Claude Code's
/// (`CLAUDE_HOOKS`); the CLI's `sessionEnd` is not registered. The camelCase names select the camelCase payload,
/// which carries `sessionId` and `toolResult.textResultForLlm`. The matcher
/// is a regular expression over the CLI's own tool names and names only the
/// two built-in web tools, so our own tools (`commonmeasure-context_fetch`)
/// never reach the hook.
pub const COPILOT_HOOKS: [(&str, &str, Option<&str>); 4] = [
    ("sessionStart", "session-start", None),
    ("postToolUse", "post-tool-use", Some("web_fetch|web_search")),
    ("userPromptSubmitted", "user-prompt-submit", None),
    ("agentStop", "stop", None),
];

/// The name of the hook file `install copilot` writes in the CLI's user
/// hooks directory. The file is this product's alone.
const COPILOT_HOOK_FILE: &str = "commonmeasure.json";

/// The extension `install pi` writes, with the binary's path substituted
/// where the marker stands.
const PI_EXTENSION: &str = include_str!("pi_extension.ts");
const PI_BINARY_MARKER: &str = "__COMMONMEASURE_BINARY__";

impl HostPaths {
    /// `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR` and
    /// `COPILOT_HOME` are the hosts' own overrides and
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
        // Claude Desktop and VS Code keep their files where the platform
        // keeps application data; neither has an override of its own.
        let application_data = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else if cfg!(windows) {
            std::env::var("APPDATA")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Roaming"))
        } else {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
        };
        let copilot_home = std::env::var("COPILOT_HOME")
            .ok()
            .filter(|dir| !dir.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".copilot"));
        // The browsers' user data directories; Windows finds native
        // messaging hosts through the registry instead, so it has none.
        let browsers: [(&'static str, &str, &str, bool); 3] = [
            ("Chrome", "Google/Chrome", "google-chrome", true),
            ("Chromium", "Chromium", "chromium", false),
            (
                "Brave",
                "BraveSoftware/Brave-Browser",
                "BraveSoftware/Brave-Browser",
                false,
            ),
        ];
        let native_messaging = if cfg!(target_os = "macos") {
            browsers
                .iter()
                .map(|(browser, mac, _, always)| NativeMessagingDirectory {
                    browser,
                    application: home.join("Library/Application Support").join(mac),
                    always: *always,
                })
                .collect()
        } else if cfg!(windows) {
            Vec::new()
        } else {
            let config = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            browsers
                .iter()
                .map(|(browser, _, linux, always)| NativeMessagingDirectory {
                    browser,
                    application: config.join(linux),
                    always: *always,
                })
                .collect()
        };
        Ok(Self {
            claude_settings,
            claude_state,
            claude_plugins,
            codex_config: codex_home.join("config.toml"),
            codex_skill: home.join(".agents/skills/commonmeasure-enrol/SKILL.md"),
            pi_extension: pi_agent.join("extensions/commonmeasure/index.ts"),
            claude_desktop_config: application_data.join("Claude/claude_desktop_config.json"),
            cursor_mcp: home.join(".cursor/mcp.json"),
            cursor_hooks: home.join(".cursor/hooks.json"),
            copilot_mcp: copilot_home.join("mcp-config.json"),
            copilot_hooks: copilot_home.join("hooks").join(COPILOT_HOOK_FILE),
            vscode_mcp: application_data.join("Code/User/mcp.json"),
            native_messaging,
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
    is_our_program(name) && rest.split_whitespace().next() == Some("hook")
}

/// Whether a program's file name is this product's binary or the plugin's
/// launcher.
fn is_our_program(name: &str) -> bool {
    matches!(
        name,
        "commonmeasure" | "commonmeasure.exe" | "commonmeasure-launch"
    )
}

/// Whether a Copilot CLI hook entry is one of this product's: it runs the
/// binary directly (`exec`) and its first argument is `hook`.
fn is_our_copilot_hook(entry: &Value) -> bool {
    entry["exec"].as_str().is_some_and(|program| {
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_our_program)
    }) && entry["args"][0] == "hook"
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

/// Write the whole file through a uniquely named temporary neighbour and a
/// rename, so a host reading the file mid-write sees the old bytes or the
/// new ones, and two installers never share a temporary. The file keeps the
/// mode it has, and an absent one is created owner-only
/// ([`crate::declaration::replace_keeping_mode`]): a host file the operator
/// made owner-only can hold another MCP server's key.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    crate::declaration::replace_keeping_mode(path, bytes)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
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

/// One Common Measure plugin Claude Code has installed, as its plugin
/// files record it.
struct PluginInstall {
    name: String,
    /// The install path Claude Code recorded, when it recorded one.
    path: Option<String>,
    /// Where the marketplace it came from is recorded, when it is.
    marketplace_location: Option<String>,
    enabled: bool,
}

impl PluginInstall {
    /// Whether Claude Code runs this plugin's hooks and MCP entry: enabled,
    /// and its files and its marketplace still where they were recorded.
    /// The one predicate behind both `install claude`'s refusal and
    /// `doctor`'s report, so the two never disagree about the same state.
    fn loads(&self) -> bool {
        self.enabled
            && self
                .path
                .as_deref()
                .is_none_or(|path| Path::new(path).is_dir())
            && self
                .marketplace_location
                .as_deref()
                .is_none_or(|location| Path::new(location).is_dir())
    }
}

/// Every Common Measure plugin in Claude Code's plugin files, in file order.
fn installed_plugins(paths: &HostPaths, settings: &Value) -> Vec<PluginInstall> {
    let installed = paths.claude_plugins.join("installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(&installed) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(plugins) = value["plugins"].as_object() else {
        return Vec::new();
    };
    let marketplaces =
        std::fs::read_to_string(paths.claude_plugins.join("known_marketplaces.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null);
    plugins
        .iter()
        .filter(|(name, _)| {
            PLUGIN_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .map(|(name, installs)| PluginInstall {
            name: name.clone(),
            path: installs
                .as_array()
                .and_then(|installs| installs.first())
                .and_then(|install| install["installPath"].as_str())
                .map(str::to_owned),
            marketplace_location: name
                .split('@')
                .nth(1)
                .and_then(|marketplace| marketplaces[marketplace]["installLocation"].as_str())
                .map(str::to_owned),
            enabled: settings["enabledPlugins"][name] == Value::Bool(true),
        })
        .collect()
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
        HostSurface::ClaudeDesktop => install_claude_desktop(binary, paths),
        HostSurface::Cursor => install_cursor(binary, paths),
        HostSurface::CopilotCli => install_copilot(binary, paths),
        HostSurface::VsCode => install_vscode(binary, paths),
        HostSurface::Chrome => install_chrome(binary, paths),
        surface @ (HostSurface::ChatgptWeb
        | HostSurface::GoogleAiOverview
        | HostSurface::BingCopilotSearch) => Err(browser_surface_is_not_a_registration(surface)),
    }
}

/// The hosts `install` registers, in the order the first run takes them.
/// The browser answer surfaces are recorded under their own names and
/// registered through Chrome.
pub const HOSTS: [HostSurface; 8] = [
    HostSurface::ClaudeCode,
    HostSurface::Codex,
    HostSurface::Pi,
    HostSurface::ClaudeDesktop,
    HostSurface::Cursor,
    HostSurface::CopilotCli,
    HostSurface::VsCode,
    HostSurface::Chrome,
];

/// Where `surface` is set up on this machine, as its own configuration
/// directory under the paths `install` writes to: the first such directory
/// that exists, or `None` when none does. The first run registers a host
/// only where this finds it, so it writes nothing for a host the operator
/// does not have. Each is a directory the host itself creates when it first
/// runs (Claude Code's also holds its state file when `$CLAUDE_CONFIG_DIR`
/// is unset, so that file counts as well); for Chrome, any Chromium-family
/// browser `install chrome` writes for. A browser answer surface is
/// registered through Chrome and is never found on its own.
pub fn detected(surface: HostSurface, paths: &HostPaths) -> Option<PathBuf> {
    let parent = |file: &Path| file.parent().map(Path::to_path_buf);
    let candidates: Vec<PathBuf> = match surface {
        HostSurface::ClaudeCode => parent(&paths.claude_settings)
            .into_iter()
            .chain([paths.claude_state.clone()])
            .collect(),
        HostSurface::Codex => parent(&paths.codex_config).into_iter().collect(),
        // `extensions/commonmeasure/index.ts` under the agent directory.
        HostSurface::Pi => paths
            .pi_extension
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .into_iter()
            .collect(),
        HostSurface::ClaudeDesktop => parent(&paths.claude_desktop_config).into_iter().collect(),
        HostSurface::Cursor => parent(&paths.cursor_mcp).into_iter().collect(),
        HostSurface::CopilotCli => parent(&paths.copilot_mcp).into_iter().collect(),
        HostSurface::VsCode => parent(&paths.vscode_mcp).into_iter().collect(),
        HostSurface::Chrome => paths
            .native_messaging
            .iter()
            .map(|browser| browser.application.clone())
            .collect(),
        HostSurface::ChatgptWeb
        | HostSurface::GoogleAiOverview
        | HostSurface::BingCopilotSearch => Vec::new(),
    };
    candidates.into_iter().find(|path| path.exists())
}

/// A browser answer surface is recorded under its own name and registered
/// through the browser.
fn browser_surface_is_not_a_registration(surface: HostSurface) -> String {
    format!(
        "{} is recorded through the browser extension; the registration is \
         commonmeasure install chrome",
        surface.id()
    )
}

/// Remove the product's registration from one host. Returns one line per
/// surface, saying what was removed or that nothing was registered.
pub fn uninstall(surface: HostSurface, paths: &HostPaths) -> Result<Vec<String>, String> {
    match surface {
        HostSurface::ClaudeCode => uninstall_claude(paths),
        HostSurface::Codex => uninstall_codex(paths),
        HostSurface::Pi => uninstall_pi(paths),
        HostSurface::ClaudeDesktop => uninstall_claude_desktop(paths),
        HostSurface::Cursor => uninstall_cursor(paths),
        HostSurface::CopilotCli => uninstall_copilot(paths),
        HostSurface::VsCode => uninstall_vscode(paths),
        HostSurface::Chrome => uninstall_chrome(paths),
        surface @ (HostSurface::ChatgptWeb
        | HostSurface::GoogleAiOverview
        | HostSurface::BingCopilotSearch) => Err(browser_surface_is_not_a_registration(surface)),
    }
}

/// The JSON file a host keeps its MCP servers in, under `key` (`mcpServers`
/// for Claude Desktop, Cursor and the Copilot CLI, `servers` for VS Code),
/// with our entry added or removed and every other key kept. Nothing is
/// ever deleted: a removal leaves the servers object in place, empty if
/// ours was its only entry, because the file and that object may be the
/// host's own and a file that vanished would be the host's loss. Returns
/// whether our entry was there.
fn set_json_server(path: &Path, key: &str, server: Option<Value>) -> Result<bool, String> {
    let mut document = read_json(path)?;
    let servers = document
        .as_object_mut()
        .expect("read_json returns an object")
        .entry(key)
        .or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!(
            "{} holds a \"{key}\" value that is not an object, so nothing is written to it",
            path.display()
        ));
    };
    let had = servers.contains_key(SERVER_NAME);
    match server {
        Some(server) => {
            servers.insert(SERVER_NAME.to_owned(), server);
        }
        None => {
            servers.remove(SERVER_NAME);
        }
    }
    write_json(path, &document)?;
    Ok(had)
}

fn install_claude_desktop(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    set_json_server(
        &paths.claude_desktop_config,
        "mcpServers",
        Some(json!({
            "command": binary.to_string_lossy(),
            "args": ["mcp", "--host", "claude-desktop"],
        })),
    )?;
    Ok(vec![
        format!(
            "claude-desktop: MCP server {SERVER_NAME} registered in {}, naming {}; Claude Desktop \
             loads it at its next start",
            paths.claude_desktop_config.display(),
            binary.display()
        ),
        "claude-desktop: mediated only. Claude Desktop has no hook surface, so nothing is \
         observed"
            .to_owned(),
        "claude-desktop: the application starts one server for its chat client and one for \
         its local agent mode; each that makes a call leaves its own session naming its \
         client (claude-ai, local-agent-mode-commonmeasure), and commonmeasure session shows \
         which"
            .to_owned(),
    ])
}

fn uninstall_claude_desktop(paths: &HostPaths) -> Result<Vec<String>, String> {
    let had = set_json_server(&paths.claude_desktop_config, "mcpServers", None)?;
    Ok(vec![
        if had {
            format!(
                "claude-desktop: MCP server {SERVER_NAME} removed from {}; the file and its \
                 mcpServers object stay, empty if ours was the only entry",
                paths.claude_desktop_config.display()
            )
        } else {
            format!(
                "claude-desktop: no MCP server {SERVER_NAME} in {}",
                paths.claude_desktop_config.display()
            )
        },
        "the evidence in the operator home is untouched".to_owned(),
    ])
}

/// Cursor reads `~/.cursor/hooks.json` for hooks and `~/.cursor/mcp.json`
/// for servers. The server is written first and taken back if the hooks
/// write fails, as for Claude Code: hooks with no server would record
/// without mediating.
fn install_cursor(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    let quoted = shell_quoted(binary);
    let mut hooks_document = read_json(&paths.cursor_hooks)?;
    let root = hooks_document
        .as_object_mut()
        .expect("read_json returns an object");
    root.entry("version").or_insert(json!(1));
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(format!(
            "{} holds a \"hooks\" value that is not an object, so nothing is written to it",
            paths.cursor_hooks.display()
        ));
    };
    for (event, argument) in CURSOR_HOOKS {
        let entries = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(entries) = entries.as_array_mut() else {
            return Err(format!(
                "{} holds a \"hooks.{event}\" value that is not an array, so nothing is \
                 written to it",
                paths.cursor_hooks.display()
            ));
        };
        strip_cursor_hooks(entries);
        entries.push(json!({"command": format!("{quoted} hook {argument} --host cursor")}));
    }

    let previous_mcp = read_previous(&paths.cursor_mcp)?;
    set_json_server(
        &paths.cursor_mcp,
        "mcpServers",
        Some(json!({
            "type": "stdio",
            "command": binary.to_string_lossy(),
            "args": ["mcp", "--host", "cursor"],
        })),
    )?;
    if let Err(error) = write_json(&paths.cursor_hooks, &hooks_document) {
        return Err(restore_after_failure(
            error,
            &paths.cursor_mcp,
            previous_mcp,
            "cursor",
        ));
    }
    Ok(vec![
        format!(
            "cursor: MCP server {SERVER_NAME} registered in {}, naming {}",
            paths.cursor_mcp.display(),
            binary.display()
        ),
        format!(
            "cursor: four hooks (sessionStart, postToolUse, beforeSubmitPrompt, stop) \
             registered in {}, each naming the same binary; Cursor reloads the file on save",
            paths.cursor_hooks.display()
        ),
        "cursor: observed crossings come from postToolUse, which carries every tool's output; \
         the session is Cursor's conversation_id"
            .to_owned(),
    ])
}

/// Remove this product's handlers from one Cursor event's entries. Cursor's
/// entries are flat `{"command": …}` objects, unlike Claude Code's nested
/// ones.
fn strip_cursor_hooks(entries: &mut Vec<Value>) -> usize {
    let before = entries.len();
    entries.retain(|entry| !entry["command"].as_str().is_some_and(is_our_hook_command));
    before - entries.len()
}

fn uninstall_cursor(paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut hooks_document = read_json(&paths.cursor_hooks)?;
    let mut removed = 0;
    if let Some(hooks) = hooks_document["hooks"].as_object_mut() {
        for (event, _) in CURSOR_HOOKS {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                removed += strip_cursor_hooks(entries);
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|e| !e.is_empty()));
    }
    if removed > 0 {
        // Nothing is deleted: an event array we emptied goes, the `hooks`
        // object and the file stay, because either may be Cursor's own.
        write_json(&paths.cursor_hooks, &hooks_document)?;
        lines.push(format!(
            "cursor: {removed} hook handler(s) removed from {}; the file stays",
            paths.cursor_hooks.display()
        ));
    } else {
        lines.push(format!(
            "cursor: no hook of this product in {}",
            paths.cursor_hooks.display()
        ));
    }
    let had = set_json_server(&paths.cursor_mcp, "mcpServers", None)?;
    lines.push(if had {
        format!(
            "cursor: MCP server {SERVER_NAME} removed from {}; the file and its mcpServers \
             object stay, empty if ours was the only entry",
            paths.cursor_mcp.display()
        )
    } else {
        format!(
            "cursor: no MCP server {SERVER_NAME} in {}",
            paths.cursor_mcp.display()
        )
    });
    lines.push("the evidence in the operator home is untouched".to_owned());
    Ok(lines)
}

/// The Copilot CLI reads `mcp-config.json` for servers and loads every
/// `*.json` file in its user hooks directory. The server goes into the
/// operator's file beside their own; the hooks go into a file of this
/// product's own. The server is written first and taken back if the hooks
/// write fails, as for Cursor.
fn install_copilot(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    let program = binary.to_string_lossy();
    let mut hooks_document = read_json(&paths.copilot_hooks)?;
    let root = hooks_document
        .as_object_mut()
        .expect("read_json returns an object");
    root.entry("version").or_insert(json!(1));
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(format!(
            "{} holds a \"hooks\" value that is not an object, so nothing is written to it",
            paths.copilot_hooks.display()
        ));
    };
    for (event, argument, matcher) in COPILOT_HOOKS {
        let entries = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(entries) = entries.as_array_mut() else {
            return Err(format!(
                "{} holds a \"hooks.{event}\" value that is not an array, so nothing is \
                 written to it",
                paths.copilot_hooks.display()
            ));
        };
        entries.retain(|entry| !is_our_copilot_hook(entry));
        // `exec` runs the binary with no shell between, so the path needs
        // no quoting on any platform.
        let mut entry = json!({"type": "command"});
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        entry["exec"] = json!(program);
        entry["args"] = json!(["hook", argument, "--host", "copilot-cli"]);
        entries.push(entry);
    }

    let previous_mcp = read_previous(&paths.copilot_mcp)?;
    set_json_server(
        &paths.copilot_mcp,
        "mcpServers",
        Some(json!({
            "type": "local",
            "command": program,
            "args": ["mcp", "--host", "copilot-cli"],
            "tools": ["*"],
        })),
    )?;
    if let Err(error) = write_json(&paths.copilot_hooks, &hooks_document) {
        return Err(restore_after_failure(
            error,
            &paths.copilot_mcp,
            previous_mcp,
            "copilot",
        ));
    }
    Ok(vec![
        format!(
            "copilot-cli: MCP server {SERVER_NAME} registered in {}, naming {}; the GitHub \
             Copilot app and VS Code's Agent Host read the same file",
            paths.copilot_mcp.display(),
            binary.display()
        ),
        format!(
            "copilot-cli: four hooks (sessionStart, postToolUse, userPromptSubmitted, agentStop) \
             registered in {}, each running the same binary",
            paths.copilot_hooks.display()
        ),
        "copilot-cli: observed crossings come from postToolUse on web_fetch and web_search; the \
         session is the CLI's sessionId. A call needs a Copilot plan that allows MCP"
            .to_owned(),
    ])
}

fn uninstall_copilot(paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut hooks_document = read_json(&paths.copilot_hooks)?;
    let mut removed = 0;
    if let Some(hooks) = hooks_document["hooks"].as_object_mut() {
        for (event, _, _) in COPILOT_HOOKS {
            if let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                let before = entries.len();
                entries.retain(|entry| !is_our_copilot_hook(entry));
                removed += before - entries.len();
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|e| !e.is_empty()));
    }
    if removed > 0 {
        // The file is this product's: it goes when nothing but ours was in
        // it, and stays with the rest when someone added to it. The hooks
        // directory is the CLI's and stays.
        let only_ours = hooks_document.as_object().is_some_and(|root| {
            root.keys().all(|key| key == "version" || key == "hooks")
                && root["hooks"]
                    .as_object()
                    .is_none_or(|hooks| hooks.is_empty())
        });
        if only_ours {
            std::fs::remove_file(&paths.copilot_hooks).map_err(|error| {
                format!("cannot remove {}: {error}", paths.copilot_hooks.display())
            })?;
            lines.push(format!(
                "copilot-cli: {removed} hook handler(s) removed; {} deleted",
                paths.copilot_hooks.display()
            ));
        } else {
            write_json(&paths.copilot_hooks, &hooks_document)?;
            lines.push(format!(
                "copilot-cli: {removed} hook handler(s) removed from {}; the file stays, because \
                 it holds entries that are not this product's",
                paths.copilot_hooks.display()
            ));
        }
    } else {
        lines.push(format!(
            "copilot-cli: no hook of this product in {}",
            paths.copilot_hooks.display()
        ));
    }
    let had = set_json_server(&paths.copilot_mcp, "mcpServers", None)?;
    lines.push(if had {
        format!(
            "copilot-cli: MCP server {SERVER_NAME} removed from {}; the file and its mcpServers \
             object stay, empty if ours was the only entry",
            paths.copilot_mcp.display()
        )
    } else {
        format!(
            "copilot-cli: no MCP server {SERVER_NAME} in {}",
            paths.copilot_mcp.display()
        )
    });
    lines.push("the evidence in the operator home is untouched".to_owned());
    Ok(lines)
}

/// VS Code accepts comments in `mcp.json`; this writer does not keep them,
/// so a file carrying one is refused whole rather than rewritten without
/// it.
fn vscode_json_error(error: String) -> String {
    if error.contains("is not valid JSON") {
        format!(
            "{error}. VS Code allows comments in this file and this writer would drop them, so \
             nothing is written; remove the comments, or add the server with VS Code's MCP: Add \
             Server command"
        )
    } else {
        error
    }
}

/// VS Code reads the user-profile `mcp.json`, key `servers`. It is written
/// directly rather than through `code --add-mcp`, which only adds, needs
/// the `code` command on `PATH`, and leaves `uninstall` and `doctor` to
/// read the file anyway.
fn install_vscode(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    set_json_server(
        &paths.vscode_mcp,
        "servers",
        Some(json!({
            "type": "stdio",
            "command": binary.to_string_lossy(),
            "args": ["mcp", "--host", "vscode"],
        })),
    )
    .map_err(vscode_json_error)?;
    Ok(vec![
        format!(
            "vscode: MCP server {SERVER_NAME} registered in {}, naming {}; VS Code starts it the \
             first time a chat uses it, and forwards it to the Agent Host for the Copilot harness",
            paths.vscode_mcp.display(),
            binary.display()
        ),
        "vscode: mediated only. VS Code runs hooks only through the Copilot Chat extension, which \
         also loads Claude Code's hook files and ignores their matchers, so no hook is registered"
            .to_owned(),
    ])
}

fn uninstall_vscode(paths: &HostPaths) -> Result<Vec<String>, String> {
    let had = set_json_server(&paths.vscode_mcp, "servers", None).map_err(vscode_json_error)?;
    Ok(vec![
        if had {
            format!(
                "vscode: MCP server {SERVER_NAME} removed from {}; the file and its servers \
                 object stay, empty if ours was the only entry",
                paths.vscode_mcp.display()
            )
        } else {
            format!(
                "vscode: no MCP server {SERVER_NAME} in {}",
                paths.vscode_mcp.display()
            )
        },
        "the evidence in the operator home is untouched".to_owned(),
    ])
}

/// A file's bytes before a write that may have to be undone, or `None`
/// when it did not exist.
fn read_previous(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Put the MCP file back as it was after the hooks write failed, and say
/// whether that worked: hooks and server must not disagree.
fn restore_after_failure(
    error: String,
    path: &Path,
    previous: Option<Vec<u8>>,
    host: &str,
) -> String {
    let restored = match previous {
        Some(bytes) => std::fs::write(path, bytes),
        None => std::fs::remove_file(path),
    };
    match restored {
        Ok(()) => format!(
            "{error}; {} was restored, so nothing is registered",
            path.display()
        ),
        Err(restore_error) => format!(
            "{error}; and {} could not be restored ({restore_error}), so the MCP server is \
             registered with no hooks: run commonmeasure uninstall {host}",
            path.display()
        ),
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

/// The native messaging host manifest `install chrome` writes, byte for byte.
/// Chrome starts `path` itself, with the calling extension's origin as the
/// only argument, so the manifest can carry no subcommand: the binary
/// recognises that argument (`commonmeasure native-host`).
pub fn native_messaging_manifest(binary: &Path) -> String {
    let document = json!({
        "name": NATIVE_HOST,
        "description": "Common Measure: records the sources browser AI answers show",
        "path": binary.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let mut text = serde_json::to_string_pretty(&document).expect("a JSON value serialises");
    text.push('\n');
    text
}

/// A native messaging manifest is ours when it names our host; a file under
/// our name that names another is left where it is.
fn is_our_native_manifest(path: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(serde_json::from_str::<Value>(&text).is_ok_and(|document| document["name"] == NATIVE_HOST))
}

fn install_chrome(binary: &Path, paths: &HostPaths) -> Result<Vec<String>, String> {
    if paths.native_messaging.is_empty() {
        return Err(
            "Chrome on this platform finds native messaging hosts through the registry, which \
             this command does not write; nothing was written"
                .to_owned(),
        );
    }
    let manifest = native_messaging_manifest(binary);
    let mut lines = Vec::new();
    for directory in &paths.native_messaging {
        if !directory.always && !directory.application.is_dir() {
            lines.push(format!(
                "chrome: {} is not set up here ({} does not exist), so nothing is written for it",
                directory.browser,
                directory.application.display()
            ));
            continue;
        }
        write_atomically(&directory.manifest(), manifest.as_bytes())?;
        lines.push(format!(
            "chrome: native messaging host {NATIVE_HOST} registered for {} in {}, naming {}",
            directory.browser,
            directory.manifest().display(),
            binary.display()
        ));
    }
    lines.push(format!(
        "chrome: only the extension chrome-extension://{EXTENSION_ID}/ may start it; load \
         browser/ unpacked in chrome://extensions, and a running browser picks the host up \
         at the extension's next message"
    ));
    lines.push(
        "chrome: observed only. The extension records the sources ChatGPT and Bing Copilot \
         Search show, retrieved and never grounded, and refuses nothing; it records nothing \
         from Google AI Overviews"
            .to_owned(),
    );
    Ok(lines)
}

fn uninstall_chrome(paths: &HostPaths) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for directory in &paths.native_messaging {
        let manifest = directory.manifest();
        if !manifest.exists() {
            lines.push(format!(
                "chrome: no native messaging host {NATIVE_HOST} for {} in {}",
                directory.browser,
                manifest.display()
            ));
            continue;
        }
        if !is_our_native_manifest(&manifest)? {
            lines.push(format!(
                "chrome: {} names another host, so it is left in place",
                manifest.display()
            ));
            continue;
        }
        std::fs::remove_file(&manifest)
            .map_err(|error| format!("cannot remove {}: {error}", manifest.display()))?;
        lines.push(format!(
            "chrome: native messaging host {NATIVE_HOST} removed for {} from {}; the directory \
             stays",
            directory.browser,
            manifest.display()
        ));
    }
    lines.push(
        "the extension stays loaded until it is removed in chrome://extensions; the evidence in \
         the operator home is untouched"
            .to_owned(),
    );
    Ok(lines)
}

fn doctor_chrome(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let mut registered = false;
    if paths.native_messaging.is_empty() {
        findings.push(Finding::note(
            "native messaging: this platform's browsers read the registry, which this binary \
             does not write"
                .to_owned(),
        ));
    }
    for directory in &paths.native_messaging {
        let manifest = directory.manifest();
        let document = match std::fs::read_to_string(&manifest) {
            Ok(text) => serde_json::from_str::<Value>(&text).ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                findings.push(Finding::note(format!(
                    "native messaging: no host {NATIVE_HOST} for {} in {}",
                    directory.browser,
                    manifest.display()
                )));
                continue;
            }
            Err(error) => {
                findings.push(Finding::unknown(format!(
                    "native messaging: cannot read {}: {error}",
                    manifest.display()
                )));
                continue;
            }
        };
        let Some(document) = document.filter(|document| document["name"] == NATIVE_HOST) else {
            findings.push(Finding::note(format!(
                "native messaging: {} is not this product's manifest",
                manifest.display()
            )));
            continue;
        };
        registered = true;
        let command = document["path"].as_str().unwrap_or_default().to_owned();
        findings.push(Finding::ok(format!(
            "native messaging: host {NATIVE_HOST} registered for {} in {}, command {command}",
            directory.browser,
            manifest.display()
        )));
        let origin = format!("chrome-extension://{EXTENSION_ID}/");
        let allows_ours = document["allowed_origins"]
            .as_array()
            .is_some_and(|origins| origins.iter().any(|allowed| allowed == &json!(origin)));
        if !allows_ours {
            findings.push(Finding::attention(format!(
                "native messaging: the manifest does not allow {origin}, so the extension cannot \
                 start the binary; run commonmeasure install chrome"
            )));
        }
        findings.push(binary_finding(&command));
    }
    findings.push(Finding::note(
        "extension: whether it is loaded is the browser's record and is not read here; its \
         popup says whether it reaches the binary"
            .to_owned(),
    ));
    findings.push(Finding::note(
        "hooks: none; the extension observes ChatGPT and Bing Copilot Search, and nothing it \
         records is mediated"
            .to_owned(),
    ));
    HostReport {
        host: "chrome",
        registered,
        findings,
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
    if let Some(plugin) = installed_plugins(paths, &settings)
        .into_iter()
        .find(PluginInstall::loads)
    {
        return Err(format!(
            "the plugin {} is enabled in {} and its files are in place, so Claude Code runs its \
             hooks. A direct registration beside it would record every crossing twice. Disable \
             it first: claude plugin disable {}. Nothing was written.",
            plugin.name,
            paths.claude_settings.display(),
            plugin.name
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
        // `--host claude-code` is spelt out because Cursor and VS Code load
        // this file and run its commands with payloads of their own; the
        // reader told it is reading Claude Code refuses those.
        let mut entry = json!({
            "hooks": [{"type": "command", "command": format!("{quoted} hook {argument} --host claude-code")}]
        });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        entries.push(entry);
    }
    let record_path = permissions_record(paths);
    let recorded = read_permissions_record(&record_path)?;
    let added = pre_approve(&mut settings, &recorded, &paths.claude_settings)?;
    // The state file (the MCP server) goes first and is restored if the
    // settings write (the hooks) then fails, so the two surfaces never
    // disagree: hooks with no server would record without mediating.
    let previous_state = read_previous(&paths.claude_state)?;
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
        // The Stop hook submits context entry and citation from the
        // transcript, which needs the session's fetches to carry handles.
        "args": ["mcp", "--host", "claude-code", "--host-observations"],
    });
    write_json(&paths.claude_state, &state)?;
    // The record before the settings, so no pre-approval is ever in the
    // settings without the record that lets `uninstall` remove it.
    let previous_record = read_previous(&record_path)?;
    let record = (!added.is_empty() || previous_record.is_some()).then(|| {
        write_json(
            &record_path,
            &json!({"settings": paths.claude_settings, "added": added}),
        )
    });
    if let Some(Err(error)) = record {
        return Err(restore_after_failure(
            error,
            &paths.claude_state,
            previous_state,
            "claude",
        ));
    }
    if let Err(error) = write_json(&paths.claude_settings, &settings) {
        if record.is_some() {
            let _ = match previous_record {
                Some(bytes) => std::fs::write(&record_path, bytes),
                None => std::fs::remove_file(&record_path),
            };
        }
        return Err(restore_after_failure(
            error,
            &paths.claude_state,
            previous_state,
            "claude",
        ));
    }

    Ok(vec![
        format!(
            "claude-code: five hooks (SessionStart, PostToolUse, UserPromptSubmit, Stop, \
             SessionEnd) \
             registered in {}, each naming {}",
            paths.claude_settings.display(),
            binary.display()
        ),
        format!(
            "claude-code: MCP server {SERVER_NAME} registered at user scope in {}, naming the \
             same binary",
            paths.claude_state.display()
        ),
        format!(
            "claude-code: {} pre-approved in permissions.allow in {}; context_enrol and \
             context_status still ask",
            CLAUDE_PRE_APPROVED.join(" and "),
            paths.claude_settings.display()
        ),
        "claude-code: a running session picks this up on its next start".to_owned(),
    ])
}

fn permissions_record(paths: &HostPaths) -> PathBuf {
    paths
        .claude_settings
        .with_file_name(CLAUDE_PERMISSIONS_RECORD)
}

/// The pre-approvals an earlier `install claude` recorded adding. Only
/// names in [`CLAUDE_PRE_APPROVED`] are taken from the file, so whatever it
/// holds, `uninstall` can remove nothing else.
fn read_permissions_record(path: &Path) -> Result<Vec<&'static str>, String> {
    let record = read_json(path)?;
    Ok(CLAUDE_PRE_APPROVED
        .into_iter()
        .filter(|name| {
            record["added"]
                .as_array()
                .is_some_and(|added| added.contains(&json!(name)))
        })
        .collect())
}

/// Add each of [`CLAUDE_PRE_APPROVED`] that `settings` lacks to its
/// `permissions.allow`, and return those this product holds as its own: the
/// ones added now and the ones `recorded` says an earlier install added. An
/// entry the operator wrote is left where it is and is not claimed.
fn pre_approve(
    settings: &mut Value,
    recorded: &[&'static str],
    file: &Path,
) -> Result<Vec<&'static str>, String> {
    let permissions = settings
        .as_object_mut()
        .expect("read_json returns an object")
        .entry("permissions")
        .or_insert_with(|| json!({}));
    let Some(permissions) = permissions.as_object_mut() else {
        return Err(format!(
            "{} holds a \"permissions\" value that is not an object, so nothing is written to it",
            file.display()
        ));
    };
    let Some(allow) = permissions
        .entry("allow")
        .or_insert_with(|| json!([]))
        .as_array_mut()
    else {
        return Err(format!(
            "{} holds a \"permissions.allow\" value that is not an array, so nothing is \
             written to it",
            file.display()
        ));
    };
    let mut ours = Vec::new();
    for name in CLAUDE_PRE_APPROVED {
        if !allow.contains(&json!(name)) {
            allow.push(json!(name));
            ours.push(name);
        } else if recorded.contains(&name) {
            ours.push(name);
        }
    }
    Ok(ours)
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
    let record_path = permissions_record(paths);
    let added = read_permissions_record(&record_path)?;
    let mut withdrawn = 0;
    if let Some(permissions) = settings["permissions"].as_object_mut()
        && let Some(allow) = permissions.get_mut("allow").and_then(Value::as_array_mut)
    {
        let before = allow.len();
        allow.retain(|entry| !entry.as_str().is_some_and(|name| added.contains(&name)));
        withdrawn = before - allow.len();
        if withdrawn > 0 && allow.is_empty() {
            permissions.remove("allow");
        }
        if withdrawn > 0 && permissions.is_empty() {
            settings
                .as_object_mut()
                .expect("object")
                .remove("permissions");
        }
    }
    let kept: Vec<&str> = CLAUDE_PRE_APPROVED
        .into_iter()
        .filter(|name| {
            settings["permissions"]["allow"]
                .as_array()
                .is_some_and(|allow| allow.contains(&json!(name)))
        })
        .collect();
    if removed > 0 || withdrawn > 0 {
        write_json(&paths.claude_settings, &settings)?;
    }
    if removed > 0 {
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
    if withdrawn > 0 {
        lines.push(format!(
            "claude-code: {withdrawn} pre-approval(s) removed from permissions.allow in {}",
            paths.claude_settings.display()
        ));
    }
    if !kept.is_empty() {
        lines.push(format!(
            "claude-code: {} kept in permissions.allow, as it was there before install",
            kept.join(" and ")
        ));
    }
    if let Err(error) = std::fs::remove_file(&record_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("cannot remove {}: {error}", record_path.display()));
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

const ENROL_SKILL: &str = include_str!("../../../plugin/skills/commonmeasure-enrol/SKILL.md");
const ENROL_POLICY: &str =
    include_str!("../../../plugin/skills/commonmeasure-enrol/agents/openai.yaml");
const ENROL_MARKER: &str = "<!-- commonmeasure managed enrol skill -->\n";

fn enrol_skill_path(paths: &HostPaths) -> PathBuf {
    paths.codex_skill.clone()
}

fn install_enrol_skill(binary: &Path, paths: &HostPaths) -> Result<(), String> {
    let path = enrol_skill_path(paths);
    if path.exists()
        && !std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .contains(ENROL_MARKER)
    {
        return Err(format!(
            "{} is not a Common Measure managed skill; preserve or move it before installing",
            path.display()
        ));
    }
    let metadata = path
        .parent()
        .ok_or("skill parent unavailable")?
        .join("agents/openai.yaml");
    if metadata.exists()
        && std::fs::read_to_string(&metadata).map_err(|e| e.to_string())? != ENROL_POLICY
    {
        return Err(format!(
            "{} has custom skill settings; preserve or move them before upgrading",
            metadata.display()
        ));
    }
    let text = format!(
        "{ENROL_SKILL}\n{ENROL_MARKER}\nInstalled binary: {}. Use this exact executable path when PATH is unavailable.\n",
        binary.display()
    );
    write_atomically(&path, text.as_bytes())?;
    write_atomically(
        &path
            .parent()
            .ok_or("skill parent unavailable")?
            .join("agents/openai.yaml"),
        ENROL_POLICY.as_bytes(),
    )
}

fn uninstall_enrol_skill(paths: &HostPaths) -> Result<(), String> {
    let path = enrol_skill_path(paths);
    if path.exists()
        && std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .contains(ENROL_MARKER)
    {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        let policy = path
            .parent()
            .ok_or("skill parent unavailable")?
            .join("agents/openai.yaml");
        if std::fs::read_to_string(&policy).ok().as_deref() == Some(ENROL_POLICY) {
            std::fs::remove_file(policy).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
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
    server["default_tools_approval_mode"] = toml_edit::value(CODEX_APPROVAL_MODE);
    servers[SERVER_NAME] = toml_edit::Item::Table(server);
    install_enrol_skill(binary, paths)?;
    write_atomically(&paths.codex_config, document.to_string().as_bytes())?;
    Ok(vec![
        format!(
            "codex: [mcp_servers.{SERVER_NAME}] registered in {}, naming {}",
            paths.codex_config.display(),
            binary.display()
        ),
        format!(
            "codex: default_tools_approval_mode = \"{CODEX_APPROVAL_MODE}\", so Codex calls the \
             tools without asking and its non-interactive runs can use them; the same table \
             serves the Codex CLI, the ChatGPT desktop app and the IDE extension"
        ),
        "codex: mediated only. Codex's hosted web search fires no hook and its shell reaches \
         the web as command text, so no observed matcher is registered"
            .to_owned(),
    ])
}

fn uninstall_codex(paths: &HostPaths) -> Result<Vec<String>, String> {
    uninstall_enrol_skill(paths)?;
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

/// What one host's registration says, checked against the machine: each
/// finding with its standing, so `doctor` can mark the ones that need the
/// operator without reading the words.
#[derive(Debug, Clone)]
pub struct HostReport {
    pub host: &'static str,
    pub registered: bool,
    pub findings: Vec<Finding>,
}

/// What a registered binary path does when run: its reported version, or
/// why it could not be run. A binary that is not there, or does not answer
/// `--version`, needs the operator: every session registered against it
/// records nothing.
fn binary_finding(path: &str) -> Finding {
    let file = Path::new(path);
    if !file.exists() {
        return Finding::attention(format!(
            "binary {path}: not found; every session registered against it records nothing \
             until it is restored or the registration is rewritten (commonmeasure install)"
        ));
    }
    match std::process::Command::new(file).arg("--version").output() {
        Ok(output) if output.status.success() => Finding::ok(format!(
            "binary {path}: runs, reports {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )),
        Ok(output) => Finding::attention(format!(
            "binary {path}: exits {} on --version",
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "by signal".to_owned())
        )),
        Err(error) => Finding::attention(format!("binary {path}: cannot run: {error}")),
    }
}

/// The hooks a host has registered, complete or with some of the set
/// missing. A partial set records some moments of a session and not
/// others, which is the operator's to put right (`commonmeasure install`).
fn hooks_finding(registered: &[&str], missing: &[&str], file: &Path) -> Finding {
    let text = format!(
        "hooks: {} registered in {}{}",
        registered.join(", "),
        file.display(),
        if missing.is_empty() {
            String::new()
        } else {
            format!(" (missing {})", missing.join(", "))
        }
    );
    if missing.is_empty() {
        Finding::ok(text)
    } else {
        Finding::attention(text)
    }
}

/// Whether a session log can be written in the operator home, in the words
/// `doctor` uses.
pub fn recording_line(home: &Path) -> String {
    home_findings(home).swap_remove(0).text
}

/// The operator home's state, the same whatever the host: whether a session
/// log can be written, and what the policy file says. `doctor` prints these
/// once, before the hosts, because every host's sessions share them.
pub fn home_findings(home: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    let sessions = home.join("sessions");
    let probe = sessions.join(".doctor-probe");
    let writable = std::fs::create_dir_all(&sessions)
        .and_then(|()| std::fs::write(&probe, b""))
        .and_then(|()| std::fs::remove_file(&probe));
    findings.push(match writable {
        Ok(()) => Finding::ok(format!("recording: {} is writable", sessions.display())),
        Err(error) => Finding::attention(format!(
            "recording: {} cannot be written ({error}); hooks exit zero and record nothing",
            sessions.display()
        )),
    });
    findings.push(match PolicyDocument::read(home) {
        Ok(document) if !document.declared() => Finding::note(format!(
            "policy: {} absent; observe mode, refusing nothing",
            home.join("policy.json").display()
        )),
        Ok(document) => {
            let resolved = document.resolve(None);
            Finding::ok(format!(
                "policy: {} loads; top-level mode {:?}, {} scope(s), {} principal(s)",
                home.join("policy.json").display(),
                resolved.mode(),
                document.scopes().len(),
                document.principals().len()
            ))
        }
        Err(error) => Finding::attention(format!(
            "policy: cannot load; the mediated tools refuse every crossing until it is fixed: \
             {error}"
        )),
    });
    findings
}

/// What the plugin route says for Claude Code: one line per Common Measure
/// plugin Claude Code has installed, and whether one of them loads, which
/// is a registration in its own right. The double-recording warning is
/// given only when a direct registration stands beside a plugin that
/// loads, because that is the only state in which two sets of hooks fire
/// for one crossing.
fn plugin_findings(paths: &HostPaths, settings: &Value, direct: bool) -> (Vec<Finding>, bool) {
    let mut findings = Vec::new();
    let mut registers = false;
    for plugin in installed_plugins(paths, settings) {
        let name = &plugin.name;
        findings.push(Finding::note(format!(
            "plugin {name}: installed at {}, {} in {}",
            plugin.path.as_deref().unwrap_or("an unrecorded path"),
            if plugin.enabled {
                "enabled"
            } else {
                "disabled"
            },
            paths.claude_settings.display()
        )));
        if let Some(path) = &plugin.path
            && !Path::new(path).is_dir()
        {
            findings.push(Finding::attention(format!(
                "plugin {name}: {path} does not exist, so the plugin loads nothing and none of \
                 its hooks fires"
            )));
        }
        // The marketplace the plugin was installed from must still exist
        // where Claude Code recorded it: a moved directory makes the host
        // report the plugin as failed to load, and every hook stays silent.
        if let Some(location) = &plugin.marketplace_location
            && !Path::new(location).is_dir()
        {
            findings.push(Finding::attention(format!(
                "plugin {name}: its marketplace is recorded at {location}, which does not \
                 exist; Claude Code reports the plugin as failed to load and none of its hooks \
                 fires"
            )));
        }
        if plugin.loads() {
            registers = true;
            if direct {
                findings.push(Finding::attention(format!(
                    "plugin {name}: its hooks fire beside the direct registration and a \
                     crossing is recorded twice; keep one of the two (claude plugin disable \
                     {name}, or commonmeasure uninstall claude)"
                )));
            } else {
                findings.push(Finding::ok(format!(
                    "plugin {name}: this is the registration; its hooks and its MCP entry \
                     are what Claude Code runs"
                )));
            }
        }
    }
    (findings, registers)
}

/// Whether a `SessionEnd` hook of this product would fire in Claude Code:
/// its own entry in the settings file, or an enabled plugin whose files
/// still load, since `plugin/hooks/hooks.json` registers the same events.
/// No other host's install registers the event
/// (`docs/contracts/host-integration.md` §2), so this answers whether a
/// session ending on this machine reaches the hook at all — and therefore
/// whether the relay runs without a person
/// (`docs/contracts/telemetry-projection.md` §Relay at session end).
pub fn session_end_registered(paths: &HostPaths) -> bool {
    let Ok(settings) = read_json(&paths.claude_settings) else {
        return false;
    };
    let direct = settings["hooks"]["SessionEnd"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["hooks"].as_array())
        .flatten()
        .filter_map(|handler| handler["command"].as_str())
        .any(is_our_hook_command);
    direct
        || installed_plugins(paths, &settings)
            .iter()
            .any(PluginInstall::loads)
}

/// Read one host's registration back and check it against the machine.
pub fn doctor(surface: HostSurface, paths: &HostPaths) -> HostReport {
    match surface {
        HostSurface::ClaudeCode => doctor_claude(paths),
        HostSurface::Codex => doctor_codex(paths),
        HostSurface::Pi => doctor_pi(paths),
        HostSurface::ClaudeDesktop => doctor_claude_desktop(paths),
        HostSurface::Cursor => doctor_cursor(paths),
        HostSurface::CopilotCli => doctor_copilot(paths),
        HostSurface::VsCode => doctor_vscode(paths),
        HostSurface::Chrome => doctor_chrome(paths),
        surface @ (HostSurface::ChatgptWeb
        | HostSurface::GoogleAiOverview
        | HostSurface::BingCopilotSearch) => HostReport {
            host: surface.id(),
            registered: false,
            findings: vec![Finding::note(browser_surface_is_not_a_registration(
                surface,
            ))],
        },
    }
}

/// The command a host's servers entry (under `key`) names, when the entry
/// is ours.
fn json_server_command(path: &Path, key: &str) -> Result<Option<String>, String> {
    let document = read_json(path)?;
    Ok(document[key][SERVER_NAME]
        .as_object()
        .and_then(|server| server.get("command"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}

fn doctor_claude_desktop(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let command = match json_server_command(&paths.claude_desktop_config, "mcpServers") {
        Ok(command) => command,
        Err(error) => {
            return HostReport {
                host: "claude-desktop",
                registered: false,
                findings: vec![Finding::unknown(format!("mcp: {error}"))],
            };
        }
    };
    match &command {
        Some(command) => {
            findings.push(Finding::ok(format!(
                "mcp: server {SERVER_NAME} registered in {}, command {command}",
                paths.claude_desktop_config.display()
            )));
            findings.push(binary_finding(command));
        }
        None => findings.push(Finding::note(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.claude_desktop_config.display()
        ))),
    }
    findings.push(Finding::note(
        "hooks: none; Claude Desktop has no hook surface, so crossings are mediated or nothing"
            .to_owned(),
    ));
    findings.push(Finding::note(
        "servers: two per launch, one for the chat client and one for the local agent mode; \
         each that makes a call leaves its own session naming its client"
            .to_owned(),
    ));
    HostReport {
        host: "claude-desktop",
        registered: command.is_some(),
        findings,
    }
}

fn doctor_cursor(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let mut binaries: Vec<String> = Vec::new();
    let hooks_document = match read_json(&paths.cursor_hooks) {
        Ok(document) => document,
        Err(error) => {
            return HostReport {
                host: "cursor",
                registered: false,
                findings: vec![Finding::unknown(format!("hooks: {error}"))],
            };
        }
    };
    let mut hooked = Vec::new();
    for (event, _) in CURSOR_HOOKS {
        let commands: Vec<String> = hooks_document["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["command"].as_str())
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
        findings.push(Finding::note(format!(
            "hooks: none of this product in {}",
            paths.cursor_hooks.display()
        )));
    } else {
        let missing: Vec<&str> = CURSOR_HOOKS
            .iter()
            .map(|(event, _)| *event)
            .filter(|event| !hooked.contains(event))
            .collect();
        findings.push(hooks_finding(&hooked, &missing, &paths.cursor_hooks));
    }
    let command = match json_server_command(&paths.cursor_mcp, "mcpServers") {
        Ok(command) => command,
        Err(error) => {
            findings.push(Finding::unknown(format!("mcp: {error}")));
            None
        }
    };
    match &command {
        Some(command) => {
            findings.push(Finding::ok(format!(
                "mcp: server {SERVER_NAME} registered in {}, command {command}",
                paths.cursor_mcp.display()
            )));
            if !binaries.contains(command) {
                binaries.push(command.clone());
            }
        }
        None => findings.push(Finding::note(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.cursor_mcp.display()
        ))),
    }
    if binaries.len() > 1 {
        findings.push(Finding::attention(
            "the hooks and the MCP server name different binaries; a session records under one \
             version and mediates under another"
                .to_owned(),
        ));
    }
    for binary in &binaries {
        findings.push(binary_finding(binary));
    }
    HostReport {
        host: "cursor",
        registered: !hooked.is_empty() || command.is_some(),
        findings,
    }
}

fn doctor_copilot(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let mut binaries: Vec<String> = Vec::new();
    let hooks_document = match read_json(&paths.copilot_hooks) {
        Ok(document) => document,
        Err(error) => {
            return HostReport {
                host: "copilot-cli",
                registered: false,
                findings: vec![Finding::unknown(format!("hooks: {error}"))],
            };
        }
    };
    let mut hooked = Vec::new();
    for (event, _, _) in COPILOT_HOOKS {
        let programs: Vec<String> = hooks_document["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| is_our_copilot_hook(entry))
            .filter_map(|entry| entry["exec"].as_str().map(str::to_owned))
            .collect();
        if !programs.is_empty() {
            hooked.push(event);
        }
        for program in programs {
            if !binaries.contains(&program) {
                binaries.push(program);
            }
        }
    }
    if hooked.is_empty() {
        findings.push(Finding::note(format!(
            "hooks: none of this product in {}",
            paths.copilot_hooks.display()
        )));
    } else {
        let missing: Vec<&str> = COPILOT_HOOKS
            .iter()
            .map(|(event, _, _)| *event)
            .filter(|event| !hooked.contains(event))
            .collect();
        findings.push(hooks_finding(&hooked, &missing, &paths.copilot_hooks));
    }
    let command = match json_server_command(&paths.copilot_mcp, "mcpServers") {
        Ok(command) => command,
        Err(error) => {
            findings.push(Finding::unknown(format!("mcp: {error}")));
            None
        }
    };
    match &command {
        Some(command) => {
            findings.push(Finding::ok(format!(
                "mcp: server {SERVER_NAME} registered in {}, command {command}",
                paths.copilot_mcp.display()
            )));
            if !binaries.contains(command) {
                binaries.push(command.clone());
            }
        }
        None => findings.push(Finding::note(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.copilot_mcp.display()
        ))),
    }
    if binaries.len() > 1 {
        findings.push(Finding::attention(
            "the hooks and the MCP server name different binaries; a session records under one \
             version and mediates under another"
                .to_owned(),
        ));
    }
    for binary in &binaries {
        findings.push(binary_finding(binary));
    }
    findings.push(Finding::note(
        "readers: the Copilot CLI, the GitHub Copilot app and VS Code's Agent Host read the \
         same mcp-config.json; the hooks are the CLI's"
            .to_owned(),
    ));
    findings.push(Finding::note(
        "calls: a Copilot plan that allows MCP is needed; without one the CLI reports the server \
         as blocked by policy and never starts it"
            .to_owned(),
    ));
    HostReport {
        host: "copilot-cli",
        registered: !hooked.is_empty() || command.is_some(),
        findings,
    }
}

fn doctor_vscode(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let command = match json_server_command(&paths.vscode_mcp, "servers") {
        Ok(command) => command,
        Err(error) => {
            return HostReport {
                host: "vscode",
                registered: false,
                findings: vec![Finding::unknown(format!(
                    "mcp: {}",
                    vscode_json_error(error)
                ))],
            };
        }
    };
    match &command {
        Some(command) => {
            findings.push(Finding::ok(format!(
                "mcp: server {SERVER_NAME} registered in {}, command {command}",
                paths.vscode_mcp.display()
            )));
            findings.push(binary_finding(command));
        }
        None => findings.push(Finding::note(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.vscode_mcp.display()
        ))),
    }
    findings.push(Finding::note(
        "hooks: none by decision; VS Code runs hooks only through the Copilot Chat extension, \
         which also loads Claude Code's hook files and ignores their matchers"
            .to_owned(),
    ));
    if command.is_some()
        && json_server_command(&paths.copilot_mcp, "mcpServers").is_ok_and(|c| c.is_some())
    {
        findings.push(Finding::note(format!(
            "copilot: {} names the server too; VS Code forwards this entry to its Agent Host, \
             which also reads that file, and which of the two the Copilot harness starts is not \
             documented",
            paths.copilot_mcp.display()
        )));
    }
    HostReport {
        host: "vscode",
        registered: command.is_some(),
        findings,
    }
}

fn doctor_pi(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let binary = match std::fs::read_to_string(&paths.pi_extension) {
        Ok(text) => match pi_extension_binary(&text) {
            Some(binary) => Some(binary),
            None => {
                findings.push(Finding::attention(format!(
                    "extension: {} exists but names no binary; run commonmeasure install pi",
                    paths.pi_extension.display()
                )));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            findings.push(Finding::note(format!(
                "extension: none at {}",
                paths.pi_extension.display()
            )));
            None
        }
        Err(error) => {
            findings.push(Finding::unknown(format!(
                "extension: cannot read {}: {error}",
                paths.pi_extension.display()
            )));
            None
        }
    };
    if let Some(binary) = &binary {
        findings.push(Finding::ok(format!(
            "extension: {} registered, naming {binary}",
            paths.pi_extension.display()
        )));
        findings.push(binary_finding(binary));
    }
    findings.push(Finding::note(
        "hooks: none by design; Pi has no web tool of its own, so crossings are mediated or \
         nothing"
            .to_owned(),
    ));
    HostReport {
        host: "pi",
        registered: binary.is_some(),
        findings,
    }
}

/// Which of [`CLAUDE_PRE_APPROVED`] the settings allow, and any other
/// allowance of this server's tools, which `install` never writes: a
/// wildcard or `context_enrol` there lets an agent enrol a directory for
/// reporting without asking.
fn permissions_finding(settings: &Value, file: &Path) -> Finding {
    let allow: Vec<&str> = settings["permissions"]["allow"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let (granted, asking): (Vec<&str>, Vec<&str>) = CLAUDE_PRE_APPROVED
        .into_iter()
        .partition(|name| allow.contains(name));
    let others: Vec<&str> = allow
        .iter()
        .copied()
        .filter(|name| {
            name.starts_with(&format!("mcp__{SERVER_NAME}")) && !CLAUDE_PRE_APPROVED.contains(name)
        })
        .collect();
    let mut text = match (granted.is_empty(), asking.is_empty()) {
        (_, true) => format!(
            "permissions: {} pre-approved in {}",
            granted.join(" and "),
            file.display()
        ),
        (true, _) => format!(
            "permissions: neither {} is pre-approved in {}, so each call asks",
            CLAUDE_PRE_APPROVED.join(" nor "),
            file.display()
        ),
        (false, false) => format!(
            "permissions: {} pre-approved in {}; {} asks",
            granted.join(" and "),
            file.display(),
            asking.join(" and ")
        ),
    };
    if !others.is_empty() {
        text.push_str(&format!(
            "; also allowed there: {}, which install does not write",
            others.join(", ")
        ));
        return Finding::attention(text);
    }
    if asking.is_empty() {
        Finding::ok(text)
    } else {
        Finding::note(text)
    }
}

fn doctor_claude(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let mut binaries: Vec<String> = Vec::new();
    let settings = match read_json(&paths.claude_settings) {
        Ok(settings) => settings,
        Err(error) => {
            return HostReport {
                host: "claude-code",
                registered: false,
                findings: vec![Finding::unknown(format!("hooks: {error}"))],
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
        findings.push(Finding::note(format!(
            "hooks: none of this product in {}",
            paths.claude_settings.display()
        )));
    } else {
        let missing: Vec<&str> = CLAUDE_HOOKS
            .iter()
            .map(|(event, _, _)| *event)
            .filter(|event| !hooked.contains(event))
            .collect();
        findings.push(hooks_finding(&hooked, &missing, &paths.claude_settings));
    }
    let server = read_json(&paths.claude_state)
        .ok()
        .map(|state| state["mcpServers"][SERVER_NAME].clone())
        .filter(|server| server.is_object());
    match &server {
        Some(server) => {
            let command = server["command"].as_str().unwrap_or_default().to_owned();
            findings.push(Finding::ok(format!(
                "mcp: server {SERVER_NAME} registered at user scope in {}, command {command}",
                paths.claude_state.display()
            )));
            if !binaries.contains(&command) {
                binaries.push(command);
            }
        }
        None => findings.push(Finding::note(format!(
            "mcp: no server {SERVER_NAME} in {}",
            paths.claude_state.display()
        ))),
    }
    if binaries.len() > 1 {
        findings.push(Finding::attention(
            "the hooks and the MCP server name different binaries; a session records under one \
             version and mediates under another"
                .to_owned(),
        ));
    }
    for binary in &binaries {
        findings.push(binary_finding(binary));
    }
    let direct = !hooked.is_empty() || server.is_some();
    if direct {
        findings.push(permissions_finding(&settings, &paths.claude_settings));
    }
    let (plugin, plugin_registers) = plugin_findings(paths, &settings, direct);
    findings.extend(plugin);
    HostReport {
        host: "claude-code",
        registered: direct || plugin_registers,
        findings,
    }
}

fn doctor_codex(paths: &HostPaths) -> HostReport {
    let mut findings = Vec::new();
    let server = match read_toml(&paths.codex_config) {
        Ok(document) => document
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table)
            .and_then(|servers| servers.get(SERVER_NAME))
            .and_then(toml_edit::Item::as_table)
            .cloned(),
        Err(error) => {
            return HostReport {
                host: "codex",
                registered: false,
                findings: vec![Finding::unknown(format!("mcp: {error}"))],
            };
        }
    };
    let command = server
        .as_ref()
        .and_then(|server| server.get("command"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_owned);
    match &command {
        Some(command) => {
            findings.push(Finding::ok(format!(
                "mcp: [mcp_servers.{SERVER_NAME}] registered in {}, command {command}",
                paths.codex_config.display()
            )));
            findings.push(binary_finding(command));
            let approval = server
                .as_ref()
                .and_then(|server| server.get("default_tools_approval_mode"))
                .and_then(toml_edit::Item::as_str);
            findings.push(match approval {
                Some(mode) if mode == CODEX_APPROVAL_MODE => Finding::ok(format!(
                    "approval: default_tools_approval_mode = \"{mode}\"; Codex calls the tools \
                     without asking, in the CLI, the ChatGPT desktop app and the IDE extension"
                )),
                Some(mode) => Finding::attention(format!(
                    "approval: default_tools_approval_mode = \"{mode}\", not the \
                     \"{CODEX_APPROVAL_MODE}\" install writes; Codex may ask before each call or \
                     refuse it in a non-interactive run"
                )),
                None => Finding::attention(format!(
                    "approval: no default_tools_approval_mode on the table; Codex asks before \
                     each call and codex exec refuses them; run commonmeasure install codex to \
                     write \"{CODEX_APPROVAL_MODE}\""
                )),
            });
        }
        None => findings.push(Finding::note(format!(
            "mcp: no [mcp_servers.{SERVER_NAME}] in {}",
            paths.codex_config.display()
        ))),
    }
    findings.push(Finding::note(
        "hooks: none by design; Codex's hosted web search fires no hook and its shell reaches \
         the web as command text, so crossings are mediated or nothing"
            .to_owned(),
    ));
    // A missing skill beside a registered server is an install left
    // incomplete; beside no registration it is one more thing nobody asked
    // for.
    findings.push(if paths.codex_skill.is_file() {
        Finding::ok(format!(
            "enrol skill: installed at {}",
            paths.codex_skill.display()
        ))
    } else if command.is_some() {
        Finding::attention(format!(
            "enrol skill: missing; run commonmeasure install codex to write {}",
            paths.codex_skill.display()
        ))
    } else {
        Finding::note(format!(
            "enrol skill: missing; commonmeasure install codex writes {}",
            paths.codex_skill.display()
        ))
    });
    HostReport {
        host: "codex",
        registered: command.is_some(),
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hosts the licence ruling counts as delivering at session end are
    /// exactly those whose hook table registers the session-end event.
    #[test]
    fn the_session_end_hosts_are_the_hook_tables_that_register_session_end() {
        use crate::delivery::SESSION_END_HOSTS;
        let tables: [(&str, bool); 3] = [
            (
                "claude-code",
                CLAUDE_HOOKS
                    .iter()
                    .any(|(_, argument, _)| *argument == "session-end"),
            ),
            (
                "cursor",
                CURSOR_HOOKS
                    .iter()
                    .any(|(_, argument)| *argument == "session-end"),
            ),
            (
                "copilot-cli",
                COPILOT_HOOKS
                    .iter()
                    .any(|(_, argument, _)| *argument == "session-end"),
            ),
        ];
        for (host, registers) in tables {
            assert_eq!(SESSION_END_HOSTS.contains(&host), registers, "{host}");
        }
        assert!(
            SESSION_END_HOSTS.iter().all(|host| tables
                .iter()
                .any(|(table, registers)| table == host && *registers)),
            "every session-end host has a hook table that registers the event"
        );
    }

    fn paths_in(directory: &Path) -> HostPaths {
        HostPaths {
            claude_settings: directory.join(".claude/settings.json"),
            claude_state: directory.join(".claude.json"),
            claude_plugins: directory.join(".claude/plugins"),
            codex_config: directory.join(".codex/config.toml"),
            codex_skill: directory.join(".agents/skills/commonmeasure-enrol/SKILL.md"),
            pi_extension: directory.join(".pi/agent/extensions/commonmeasure/index.ts"),
            claude_desktop_config: directory.join("Claude/claude_desktop_config.json"),
            cursor_mcp: directory.join(".cursor/mcp.json"),
            cursor_hooks: directory.join(".cursor/hooks.json"),
            copilot_mcp: directory.join(".copilot/mcp-config.json"),
            copilot_hooks: directory.join(".copilot/hooks/commonmeasure.json"),
            vscode_mcp: directory.join("Code/User/mcp.json"),
            native_messaging: vec![
                NativeMessagingDirectory {
                    browser: "Chrome",
                    application: directory.join("Google/Chrome"),
                    always: true,
                },
                NativeMessagingDirectory {
                    browser: "Brave",
                    application: directory.join("BraveSoftware/Brave-Browser"),
                    always: false,
                },
            ],
        }
    }

    /// EGR-149 (private-replace review N1). Catches: a host file staged
    /// through a temporary created under the umask, which turns a 0600 file
    /// holding another MCP server's key into 0644; an absent host file
    /// created under the umask; a 0644 host file narrowed. Every surface
    /// family's install and uninstall writes are checked.
    #[cfg(unix)]
    #[test]
    fn install_and_uninstall_keep_each_host_files_mode() {
        crate::test_umask::under_umask_022(
            "registration::tests::install_and_uninstall_keep_each_host_files_mode",
            install_and_uninstall_keep_each_host_files_mode_body,
        );
    }

    #[cfg(unix)]
    fn install_and_uninstall_keep_each_host_files_mode_body() {
        use std::os::unix::fs::PermissionsExt as _;
        const PLANTED: &str =
            r#"{"mcpServers":{"other":{"command":"other","env":{"API_KEY":"planted"}}}}"#;
        let mode_of = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        // `None` is an absent file, created owner-only because the host may
        // later put keys in it.
        for before in [Some(0o600), None, Some(0o644)] {
            let directory = tempfile::tempdir().unwrap();
            let paths = paths_in(directory.path());
            let binary = directory.path().join("commonmeasure");
            std::fs::write(&binary, b"").unwrap();
            let skill_policy = paths
                .codex_skill
                .parent()
                .unwrap()
                .join("agents/openai.yaml");
            let families: [(HostSurface, Vec<(PathBuf, String)>); 8] = [
                (
                    HostSurface::ClaudeCode,
                    vec![
                        (paths.claude_state.clone(), PLANTED.to_owned()),
                        (paths.claude_settings.clone(), PLANTED.to_owned()),
                    ],
                ),
                (
                    HostSurface::Cursor,
                    vec![
                        (paths.cursor_mcp.clone(), PLANTED.to_owned()),
                        (paths.cursor_hooks.clone(), PLANTED.to_owned()),
                    ],
                ),
                (
                    HostSurface::ClaudeDesktop,
                    vec![(paths.claude_desktop_config.clone(), PLANTED.to_owned())],
                ),
                (
                    HostSurface::CopilotCli,
                    vec![
                        (paths.copilot_mcp.clone(), PLANTED.to_owned()),
                        (paths.copilot_hooks.clone(), PLANTED.to_owned()),
                    ],
                ),
                (
                    HostSurface::VsCode,
                    vec![(paths.vscode_mcp.clone(), PLANTED.to_owned())],
                ),
                (
                    HostSurface::Pi,
                    vec![(paths.pi_extension.clone(), "// planted\n".to_owned())],
                ),
                (
                    HostSurface::Chrome,
                    vec![(paths.native_messaging[0].manifest(), PLANTED.to_owned())],
                ),
                (
                    HostSurface::Codex,
                    vec![
                        (
                            paths.codex_config.clone(),
                            "[mcp_servers.other]\ncommand = \"other\"\nenv = { API_KEY = \"planted\" }\n"
                                .to_owned(),
                        ),
                        (paths.codex_skill.clone(), ENROL_MARKER.to_owned()),
                        (skill_policy, ENROL_POLICY.to_owned()),
                    ],
                ),
            ];
            for (surface, files) in &families {
                if let Some(mode) = before {
                    for (path, text) in files {
                        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                        std::fs::write(path, text).unwrap();
                        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                            .unwrap();
                    }
                }
                let expected = before.unwrap_or(0o600);
                install(*surface, &binary, &paths).unwrap();
                for (path, _) in files {
                    assert_eq!(
                        mode_of(path),
                        expected,
                        "{surface:?} install, {} was {before:?}",
                        path.display()
                    );
                }
                // The manifest is ours whole; every other host file keeps
                // the planted key beside our entry.
                if before.is_some() && *surface != HostSurface::Chrome {
                    for (path, _) in files.iter().filter(|(_, text)| text == PLANTED) {
                        let now = std::fs::read_to_string(path).unwrap();
                        assert!(now.contains("planted"), "{}: {now}", path.display());
                    }
                }
                uninstall(*surface, &paths).unwrap();
                for (path, _) in files {
                    if path.exists() {
                        assert_eq!(
                            mode_of(path),
                            expected,
                            "{surface:?} uninstall, {} was {before:?}",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    /// The id the native messaging manifest allows is the one Chrome derives
    /// from the key in the extension's own manifest, and the extension's
    /// version is the binary's: the two halves cannot drift apart unnoticed.
    #[test]
    fn the_extension_id_is_derived_from_the_key_the_extension_carries() {
        use base64::Engine as _;
        use sha2::Digest as _;
        let manifest: Value =
            serde_json::from_str(include_str!("../../../browser/manifest.json")).expect("JSON");
        let key = base64::engine::general_purpose::STANDARD
            .decode(manifest["key"].as_str().expect("a key"))
            .expect("base64");
        let digest = sha2::Sha256::digest(&key);
        let id: String = digest[..16]
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0x0f])
            .map(|nibble| char::from(b'a' + nibble))
            .collect();
        assert_eq!(id, EXTENSION_ID);
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        assert!(
            manifest["permissions"]
                .as_array()
                .is_some_and(|permissions| permissions.contains(&json!("nativeMessaging")))
        );
    }

    /// Chrome is always written; a browser whose directory does not exist is
    /// not, so the registration creates nothing for a browser not in use.
    /// A file under our name that names another host survives uninstall.
    #[test]
    fn the_chrome_manifest_is_written_where_a_browser_is_set_up_and_removed_only_if_ours() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();

        let lines = install_chrome(&binary, &paths).expect("installs");
        assert!(paths.native_messaging[0].manifest().is_file());
        assert!(!paths.native_messaging[1].application.exists(), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Brave is not set up here"))
        );

        std::fs::create_dir_all(&paths.native_messaging[1].application).unwrap();
        install_chrome(&binary, &paths).expect("installs again");
        assert_eq!(
            std::fs::read_to_string(paths.native_messaging[1].manifest()).unwrap(),
            native_messaging_manifest(&binary)
        );
        let report = doctor_chrome(&paths);
        assert!(report.registered, "{:?}", report.findings);

        std::fs::write(
            paths.native_messaging[1].manifest(),
            r#"{"name": "com.example.other", "path": "/usr/bin/other"}"#,
        )
        .unwrap();
        uninstall_chrome(&paths).expect("uninstalls");
        assert!(!paths.native_messaging[0].manifest().exists());
        assert!(
            paths.native_messaging[1].manifest().exists(),
            "a manifest naming another host is not ours to remove"
        );
        assert!(
            paths.native_messaging[0]
                .manifest()
                .parent()
                .is_some_and(Path::is_dir),
            "the directory stays"
        );
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
            format!(
                "\"{}\" hook post-tool-use --host claude-code",
                binary.display()
            )
        );
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let state = read_json(&paths.claude_state).unwrap();
        assert_eq!(state["numStartups"], 3);
        assert_eq!(state["mcpServers"]["other"]["command"], "/usr/bin/other");
        assert_eq!(
            state["mcpServers"][SERVER_NAME]["args"],
            json!(["mcp", "--host", "claude-code", "--host-observations"])
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

    /// A host is found by the directory it creates for itself, under the
    /// same paths `install` writes to, and by nothing else.
    #[test]
    fn a_host_is_detected_by_its_own_configuration_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        for surface in HOSTS.into_iter().chain([HostSurface::ChatgptWeb]) {
            assert_eq!(detected(surface, &paths), None, "{surface:?}");
        }
        for made in [
            ".codex",
            ".pi/agent",
            "Claude",
            ".cursor",
            ".copilot",
            "Code/User",
            "BraveSoftware/Brave-Browser",
        ] {
            std::fs::create_dir_all(directory.path().join(made)).unwrap();
        }
        // Claude Code's state file alone is Claude Code having run.
        std::fs::write(&paths.claude_state, "{}").unwrap();
        for surface in HOSTS {
            assert!(detected(surface, &paths).is_some(), "{surface:?}");
        }
        assert_eq!(detected(HostSurface::ChatgptWeb, &paths), None);
        assert_eq!(
            detected(HostSurface::Pi, &paths),
            Some(directory.path().join(".pi/agent"))
        );
    }

    fn our_allowances(settings: &Value) -> Vec<String> {
        settings["permissions"]["allow"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|name| name.starts_with("mcp__commonmeasure"))
            .map(str::to_owned)
            .collect()
    }

    /// The settings `install claude` writes allow exactly the two mediated
    /// tools the owner pre-approved and no other entry of this product's,
    /// beside the operator's own; a second install adds nothing; uninstall
    /// takes out the two and leaves the operator's.
    #[test]
    fn install_pre_approves_fetch_and_search_only_and_uninstall_removes_them() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".claude")).unwrap();
        std::fs::write(
            &paths.claude_settings,
            r#"{"permissions": {"allow": ["Bash(git status)"], "deny": ["WebFetch"]}}"#,
        )
        .unwrap();
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();

        let lines = install_claude(&binary, &paths).expect("installs");
        assert!(
            lines.iter().any(|line| line.contains(
                "mcp__commonmeasure__context_fetch and mcp__commonmeasure__context_search \
                 pre-approved in permissions.allow"
            )),
            "{lines:?}"
        );
        install_claude(&binary, &paths).expect("installs again");
        let settings = read_json(&paths.claude_settings).unwrap();
        assert_eq!(
            our_allowances(&settings),
            CLAUDE_PRE_APPROVED,
            "exactly the two, once each"
        );
        assert_eq!(
            settings["permissions"]["allow"],
            json!([
                "Bash(git status)",
                "mcp__commonmeasure__context_fetch",
                "mcp__commonmeasure__context_search"
            ])
        );
        assert_eq!(settings["permissions"]["deny"], json!(["WebFetch"]));
        let finding = permissions_finding(&settings, &paths.claude_settings);
        assert_eq!(finding.standing, commonmeasure_types::Standing::Ok);
        assert!(finding.text.contains("pre-approved"), "{}", finding.text);

        uninstall_claude(&paths).expect("uninstalls");
        let settings = read_json(&paths.claude_settings).unwrap();
        assert_eq!(
            settings["permissions"]["allow"],
            json!(["Bash(git status)"])
        );
        assert!(!permissions_record(&paths).exists());
    }

    /// An allowance the operator wrote before install is theirs: install
    /// does not claim it and uninstall leaves it, while the one install
    /// added goes.
    #[test]
    fn uninstall_keeps_a_pre_approval_the_operator_already_had() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".claude")).unwrap();
        std::fs::write(
            &paths.claude_settings,
            r#"{"permissions": {"allow": ["mcp__commonmeasure__context_fetch"]}}"#,
        )
        .unwrap();
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();

        install_claude(&binary, &paths).expect("installs");
        install_claude(&binary, &paths).expect("installs again");
        assert_eq!(
            read_permissions_record(&permissions_record(&paths)).unwrap(),
            ["mcp__commonmeasure__context_search"]
        );
        let lines = uninstall_claude(&paths).expect("uninstalls");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("mcp__commonmeasure__context_fetch kept")),
            "{lines:?}"
        );
        let settings = read_json(&paths.claude_settings).unwrap();
        assert_eq!(
            settings["permissions"]["allow"],
            json!(["mcp__commonmeasure__context_fetch"])
        );
    }

    /// Doctor names a missing pre-approval, and an allowance of this
    /// server's tools that install never writes.
    #[test]
    fn doctor_reports_the_two_pre_approvals_and_any_other_allowance() {
        let file = Path::new("/settings.json");
        let finding = permissions_finding(&json!({}), file);
        assert_eq!(finding.standing, commonmeasure_types::Standing::Note);
        assert!(finding.text.contains("each call asks"), "{}", finding.text);
        let finding = permissions_finding(
            &json!({"permissions": {"allow": [
                "mcp__commonmeasure__context_fetch",
                "mcp__commonmeasure__context_enrol"
            ]}}),
            file,
        );
        assert_eq!(finding.standing, commonmeasure_types::Standing::Attention);
        assert!(
            finding
                .text
                .contains("mcp__commonmeasure__context_search asks"),
            "{}",
            finding.text
        );
        assert!(
            finding.text.contains(
                "also allowed there: mcp__commonmeasure__context_enrol, which install does not \
                 write"
            ),
            "{}",
            finding.text
        );
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
        let plugin_dir = directory.path().join("plugin-files");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::create_dir_all(&paths.claude_plugins).unwrap();
        std::fs::write(
            paths.claude_plugins.join("installed_plugins.json"),
            format!(
                r#"{{"version": 2, "plugins": {{"commonmeasure@commonmeasure": [{{"scope": "user", "installPath": "{}", "version": "0.3.0"}}]}}}}"#,
                plugin_dir.display()
            ),
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
            text.contains("default_tools_approval_mode = \"approve\""),
            "the approval mode Codex's non-interactive runs need: {text}"
        );
        assert!(
            !text.contains("\n[mcp_servers]\n"),
            "the parent table stays implicit: {text}"
        );
        let report = doctor_codex(&paths);
        assert!(report.registered);
        assert!(
            report.findings.iter().any(|finding| finding
                .text
                .contains("approval: default_tools_approval_mode = \"approve\"")),
            "{:?}",
            report.findings
        );

        uninstall_codex(&paths).expect("uninstalls");
        assert_eq!(
            std::fs::read_to_string(&paths.codex_config).unwrap(),
            original
        );
    }

    /// A table without the approval mode, as an older install wrote it, is
    /// registered but reported as one Codex will ask about on every call.
    #[test]
    fn the_codex_doctor_names_a_missing_approval_mode() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".codex")).unwrap();
        std::fs::write(
            &paths.codex_config,
            "[mcp_servers.commonmeasure]\ncommand = \"/usr/bin/commonmeasure\"\nargs = [\"mcp\", \"--host\", \"codex\"]\n",
        )
        .unwrap();
        let report = doctor_codex(&paths);
        assert!(report.registered);
        assert!(
            report.findings.iter().any(|finding| finding
                .text
                .contains("approval: no default_tools_approval_mode")),
            "{:?}",
            report.findings
        );
    }

    /// A table carrying a different approval mode is registered, and the
    /// doctor says which mode it found and which install writes.
    #[test]
    fn the_codex_doctor_names_an_approval_mode_that_is_not_the_one_install_writes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        std::fs::create_dir_all(directory.path().join(".codex")).unwrap();
        std::fs::write(
            &paths.codex_config,
            "[mcp_servers.commonmeasure]\ncommand = \"/usr/bin/commonmeasure\"\nargs = [\"mcp\", \"--host\", \"codex\"]\ndefault_tools_approval_mode = \"prompt\"\n",
        )
        .unwrap();
        let report = doctor_codex(&paths);
        assert!(report.registered);
        assert!(
            report.findings.iter().any(|finding| {
                finding.text.contains(
                    "approval: default_tools_approval_mode = \"prompt\", not the \"approve\" install writes",
                )
            }),
            "{:?}",
            report.findings
        );
    }

    /// An enabled plugin whose files exist is the registration for Claude
    /// Code on its own; beside a direct registration it is the second of
    /// two, and only then is the double-recording warning given. The same
    /// predicate refuses a direct install beside it.
    #[test]
    fn an_enabled_plugin_registers_alone_and_warns_only_beside_a_direct_registration() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(directory.path());
        let plugin_dir = directory.path().join("plugin-files");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::create_dir_all(&paths.claude_plugins).unwrap();
        std::fs::write(
            &paths.claude_settings,
            r#"{"enabledPlugins": {"commonmeasure@commonmeasure": true}}"#,
        )
        .unwrap();
        std::fs::write(
            paths.claude_plugins.join("installed_plugins.json"),
            format!(
                r#"{{"version": 2, "plugins": {{"commonmeasure@commonmeasure": [{{"scope": "user", "installPath": "{}", "version": "0.3.0"}}]}}}}"#,
                plugin_dir.display()
            ),
        )
        .unwrap();

        let report = doctor_claude(&paths);
        assert!(report.registered, "{:?}", report.findings);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.text.contains("this is the registration")),
            "{:?}",
            report.findings
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.text.contains("recorded twice")),
            "{:?}",
            report.findings
        );

        // A direct MCP registration written beside it: now two routes.
        std::fs::write(
            &paths.claude_state,
            r#"{"mcpServers": {"commonmeasure": {"command": "/usr/bin/commonmeasure", "args": ["mcp", "--host", "claude-code"]}}}"#,
        )
        .unwrap();
        let report = doctor_claude(&paths);
        assert!(report.registered);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.text.contains("recorded twice")),
            "{:?}",
            report.findings
        );

        // install refuses on the same predicate the doctor reports on; with
        // the plugin's files gone it does not load, doctor says so, and a
        // direct install proceeds.
        let binary = directory.path().join("commonmeasure");
        std::fs::write(&binary, b"").unwrap();
        std::fs::remove_file(&paths.claude_state).unwrap();
        let error = install_claude(&binary, &paths).expect_err("refuses beside a loading plugin");
        assert!(
            error.contains("claude plugin disable commonmeasure@commonmeasure"),
            "{error}"
        );
        std::fs::remove_dir_all(&plugin_dir).unwrap();
        let report = doctor_claude(&paths);
        assert!(!report.registered, "{:?}", report.findings);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.text.contains("loads nothing")),
            "{:?}",
            report.findings
        );
        install_claude(&binary, &paths).expect("installs beside a plugin that does not load");
    }
}
