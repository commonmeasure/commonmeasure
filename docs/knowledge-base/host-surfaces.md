---
title: Host surfaces
draft: true
---

# Host surfaces

The investigation `ROADMAP.md` §The other hosts asks for: one section per
host, stating how the host is configured with an MCP server, whether it has
lifecycle hooks and which events carry a tool result, what session identity
it exposes, whether an extension API can observe tool calls where hooks do
not exist, how the host identifies itself, and whether the mediated tools
(`context_fetch`, `context_search`, `context_status`) work through it
unchanged. The closing section ranks the hosts for integration.

Every finding carries one of three grades, never collapsed upwards:

- `live-verified`: run on one macOS machine on 13 September 2026 with the
  v0.3.0 edge installed by the public installer into `~/.local/bin`; the
  file or output is quoted.
- `spec-verified`: read in the host's published documentation, cited by
  URL; every page was read on 13 September 2026, and where the page carries
  its own date that date is given too.
- `planned`: the interface is identified and nothing is proven.

The two ways a host reaches the runtime are
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §1:
the **mediated** path (the MCP server, which can refuse before content
moves) and the **observed** path (a hook that runs after the host's own
tool call and can only record). "Mediated only" below means the host offers
no hook that carries a tool result; "observed too" means it does.

## How the live probes were run

- The v0.3.0 binary was installed with the installer command the release
  notes state, into `~/.local/bin/commonmeasure`, checksum verified.
- Each registration was written into the host's own configuration, either
  by hand or with the host's own command, and removed afterwards. Where the
  product already has a subcommand for the host (`install codex`), that was
  used, and `uninstall codex` restored the file byte for byte.
- To learn how each host names itself, the registered command was a short
  shell script that copies what the MCP client sends into a log and then
  relays the stream to the real binary. The first message a client sends is
  the protocol's `initialize` request, whose `clientInfo` field is the
  client's own name and version. Those fields are quoted verbatim below.
- No probe changed the operator home. The session logs the probes left in
  `~/.commonmeasure/sessions/` hold a `credentials_loaded` record each and,
  for the one Codex run that completed a call, a `crossing_mediated` record.

One fact about the runtime frames every section. The `host` field in
session evidence is the value of the `--host` argument the registration
passes to `commonmeasure mcp`, which defaults to `claude-code`; the probes
below were run before the argument accepted more than `claude-code`, `codex`
and `pi`, so the Claude Desktop probe recorded `host: claude-code`. The
server now accepts `claude-desktop`, `cursor`, `copilot-cli` and `vscode`
as well, refuses any other value with the list, and records the client's
`clientInfo` name and version at `initialize` on the session and on each
mediated crossing ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Client identity). The section on each host says what the client sends, so
the value a future `install <host>` passes and the name the record shows
are both exact.

## Summary

| Host | Registration surface | Hooks with a tool result | Session id to the server | Client name sent in `initialize` | Mediated tools | Verdict |
|---|---|---|---|---|---|---|
| Claude Desktop | `~/Library/Application Support/Claude/claude_desktop_config.json`, `mcpServers`, stdio | none | none (server generates one) | `claude-ai` 0.1.0 and `local-agent-mode-<server>` 1.0.0 | listed, `live-verified`; a call `planned` | mediated only |
| Claude in the browser, Claude in Chrome | account-level remote connectors only | none | not applicable | not probed | needs an HTTPS edge, `planned`; Claude Desktop's bridge announces the local tools and a browser chat cannot call them, `live-verified` | out of reach of the local binary |
| Cursor | `~/.cursor/mcp.json` and `<project>/.cursor/mcp.json`, `mcpServers`, stdio or HTTP | `afterMCPExecution`, `postToolUse`, `afterShellExecution` (`spec-verified`) | none to the server; `session_id` to hooks | `cursor-vscode` 1.0.0 | listed, `live-verified`; a call `planned` | observed too |
| VS Code without Copilot | user `mcp.json` and `.vscode/mcp.json`, `servers`, stdio or HTTP | only through the Copilot Chat extension | none to the server; `sessionId` to hooks | not captured | registered by `install vscode`, read by VS Code `live-verified`; start and call `planned` | mediated only |
| Copilot CLI | `~/.copilot/mcp-config.json`, `mcpServers`, `type: local` or `http` | `postToolUse` with `textResultForLlm`, including `web_fetch` and `web_search` (`spec-verified`) | none to the server; `sessionId` to hooks | not captured: the client refused the server before starting it | registered by `install copilot`, read by the CLI `live-verified`; a call `planned` | observed too |
| Copilot agent mode in VS Code | the VS Code `mcp.json`, forwarded to the Agent Host, or `~/.copilot/mcp-config.json` | Copilot Chat hooks | as VS Code | not captured | `spec-verified` | observed too, subject to subscription |
| Copilot cloud agent on GitHub | repository settings JSON, `mcpServers` | `.github/hooks/*.json` in the job sandbox | `sessionId` per job | not applicable | needs an edge inside the job or hosted | out of reach of the local binary |
| Microsoft 365 Copilot | Copilot Studio connector or declarative-agent plugin, Streamable HTTP URL | none | not applicable | not probed | needs an HTTPS edge | out of reach of the local binary |
| Codex CLI | `~/.codex/config.toml`, `[mcp_servers.<name>]`, stdio or HTTP | `PostToolUse` with `tool_response` for MCP tools (`spec-verified`); hosted web search fires none | none to the server; `session_id` and `turn_id` to hooks | `codex-mcp-client`, title `Codex`, 0.154.0 | one fetch recorded, `live-verified` | mediated first |
| ChatGPT desktop app, Codex mode | the same `~/.codex/config.toml` | the same Codex hooks | as Codex CLI | `codex-mcp-client`, title `Codex`, 0.153.4 | listed, `live-verified`; a call `planned` | mediated first |
| Codex IDE extension | the same `~/.codex/config.toml` | the same Codex hooks | as Codex CLI | not captured | `spec-verified`, and the bundle reads the file | mediated first |
| ChatGPT (web, and the chat mode of the desktop app) | developer-mode apps and plugins, remote MCP only | none | not applicable | not probed | needs an HTTPS edge | out of reach of the local binary |

## Claude Desktop

**Configuration.** Local servers are declared in
`~/Library/Application Support/Claude/claude_desktop_config.json` under the
key `mcpServers`, one object per server with `command`, `args` and optional
`env`. The file is per macOS user; there is no project scope. The only
transport this file carries is stdio: a remote server is added instead as an
account-level "custom connector", which is a different mechanism (below).
Servers can also arrive as desktop extensions, `.mcpb` packages installed
from Settings > Extensions, which wrap a stdio server with a manifest and
are subject to an enterprise allowlist. `spec-verified`:
<https://modelcontextprotocol.io/docs/develop/connect-local-servers> (the
protocol's own quickstart, version 2026-07-28 of the documentation) and
<https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop>
(page dated 30 June 2026).

**Live probe.** With the v0.3.0 binary installed, the file was given this
entry and the application restarted:

```json
{ "mcpServers": { "commonmeasure": { "command": "~/.local/bin/commonmeasure", "args": ["mcp"] } } }
```

(the command was written as the absolute path of that file; the application
does not expand `~`). Claude Desktop writes two logs under
`~/Library/Logs/Claude/`. `mcp.log` recorded:

```
[commonmeasure] Server started and connected successfully
[commonmeasure] Message from client: method="initialize" id=0 params
[commonmeasure] Message from server: id=0 result
[commonmeasure] Message from client: method="notifications/initialized"
[commonmeasure] Message from client: method="tools/list" id=1 params
[commonmeasure] Message from server: id=1 result
```

`mcp-server-commonmeasure.log` holds the server's own standard error:

```
commonmeasure: credentials from ~/.commonmeasure/credentials.env (15 applied, 0 shadowed by the environment)
commonmeasure: mediating session local-1789349125214-33722, recording to ~/.commonmeasure/sessions/local-1789349125214-33722.ndjson
```

Facts read from the running process and the probe log:

- Claude Desktop launched the server through its own helper, as
  `Claude.app/Contents/Helpers/disclaimer --pgroup -- <command> mcp`, with
  the working directory `/`. The policy scope therefore resolves against
  `/`, so no directory-keyed scope applies to a Claude Desktop session, and
  the crossing's `cwd` is `/`.
- It started the server twice per launch. The two clients name themselves
  differently in `initialize` (protocol version `2025-11-25` in both):

  ```
  "clientInfo":{"name":"local-agent-mode-commonmeasure","version":"1.0.0"}
  "clientInfo":{"name":"claude-ai","version":"0.1.0"}
  ```

  The first is the local agent mode (the Cowork and Code sessions the
  desktop hosts); the second is the chat client. Each connection is a
  separate server process with its own generated session id; the binary
  of that probe wrote the session file at start, so the launch left two
  logs, where the current binary writes it at the first call and a server
  asked for nothing leaves none.
- No session identifier is passed to the server; it generated
  `local-<millis>-<pid>`. The session evidence carries `host: claude-code`,
  the default, because the file passes no `--host`.
- The application's own log, `main.log`, recorded `Launching MCP Server:
  commonmeasure` and, on quit, `Shutting down MCP Server: commonmeasure`.

Grade: registration, server start and tool listing `live-verified`. A tool
call through the chat was not exercised: sending a message needs a person at
the keyboard, and scripted keystrokes are refused by macOS unless the
terminal has the Accessibility permission. The `claude://claude.ai/new?q=…`
deep link prefills the prompt but does not send it (`spec-verified`:
<https://support.claude.com/en/articles/14729294-open-claude-desktop-with-a-link>,
dated 30 June 2026). A mediated call from Claude Desktop is `planned` until
one message is sent with the entry in place.

**Hooks.** None. Claude Desktop documents no lifecycle hooks, and no hook
surface exists in its configuration file. Its web search is a hosted tool
with no local event. **Extension API.** None that observes tool calls:
desktop extensions are servers, not observers. **Verdict:** mediated only.

**Integration.** `commonmeasure install claude-desktop` writes the one
entry with `mcp --host claude-desktop`; `uninstall` removes it and
`doctor` reads it back (`crates/commonmeasure-cli/tests/install_e2e.rs`).
With that entry written by the binary, a restart of the application
recorded in `mcp.log` `Server started and connected successfully`, the
`initialize` exchange and `tools/list`, and two processes `commonmeasure
mcp --host claude-desktop` ran, one per client. Each server that makes a
call leaves its own session carrying its client's name, and they are not
merged, because each server process is its own witness; a server asked
for nothing leaves no file, and this launch left none. A mediated call
through the application is `planned` until a person sends one message in
it. The
contract grades the host in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §6.

**Self-identification.** `clientInfo.name` is `claude-ai` for the chat and
`local-agent-mode-<server name>` for the local agent mode. The record's
`host` must come from the registration, so the integration needs an accepted
value for this host (`claude-desktop` fits the existing pattern), and the two
clients should be told apart by `clientInfo` if the server ever records it.

**Stability.** The file, its key and the `command`/`args`/`env` shape are
the ones the protocol's quickstart has documented since the protocol was
published; the support article dated 30 June 2026 adds desktop extensions
beside the file without changing it. `spec-verified` from the two pages
cited above; no dated changelog of the file format was found.

### Claude in the browser and Claude in Chrome

The desktop file does not serve the browser. Custom connectors are the only
way the web app reaches an MCP server, and the connection "originates from
Anthropic's servers, not from your machine's network interface", across
every Claude client including Claude Desktop and Cowork; a server "must be
reachable over the public internet from Anthropic's IP ranges"; and "Local
MCP servers configured in Claude Desktop via claude_desktop_config.json are
a separate mechanism and do use your local network, but those aren't
available in Cowork or claude.ai". Advanced research cannot invoke local
servers at all. `spec-verified`:
<https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp>
(dated 11 August 2026).

One live observation points the other way and is recorded as a lead, not a
result. After the entry above was in place, `main.log` recorded:

```
[localMcpBridge] announcing commonmeasure: 3 tool(s)
[remote-tools-device] connecting DO bridge with: … (+5 grand-prix, +3 local-mcp, +0 direct-mcp)
```

and, with the entry removed, `[localMcpBridge] no stdio servers connected —
announcing an empty local-MCP group`. The desktop announces a local stdio
server's tools to the same device bridge that exposes its computer-use and
browser tools to cloud sessions. Grade: the log lines `live-verified`.
Whether a browser chat can call the tools is the probe below.

Claude in Chrome is a browser extension whose side panel runs as a Cowork
session in the cloud; its `nativeMessaging` permission is reserved "to
integrate with other Anthropic products on your computer like Claude
Desktop or Claude Code once we enable that capability". It has no MCP
configuration of its own. `spec-verified`:
<https://support.claude.com/en/articles/12012173-getting-started-with-claude-in-chrome>
(page says updated within two weeks of reading).

**Probe.** With `install claude-desktop` in place and the desktop running,
`main.log` recorded `[localMcpBridge] announcing commonmeasure: 3 tool(s)`
twice. A claude.ai chat in Chrome on the same account, with web search off,
answered that it had no `context_fetch` tool and no `commonmeasure` server,
and listed only the Google connectors. The bridge announces the local
server's tools and does not carry calls to a browser chat. `live-verified`
on one operator machine, 14 September 2026.

**Verdict:** the browser and the extension need the mediated tools served
over HTTPS by an edge that Anthropic's cloud can reach; the desktop bridge
does not carry calls (above), and the local binary as it stands serves
neither. The hosted edge is designed in
[`docs/knowledge-base/hosted-edge.md`](hosted-edge.md); custom connectors
take client metadata documents and dynamic client registration, register
the redirect `https://claude.ai/api/mcp/auth_callback` and reach servers
from `160.79.104.0/21` (`spec-verified`:
<https://claude.com/docs/connectors/building/authentication> and
<https://platform.claude.com/docs/en/api/ip-addresses>, read 14 September
2026).

## Cursor

**Configuration.** `~/.cursor/mcp.json` for every project and
`<project>/.cursor/mcp.json` for one project, key `mcpServers`. A stdio
server has `type: "stdio"`, `command`, `args`, `env` and optional
`envFile`; a remote server has `url` with `headers` and optional static
OAuth. Values may use `${env:NAME}`, `${userHome}`, `${workspaceFolder}`.
Cursor also has an extension API for programmatic registration,
`vscode.cursor.mcp.registerServer()`. Enterprise administrators can
allowlist servers by command or URL pattern and set a per-server network
mode. `spec-verified`: <https://cursor.com/docs/mcp>.

**Live probe.** Cursor 3.20.17 was installed with `brew install --cask
cursor`. `~/.cursor/mcp.json` was written with the stdio entry above (the
probe script as `command`, `["mcp"]` as `args`) and Cursor opened for the
first time, with no account signed in. Cursor started the server twice
during that launch. Both clients sent, protocol `2025-11-25`:

```
"clientInfo":{"name":"cursor-vscode","version":"1.0.0"}
```

followed by `tools/list`. Cursor's log at
`~/Library/Application Support/Cursor/logs/<timestamp>/mcp-server-user-commonmeasure.log`
recorded:

```
connecting stdio for "commonmeasure" (user-commonmeasure)
MCP stdio spawn policy decision: sandboxed=false, sandboxReason=controls_disabled, networkControlsEnabled=false, …
commonmeasure: mediating session local-1789349435814-46919, recording to ~/.commonmeasure/sessions/local-1789349435814-46919.ndjson
```

Grade: registration, start and tool listing `live-verified` before sign-in.
A call needs a signed-in account and a chat; `planned`.

**Hooks.** Cursor has lifecycle hooks in `hooks.json` at user
(`~/.cursor/hooks.json`), project (`<project>/.cursor/hooks.json`),
enterprise (`/Library/Application Support/Cursor/hooks.json` on macOS) and
team (dashboard-distributed) levels, `"version": 1`, reloaded on save. Hook
processes receive JSON on stdin and answer on stdout; exit code 2 blocks.
Events and what they carry, `spec-verified`: <https://cursor.com/docs/hooks>:

- Common input to every hook: `conversation_id`, `generation_id`,
  `hook_event_name`, `cursor_version`, `workspace_roots`,
  `transcript_path`, `user_email`, `model`.
- `afterMCPExecution`: `tool_name`, `tool_input`, `mcp_server_name`,
  `result_json` (the full tool result as a JSON string), `duration`. This
  carries the fetched content of any MCP fetch tool.
- `postToolUse`: `tool_name`, `tool_input`, `tool_output` (the result as a
  JSON string), `tool_use_id`, `cwd`, `duration`; fires for every tool;
  matcher values are `Shell`, `Read`, `Write`, `Grep`, `Delete`, `Task` and
  `MCP:<tool name>`.
- `afterShellExecution`: `command`, `output` (the full terminal output),
  `duration`.
- `beforeSubmitPrompt`: `prompt` and attachments. `sessionStart`:
  `session_id` (equal to `conversation_id`), `is_background_agent`,
  `composer_mode`. `sessionEnd`, `stop`, `preCompact`,
  `afterAgentResponse`, `afterAgentThought`, `subagentStart`,
  `subagentStop`, `postToolUseFailure`.
- Cursor loads Claude Code hook files as third-party hooks, and sets
  `CLAUDE_PROJECT_DIR` beside `CURSOR_PROJECT_DIR`.

Cursor's own agent has a web tool ("Generate search queries and perform web
searches", <https://cursor.com/docs/agent/overview>). Its result reaches a
hook through the generic `postToolUse`, but the documented matcher values do
not name it, so whether a web search result is observable with content is
`planned` until a hook is run against it. Cloud agents run project hooks
only, never `~/.cursor/hooks.json`, and skip `afterMCPExecution`.

**Session identity.** Hooks receive `conversation_id`, `generation_id` and
`session_id`; nothing is passed to the MCP server, whose configuration
supports no session placeholder, so the server generates its own id. A hook
record and a mediated record from one Cursor session cannot share a log
today.

**Self-identification.** `clientInfo.name` `cursor-vscode`, `version`
`1.0.0` (not the application version); hooks carry `cursor_version`. A
`--host cursor` value is the exact name for the record.

**Verdict:** mediated tools by a config write alone, and observed crossings
through `afterMCPExecution` and `postToolUse`; observed too.

**Integration.** `commonmeasure install cursor` writes the server into
`~/.cursor/mcp.json` and four hooks into `~/.cursor/hooks.json`
(`sessionStart`, `postToolUse`, `beforeSubmitPrompt`, `stop`, each running
`hook <event> --host cursor`); the hook command reads Cursor's payload
shape, records under `conversation_id`, excludes our own tools under
Cursor's `MCP:<tool>` spelling, answers Cursor in JSON and delivers the
nudge as `additional_context`
(`crates/commonmeasure-cli/tests/install_e2e.rs`,
`crates/commonmeasure-cli/tests/hook_e2e.rs`). Cursor's built-in web tool
is not named in the documented matcher values, so its results are not
claimed as observed until a hook has run against one. Because Cursor can
load `~/.claude/settings.json` as third-party hooks (opt-in, under
Settings, "include third-party plugins"), every hook command `install
claude` writes says `--host claude-code`, and a reader told that refuses
Cursor's and the Copilot family's payload shapes. Cursor's third-party
hooks page does not say whether those hooks receive Cursor's shape or a
Claude Code translation, so the field names are not the whole test: the
reader also refuses any payload when `CURSOR_PROJECT_DIR`, which Cursor
sets for every hook it runs, is in the environment. Cursor's observed path
is `spec-verified` on the documented shapes, because no payload recorded
from Cursor exists; a mediated fetch and an observed one through Cursor
are `planned` until a signed-in Cursor runs a chat. The contract grades
the host in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §6.

**Stability.** The changelog (<https://cursor.com/changelog>) lists five
product releases between 17 August and 10 September 2026; none changes
`mcp.json` or `hooks.json`. The hooks file carries a `version` field, and
the hooks page documents fields that were added without renaming earlier
ones (`model_id`, `model_params`, `workspaceOpen`). The application
version advances roughly weekly.

## VS Code without Copilot

**Configuration.** `mcp.json` with a top-level `servers` object (not
`mcpServers`), an optional `inputs` array and an optional `sandbox` object.
It lives in the user profile or in the workspace as `.vscode/mcp.json`. A
stdio server has `type: "stdio"`, `command`, `args`, `cwd`, `env`,
`envFile`, `sandboxEnabled`; a remote server has `type: "http"` or `"sse"`,
`url`, `headers`, `oauth`. Servers run wherever they are configured (locally
for the user profile, on the remote for a remote workspace). Dev containers
declare servers under `customizations.vscode.mcp`. The command line accepts
`code --add-mcp '<json>'`. `spec-verified`:
<https://code.visualstudio.com/docs/agent-customization/mcp-servers> and
<https://code.visualstudio.com/docs/agents/reference/mcp-configuration>
(both dated 9 September 2026 on the page).

**Live probe.** VS Code 1.137.0 was installed with `brew install --cask
visual-studio-code`. Running

```
code --add-mcp '{"name":"commonmeasure","command":"~/.local/bin/commonmeasure","args":["mcp"]}'
```

(with the absolute path) answered `Added MCP servers: commonmeasure` and
created `~/Library/Application Support/Code/User/mcp.json`:

```json
{
	"servers": {
		"commonmeasure": {
			"command": "~/.local/bin/commonmeasure",
			"args": ["mcp"]
		}
	},
	"inputs": []
}
```

VS Code did not start the server on launch; servers start on first use in a
chat. Grade: the user-scope file and its shape `live-verified`; start and
call `planned`.

**Who reads the file.** VS Code itself: the MCP service behind the `MCP:
List Servers` commands and the "Local" harness, which "can use VS Code
built-in tools, extension-provided tools, MCP servers, and the models
configured in VS Code, including bring your own key models". The Copilot
harness runs in the Agent Host, to which VS Code forwards the configured
servers except those needing `${input:…}`; the reference recommends a
workspace `.mcp.json` or the user `~/.copilot/mcp-config.json`, "which the
Agent Host reads natively", for portable configuration. The Claude and Codex
harnesses run "on your machine" with provider-specific capabilities; whether
they read `~/.claude.json` or `~/.codex/config.toml` inside VS Code is not
stated: `planned`. `spec-verified`:
<https://code.visualstudio.com/docs/agents/run/agent-harnesses> and
<https://code.visualstudio.com/docs/agents/concepts/agent-harnesses>.

Third-party agent extensions do not read it. Each keeps its own file,
`spec-verified` from its documentation:

- Cline: `~/.cline/mcp.json` for the CLI and a settings JSON opened from the
  extension's MCP panel, key `mcpServers`
  (<https://docs.cline.bot/mcp/mcp-overview>).
- Continue: `.continue/mcpServers/*.yaml`, and it also picks up a JSON file
  in the `mcpServers` shape dropped into that directory
  (<https://docs.continue.dev/customize/deep-dives/mcp>).
- Roo Code: a global `mcp_settings.json` and a project `.roo/mcp.json`, key
  `mcpServers`
  (<https://roocodeinc.github.io/Roo-Code/features/mcp/using-mcp-in-roo/>,
  last updated 15 May 2026).

One registration therefore serves VS Code's own harnesses and, forwarded,
the Copilot harness; it serves none of these three extensions.

**Hooks.** Agent hooks are in preview: `.github/hooks/*.json` in the
workspace, and by default also `.claude/settings.json`,
`.claude/settings.local.json` and `~/.claude/settings.json` (the setting
`chat.hookFilesLocations`). Events: `PreToolUse`, `PostToolUse`,
`UserPromptSubmit`, `SessionStart`, `Stop`, `SubagentStart`, `SubagentStop`,
`PreCompact`. Input carries `sessionId`, `timestamp`, `tool_name` and
`tool_input` with camelCase property names; matchers in Claude Code files
are parsed but ignored, so every hook fires for every tool. The reference
documents `PostToolUse` output fields but not whether its input carries the
tool result: `planned`. The diagnostics live in the "GitHub Copilot Chat
Hooks" output channel, so the hooks are the Copilot Chat extension's;
without that extension there is no hook surface. `spec-verified`:
<https://code.visualstudio.com/docs/agent-customization/hooks> and
<https://code.visualstudio.com/docs/agents/reference/hooks-reference>
(dated 9 September 2026).

Two consequences for the existing Claude Code registration: VS Code with
Copilot Chat loads the four hooks `install claude` writes to
`~/.claude/settings.json`, ignores their matchers, and calls the binary with
VS Code payloads whose tool names it does not classify, so no crossing is
written but every VS Code tool call spawns the hook; and Cursor loads the
same file. An integration for either host must decide whether the Claude
Code hooks are meant to fire there.

**Extension API.** `vscode.lm.registerMcpServerDefinitionProvider` with the
`contributes.mcpServerDefinitionProviders` contribution point lets an
extension register servers. No API observes other extensions' or built-in
tools' calls. `spec-verified`:
<https://code.visualstudio.com/api/extension-guides/ai/mcp> (dated 9
September 2026).

**Self-identification.** Not captured, because the server was not started.
The name VS Code sends in `initialize` is not documented: `planned`.

**Verdict:** mediated tools by a config write alone (the Local harness with
a configured model, or the Copilot harness); observed crossings only with
the Copilot Chat extension. Without Copilot: mediated only.

**Integration.** `commonmeasure install vscode` writes one
`servers.commonmeasure` entry (`type: stdio`, `mcp --host vscode`) into the
user `mcp.json` directly, not through `code --add-mcp`, which cannot remove
what it adds and needs the `code` command on `PATH`; a file with comments
is refused rather than rewritten without them
(`crates/commonmeasure-cli/tests/install_e2e.rs`). No hook is registered:
the hooks are Copilot Chat's, load Claude Code's hook files with matchers
ignored, and are not documented to carry a tool result, so the decision is
left open in `ROADMAP.md` until a session with Copilot shows the payload.
The snake_case payload those hooks send carries Claude Code's field names
beside a `timestamp` Claude Code never sends, and the Claude Code reader
refuses it on that field (`crates/commonmeasure-cli/tests/hook_e2e.rs`).
Live, on 14 September 2026: with the entry written by `install vscode`,
opening VS Code 1.137.0 created the server's output log,
`logs/<timestamp>/window1/mcpServer.mcp.config.usrlocal.commonmeasure.log`
under VS Code's application data, empty because no chat started the
server. Grade: registration `fixture-tested`, and read by VS Code
`live-verified` on that machine; start and call `planned`, needing a chat
with a model. The contract grades the host in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §6.

**Stability.** VS Code releases monthly (1.137.0 read on 13 September
2026). The MCP configuration reference documents `servers`, `inputs` and
`sandbox` without deprecations; the hooks page states the format "might
change in future releases" while in preview.

## GitHub Copilot

**Entitlement.** Every Copilot surface needs an active Copilot plan on the
GitHub account the client is signed in to; the client refuses an MCP
server before starting it when the plan does not allow MCP. The probes
below stopped at that boundary, so each Copilot call is `planned` and an
active plan is the act that unblocks it.

### Agent mode in VS Code

The Copilot harness "is powered by the Copilot SDK and runs locally on your
machine in the Agent Host", which owns the session independently of the
window. MCP servers come from VS Code's `mcp.json` (forwarded) or from
`~/.copilot/mcp-config.json` (read natively). Hooks are the VS Code agent
hooks of the previous section. `spec-verified`:
<https://code.visualstudio.com/docs/agents/run/agent-harnesses>. Whether
the harness's `PostToolUse` input carries the tool result is `planned`; the
Copilot CLI's "VS Code compatible" payload (below) does, which suggests the
extension's does, but the VS Code reference does not say so.

### Copilot CLI

**Configuration.** User file `~/.copilot/mcp-config.json`, key
`mcpServers`; a local server has `type: "local"` (or `"stdio"`, treated the
same), `command`, `args`, `env`, `tools`; a remote one `type: "http"` or
`"sse"`, `url`, `headers`. Workspace files `.mcp.json` (any directory from
the working directory up to the repository root) and `.github/mcp.json`,
loaded after folder trust. Plugins contribute servers. VS Code's
`.vscode/mcp.json` is not read, because its top-level key is `servers`.
`copilot mcp add <name> --env K=V -- <command> [args]` writes the user file.
`spec-verified`:
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers>.

**Live probe.** Copilot CLI 1.0.83 was installed with `brew install --cask
copilot-cli`. `copilot mcp --help` states the sources: "User
`~/.copilot/mcp-config.json`; Workspace `.mcp.json` or `.github/mcp.json`;
Plugin". `copilot mcp add commonmeasure --env MCP_TEE_LABEL=copilot-cli --
<command> mcp` created `~/.copilot/mcp-config.json`:

```json
{
  "mcpServers": {
    "commonmeasure": {
      "tools": ["*"],
      "type": "local",
      "command": "<the probe script>",
      "args": ["mcp"],
      "env": { "MCP_TEE_LABEL": "copilot-cli" }
    }
  }
}
```

and `copilot mcp list` printed `commonmeasure (local)`. A one-shot prompt,
`copilot -p "…" --allow-all-tools`, answered:

```
! 1 MCP server was blocked by policy: 'commonmeasure'
Error: Access denied by policy settings (Request ID: …)
Your Copilot CLI policy setting may be preventing access.
```

The server was never started, so no `initialize` was captured. Grade: the
registration surface `live-verified`; a call `planned` until an account
whose plan allows MCP runs one. `copilot mcp remove commonmeasure` removed
the entry.

**Hooks.** Copilot CLI has lifecycle hooks with a documented payload,
`spec-verified`: <https://docs.github.com/en/copilot/reference/hooks-reference>.

- Locations, loaded in order and combined: policy files
  (`/etc/github-copilot/policy.d/*.json`, root-owned), repository
  `.github/hooks/*.json`, user `~/.copilot/hooks/*.json` (or
  `$COPILOT_HOME/hooks/`), inline `hooks` in `.github/copilot/settings.json`
  and `~/.copilot/settings.json`, the repository's `.claude/settings.json`,
  and plugins. Files carry `"version": 1`.
- Events: `sessionStart`, `sessionEnd`, `userPromptSubmitted`,
  `userPromptTransformed`, `preToolUse`, `permissionRequest`, `postToolUse`,
  `postToolUseFailure`, `agentStop`, `subagentStart`, `subagentStop`,
  `preCompact`, `errorOccurred`, `notification`.
- `postToolUse` input: `sessionId`, `timestamp`, `cwd`, `toolName`,
  `toolArgs`, `toolResult: { resultType: "success", textResultForLlm }`.
  The same event configured under the PascalCase name `PostToolUse` uses
  snake_case fields, `tool_name`, `tool_input`, `tool_result:
  { result_type, text_result_for_llm }`, "to match the VS Code Copilot
  extension format", and applies Claude Code matcher semantics with tool
  names mapped: `web_fetch` is `WebFetch`, `web_search` is `WebSearch`,
  `bash` is `Bash`, `task` is `Agent`.
- `web_fetch` and `web_search` are local tools in the hook path, so their
  results are observable with content. This is the one host beyond Claude
  Code where the built-in fetch is observable.
- Handlers: `bash`, `powershell`, `command`, `exec` with `args`; HTTP
  handlers posting the payload to an `https://` URL; prompt handlers on
  `sessionStart`. Exit code 2 denies on `preToolUse`; timeouts fail open.

**Session identity.** Hooks carry `sessionId` and `cwd`; nothing is passed
to the MCP server. **Self-identification.** Not captured. `hook_event_name`
in the PascalCase payload and the `~/.copilot` directory identify the CLI; a
`--host copilot-cli` value is the exact name.

**Verdict:** mediated tools by a config write alone, and observed crossings
with content through `postToolUse`; observed too, once the account allows
MCP.

**Integration.** `commonmeasure install copilot` writes the server into
`~/.copilot/mcp-config.json` (`type: local`, `mcp --host copilot-cli`,
`tools: ["*"]`) and four hooks into `~/.copilot/hooks/commonmeasure.json`
under the camelCase names (`sessionStart`, `postToolUse` with the matcher
`web_fetch|web_search`, `userPromptSubmitted`, `agentStop`), each running
the binary through `exec` with `hook <event> --host copilot-cli`. The hook
command reads the camelCase payload under `sessionId`, hashes a `web_fetch`
over `textResultForLlm`, records `web_search` results as retrieved, and
answers `sessionStart` with the nudge as `additionalContext`
(`crates/commonmeasure-cli/tests/install_e2e.rs`,
`crates/commonmeasure-cli/tests/hook_e2e.rs`). The CLI's installed
JavaScript bundle declares
`web_fetch`'s arguments as `url`, `raw`, `max_length` and `start_index`,
names MCP tools `<server>-<tool>` (`github-mcp-server-web_search`), and also
carries a hosted web search for one model provider, which would not reach a
local hook. Live, on 14 September 2026, with the Copilot CLI 1.0.83 and the
registration `install copilot` wrote: `copilot mcp list` printed `User
servers: commonmeasure (local)`, and `copilot -p "…" --allow-all-tools`
answered:

```
! Third-party MCP servers are disabled by your organization's Copilot policy. Only built-in servers
  are available.

! 1 MCP server was blocked by policy: 'commonmeasure'

Error: Access denied by policy settings (Request ID: E6B1:16ED0E:13A2D68:153908F:6AA826BD)
```

The CLI's log recorded `Error loading models: Error: 403 "unauthorized: not
authorized to use this Copilot feature"`; no hook ran and no session record
was written. `uninstall copilot` removed both entries. Grade: registration
`fixture-tested`, and read by the CLI `live-verified`; observed
`spec-verified` on the documented shapes; a call `planned`, needing an
account whose plan allows MCP. The contract grades each surface in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §6.

### The GitHub Copilot app

A native client (Homebrew cask `github-copilot-app`, 1.1.20, not
installed). "Any MCP servers configured for your repositories or Copilot
CLI are automatically available in the GitHub Copilot app", and more can be
added in its Customize tab. One `~/.copilot/mcp-config.json` entry therefore
serves the CLI, the app and, natively, VS Code's Agent Host.
`spec-verified`:
<https://docs.github.com/en/copilot/how-tos/github-copilot-app/customize-github-copilot-app>
and <https://docs.github.com/en/copilot/concepts/context/mcp>.

### The cloud agent on GitHub

Runs where no edge is. MCP servers are configured per repository in its
settings (Copilot > MCP servers) as JSON with `mcpServers`, `type: "local"`,
`"stdio"`, `"http"` or `"sse"`, a required `tools` list, and environment
from secrets named `COPILOT_MCP_*`; the same configuration serves Copilot
code review. Hooks are `.github/hooks/*.json` on the default branch; only
the `bash` handler is honoured; the job runs in an ephemeral Linux sandbox
with `/workspace` as the working directory, an outbound firewall that
reaches GitHub and Copilot hosts unless an administrator allows more, and a
filesystem discarded at the end of the job. `spec-verified`:
<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/configure-mcp-servers>,
<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/use-hooks>
and the hooks reference above.

A `type: "local"` server can be a Linux binary the job installs in its
`copilot-setup-steps.yml`, so the edge could run inside the sandbox; but its
policy file would have to be provisioned per job, its evidence would die
with the job unless relayed through an allowed host, and there is no
operator home. That is a hosted-edge design, not the local product. Out of
reach of the local binary as it stands.

### Microsoft 365 Copilot

Reaches an MCP server only over the network. In Copilot Studio a server is
added by URL with the Streamable HTTP transport (SSE dropped after August
2025), with none, API-key or OAuth 2.0 authentication, and access "relies on
Power Platform connectors for connectivity"; `spec-verified`:
<https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-add-existing-server-to-agent>
(last updated 28 May 2026) and
<https://learn.microsoft.com/en-us/microsoft-copilot-studio/agent-extend-action-mcp>
(last updated 26 August 2026). A declarative agent built with the Microsoft
365 Agents Toolkit takes "your MCP server URL" and an OAuth callback at
`https://teams.microsoft.com/api/platform/v1.0/oAuthRedirect`;
`spec-verified`:
<https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/build-mcp-plugins>
(last updated 11 August 2026). No local-process path exists. Out of reach of
the local binary; needs an edge over HTTPS.

## Codex beyond the CLI

**One configuration for three surfaces.** "The ChatGPT desktop app, Codex
CLI, and IDE extension support MCP servers and share MCP configuration for
the same Codex host." Servers are `[mcp_servers.<name>]` tables in
`~/.codex/config.toml`, or a project's `.codex/config.toml` in a trusted
project. A stdio server has `command`, `args`, `env`, `env_vars`, `cwd`; a
remote one `url` with bearer, header or OAuth settings. Per server:
`enabled`, `required`, `enabled_tools`, `disabled_tools`,
`default_tools_approval_mode` (`auto`, `prompt`, `writes`, `approve`),
`startup_timeout_sec`, `tool_timeout_sec`. The desktop app and the IDE
extension each have a settings screen that writes the same file. The web
app "doesn't read local Codex configuration files". `spec-verified`:
<https://learn.chatgpt.com/docs/extend/mcp>.

The separate Codex desktop application is superseded: the changelog entry
of 9 July 2026, "Codex joins the ChatGPT desktop app 26.707", states that
Codex "is now part of the ChatGPT desktop app on macOS and Windows", and
the Homebrew cask `codex-app` (26.623) is marked discontinued upstream.
`spec-verified`: <https://learn.chatgpt.com/docs/changelog>. On the probe
machine the ChatGPT application bundles `Contents/Resources/codex`
(reporting `codex-cli 0.153.4`) and a Codex framework; the legacy cask
installs an older bundle (`codex-cli 0.142.5`). `live-verified`.

**Live probe of the CLI, which the other two surfaces share.**
`~/.local/bin/commonmeasure install codex` wrote the table (with the probe
script as the binary):

```toml
[mcp_servers.commonmeasure]
command = "<the probe script>"
args = ["mcp", "--host", "codex"]
```

`codex exec` (Codex CLI 0.154.0) with the default approval policy refused
both tool calls: `MCP tool call requires approval, but approval policy is
never`. An override of `default_tools_approval_mode` on the command line did
not change that. With `--dangerously-bypass-approvals-and-sandbox`, the same
prompt called `context_status`, which answered
`{"session_id":"local-1789349663636-65439","host":"codex"}`, then
`context_fetch` of `https://example.com/`. The session log
`~/.commonmeasure/sessions/local-1789349663636-65439.ndjson` holds one
`credentials_loaded`, three `processor_invoked`, one `manifest_resolved` and
one `crossing_mediated` record, the last with `host: codex`,
`url: https://example.com/`, `http_status: 200`,
`content_hash: sha256:feb057ddba5ac313506909af05c17370808a81030f057346499d045c25534dbf`,
`retrieved_hash: sha256:ff67a9d764d6a2367a187734e697f6a53217db9a21c101d410a113ca871a299d`,
`principal: os-user:501`, `named_by: unknown`, and `cwd` the directory the
command was started in. This is the first recorded Codex session through
the registration; the contract's Codex row (`spec-verified` in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md)
§6) can rest on it. The client sent, protocol `2025-06-18`:

```
"clientInfo":{"name":"codex-mcp-client","title":"Codex","version":"0.154.0"}
```

`uninstall codex` removed the table; Codex itself had added a
`[projects."<directory>"]` trust entry for the probe directory during the
run, which was reverted by hand. Grade: registration and one mediated fetch
`live-verified`.

The approval mode is settled by the values Codex accepts on the written
table under `codex exec` (Codex CLI 0.154.0). `default_tools_approval_mode
= "auto"` is refused: "automatic approval review rejected the
context_status call because it requires approval, but approval policy is
never". `"approve"` completes the call. `install codex` therefore writes
`default_tools_approval_mode = "approve"`, `doctor codex` reports whether
the table carries it, and the read-only tool annotations are not needed.
The server records the client's `clientInfo` from `initialize` before the
first crossing, as a `client_identified` record and as `client` on each
mediated crossing, so a crossing from the CLI (`codex-mcp-client` 0.154.x)
and one from the desktop app (0.153.x) are told apart in the record while
both carry `host: codex` ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Client identity). Each of the three surfaces is graded in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §6.

The session one `codex exec` turn records through the table `install
codex` writes, asked to read `https://example.com` with `context_fetch`, is
committed at `demo/host-sessions/codex/` and read by
`crates/commonmeasure-cli/tests/recorded_sessions.rs`: a `client_identified`
record `{"name":"codex-mcp-client","version":"0.154.0","title":"Codex"}`
with protocol `2025-06-18`, and a `crossing_mediated` record carrying the
same `client` beside `host: codex`, `http_status: 200` and the content hash
the agent quotes back. `commonmeasure session` prints it as `client
codex-mcp-client 0.154.0 via host codex`.

The ChatGPT desktop application and the Codex IDE extension read the same
table, but neither has a recorded session here. The application started
three servers on one launch (below) and none within forty seconds of
another with the same table in place, so its server start depends on the
application's state, and a fetch through it is `planned` until a person
opens a Codex thread in it. The extension starts its Codex application
server on launch (the bundled `codex-cli 0.154.0-alpha.6.2`) and its MCP
servers when a thread is opened; its server start and fetch are `planned`
until a person opens the Codex sidebar and sends one message.

**The ChatGPT desktop app.** With the table in place, opening the ChatGPT
application (26.901.41600) started the server three times before any
interaction, each client sending:

```
"clientInfo":{"name":"codex-mcp-client","title":"Codex","version":"0.153.4"}
```

The version is the bundled binary's, not the CLI's, and one connection
advertised the app's own UI extension (`text/html+skybridge`) and asked for
`resources/list`. So the CLI registration reaches the desktop app:
`live-verified`. A call from the app was not exercised (needs a person):
`planned`. Codex rollout files under `~/.codex/sessions/` record how a
session was started in an `originator` field; the probe recorded the
values `codex-tui`, `codex_exec` and `Codex Desktop` (`live-verified`),
so the host can tell its own surfaces apart even though the MCP client name
is the same.

**The IDE extension.** `openai.chatgpt`, "Codex – OpenAI's coding agent",
for VS Code, Cursor and Windsurf (JetBrains and Xcode have their own
integrations). Version 26.908.40401 installed into VS Code bundles its own
Codex binary under `bin/macos-aarch64` and its extension code joins
`config.toml` under the Codex home directory, so it reads the same
registration: `live-verified` from the installed bundle, not run. Its gear
menu writes MCP servers to that file: `spec-verified`:
<https://learn.chatgpt.com/docs/codex/ide> and the MCP page above.

**Hooks.** Codex hooks apply to every Codex surface. Sources:
`~/.codex/hooks.json`, `<repo>/.codex/hooks.json`, inline `[hooks]` tables
in either `config.toml`, plugins, and managed hooks from `requirements.toml`.
A non-managed hook must be reviewed and trusted (`/hooks`), pinned to the
hash of its definition; the desktop app has an in-app trust review flow
(changelog, 8 May 2026). Events: `SessionStart`, `SessionEnd`,
`PreToolUse`, `PermissionRequest`, `PostToolUse`, `UserPromptSubmit`,
`Stop`, `Interrupt`, `SubagentStart`, `SubagentStop`, `PreCompact`,
`PostCompact`. Common input: `session_id`, `transcript_path`, `cwd`,
`hook_event_name`, `model`; turn hooks add `turn_id`. `PostToolUse` input:
`tool_name` (`Bash`, `apply_patch`, or `mcp__<server>__<tool>`),
`tool_use_id`, `tool_input`, `tool_response` ("MCP tools send the MCP call
result"). Hosted tools such as web search "don't use the local function-tool
hook path". Handlers may be commands or calls to an already-connected MCP
server's tool. `spec-verified`: <https://learn.chatgpt.com/docs/hooks>. A
`~/.codex/hooks.json` written by another product was found on the probe
machine, carrying a `PostToolUse` hook with matcher `^mcp__.*` and
`SessionStart`, `Stop` and `UserPromptSubmit` entries (`live-verified`),
which confirms the file is shared with other tools and must be edited
entry by entry.

So a Codex hook can observe a third-party MCP fetch with its result, and
the shell as command text; the host's own web search stays unobservable,
which is the reading `DECISIONS.md` §Integration and ownership already
records. The runtime excludes its own tools from observation, so a
`PostToolUse` hook on `mcp__commonmeasure__.*` would not double-record.

**Session identity.** Hooks carry `session_id` and `turn_id`; the MCP
server receives none (the table has no session placeholder) and generates
its own. **Self-identification.** `codex-mcp-client`, title `Codex`, with
`version` telling the CLI (0.154.0) from the desktop app (0.153.4); the
rollout `originator`. `--host codex` is the exact value for all three
surfaces; a finer name needs `clientInfo.version` recorded.

**Verdict:** mediated first, through one table that serves the CLI, the
desktop app and the IDE extension; observed crossings for MCP tools and
shell text through hooks; hosted web search unobserved.

**Stability.** The changelog (<https://learn.chatgpt.com/docs/changelog>)
carries about eighty dated entries in the twelve months to 13 September
2026. The `[mcp_servers]` table shape is unchanged across them; hooks
reached general availability on 14 May 2026, gained asynchronous handlers
and MCP-tool handlers on 18 August 2026 and an `Interrupt` event on 26
August 2026; "relative MCP executable paths start more reliably on macOS" on
3 September 2026; the `codex mcp-server` command, Codex acting as a server,
was removed on 5 September 2026, which does not touch a client
registration. A busy surface whose configuration shape has held.

## ChatGPT

Two different things carry the name. The Codex mode of the ChatGPT desktop
application is a local host and is covered in the previous section. ChatGPT
the assistant, on the web and in the chat mode of the desktop application,
reaches MCP only remotely:

- Developer mode: "full Model Context Protocol (MCP) client support for
  all tools", for Pro, Plus, Business, Enterprise and Education accounts on
  the web; an app is created from a remote MCP server; transports SSE and
  streaming HTTP; authentication OAuth, none or mixed; tools without a
  `readOnlyHint` annotation are treated as writes and ask for confirmation.
  `spec-verified`: <https://developers.openai.com/api/docs/guides/developer-mode>.
- Plugins (the successor of the Apps SDK): "ChatGPT web can use remote
  MCP-backed tools supplied by plugins"; "ChatGPT web doesn't read local
  Codex configuration files or expose the local Codex command menu".
  `spec-verified`: <https://learn.chatgpt.com/docs/extend/mcp>.
- Site tools (WebMCP): tools a website provides to the desktop app's
  built-in browser; not a local process. `spec-verified`: the changelog
  entry of 25 August 2026.

No local MCP or local-process path exists for ChatGPT proper. A Common
Measure integration for it would have to be an edge reachable over HTTPS,
serving the mediated tools as a Streamable HTTP MCP endpoint with OAuth or
no authentication, run somewhere other than the operator's machine and
registered as a developer-mode app or a plugin. That is a product decision,
and this page stops there.

## Order

Ranked for integration by (a) whether the mediated tools work with a
configuration write alone, (b) whether hooks exist, and (c) how stable the
surface has been over the last year.

1. **Codex hosts, as one family** (ChatGPT desktop app, IDE extension,
   CLI): one `[mcp_servers]` table serves all three, the desktop app and
   the CLI started the server from it and the CLI completed a mediated
   fetch; hooks carry MCP tool results; the table shape has held for a
   year. `install codex` exists; the work is the approval mode and a
   recorded session per surface.
2. **Claude Desktop**: one JSON key in one file, server start and tool
   listing proven, format unchanged since the protocol was published; no
   hooks, so mediated only; needs its own `--host` value and a decision on
   the two clients per launch.
3. **Cursor**: one JSON file, server start proven before sign-in; hooks
   carry the full MCP result and every tool's output; weekly releases with a
   versioned hooks file.
4. **Copilot CLI**, with the Copilot app on the same file: one command
   writes the file; hooks carry `textResultForLlm` for `web_fetch` and
   `web_search`; the call needs an account whose plan allows MCP, so it is
   unproven.
5. **VS Code**, Local harness, and the Copilot harness on the same file: a
   command writes the user file; hooks only through Copilot Chat, with
   matchers ignored and the Claude Code hook file loaded; decided: no hooks
   until a Copilot Chat payload shows a tool result.
6. **Copilot agent mode in VS Code**: as 5, plus the subscription.

Hosts that cannot be served by the local binary and need a hosted edge, an
edge reachable over HTTPS from the vendor's cloud:

- Claude in the browser and Claude in Chrome (custom connectors; the
  desktop's local-MCP bridge announces the local tools and does not carry
  calls, `live-verified`).
- ChatGPT on the web and the chat mode of its desktop app (developer-mode
  apps and plugins).
- Microsoft 365 Copilot (Copilot Studio connector or declarative-agent
  plugin, Streamable HTTP).
- The Copilot cloud agent on GitHub: a binary can be installed into the
  job's sandbox, but policy and evidence have nowhere local to live, which
  makes it a hosted design in practice.
