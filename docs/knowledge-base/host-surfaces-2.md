---
title: Host surfaces, the other hosts
draft: true
---

# Host surfaces, the other hosts

This page continues [Host surfaces](host-surfaces.md), which covers the
Anthropic, OpenAI and Microsoft hosts, and asks the same questions of the
other agent hosts with an MCP client: how the host is configured with an MCP
server, whether it has lifecycle hooks and which events carry a tool
result, what session identity it exposes, whether an extension API can
observe tool calls where hooks do not exist, how the host identifies itself,
and whether the mediated tools (`context_fetch`, `context_search`,
`context_status`) work through it unchanged. The closing section ranks
these hosts among themselves for integration.

Every finding carries one of three grades, never collapsed upwards:

- `live-verified`: run on one macOS machine on 14 September 2026 with the
  v0.3.1 edge installed by the public installer into `~/.local/bin`; the
  file or output is quoted. A fact read from a host's installed bundle,
  without running that code path, says so beside the grade.
- `spec-verified`: read in the host's published documentation, cited by
  URL; every page was read on 14 September 2026, and where the page carries
  its own date that date is given too. Several of these hosts publish their
  source and document less than it does; a fact read from published source
  is `spec-verified`, cites the file, and says "from source". Where a
  documentation page shows no date and lives in a repository, the date
  given is the last commit to that file.
- `planned`: the interface is identified and nothing is proven.

The two ways a host reaches the runtime are
[`docs/contracts/host-integration.md`](../contracts/host-integration.md) §1:
the **mediated** path (the MCP server, which can refuse before content
moves) and the **observed** path (a hook that runs after the host's own
tool call and can only record). "Mediated only" below means the host offers
no hook that carries a tool result; "observed too" means it does.

## How the live probes were run

- The v0.3.1 binary was installed with the command its release notes
  state, into `~/.local/bin/commonmeasure`, checksum verified.
- Each host was installed with npm or Homebrew and run with no account
  signed in and no model key. Where the host stops at a sign-in screen, the
  section says whether anything had started before it.
- Each registration was written with the host's own command where it has
  one (`gemini mcp add`, `kiro-cli mcp add`, `amp mcp add`, `agy mcp add`)
  and by hand otherwise, and removed afterwards with the same command or by
  restoring the file.
- As on the first page, the registered command was a short shell script
  that copies what the MCP client sends, and what the server answers, into
  a log and relays the stream to the real binary. It also records the
  working directory and the names of the environment variables the host
  passed. The `clientInfo` quoted below is from those logs.
- Hook payloads were captured the same way: the hook command was a script
  that copies its standard input and the host's own environment variables
  into a log.
- The Devin CLI reads Claude Code's configuration files. It was run with
  `HOME` pointed at an empty directory holding Claude Code-format files
  that name the probe scripts, so no hook already registered on the machine
  ran inside it.
- No probe changed the operator home's policy or credentials. Five of the
  servers that were started and listed (through Cline, OpenCode, Junie,
  Goose and Antigravity) left a session file in `~/.commonmeasure/sessions/`
  holding one `credentials_loaded` record and nothing else.

One fact about the runtime frames every section. The `host` field in
session evidence is the value of the `--host` argument the registration
passes to `commonmeasure mcp`, which defaults to `claude-code`, and the
server accepts only `claude-code`, `codex`, `pi`, `claude-desktop`,
`cursor`, `copilot-cli` and `vscode`
([`docs/contracts/host-integration.md`](../contracts/host-integration.md)
§1). None of the hosts on this page has a value, so a registration written
for any of them today records `host: claude-code`, and only `client`, the
name the program sends in `initialize`
([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Client identity), says which program it was. Each section gives that name
so a future `--host` value and the record can both be exact.

## Summary

| Host | Registration surface | Hooks with a tool result | Session id to the server | Client name sent in `initialize` | Mediated tools | Verdict |
|---|---|---|---|---|---|---|
| Gemini CLI | `~/.gemini/settings.json` and `.gemini/settings.json`, `mcpServers`, stdio, SSE or HTTP; stdio suppressed in untrusted folders | `AfterTool` with `tool_response` (installed bundle); `SessionStart` payload `live-verified`; built-in web tools return a model summary | none (server generates one); `GEMINI_SESSION_ID` to hooks | `gemini-cli-mcp-client` 0.59.0; `mcp-test-client` 0.0.1 from `gemini mcp list` | started and listed before sign-in, `live-verified`; a call `planned` | observed too |
| Zed | `context_servers` in `~/.config/zed/settings.json` (JSON with comments) and `.zed/settings.json`, stdio or HTTP | none | none | `Zed` 0.1.0 | one server per open project, listed, `live-verified`; a call `planned` | mediated only; forwards servers to external agents |
| Cline (VS Code) | `cline_mcp_settings.json` in VS Code's global storage, or `~/.cline/data/settings/`, `mcpServers`, stdio, SSE or HTTP | `PostToolUse` executable with `postToolUse.result` (installed bundle) | none; `taskId` to hooks | `Cline` 4.1.17 | started and listed before any account, `live-verified`; a call `planned` | observed too |
| OpenCode | `mcp` in `~/.config/opencode/opencode.json` or project `opencode.json`, `type: local` or `remote` | plugin `tool.execute.after` with the output (from source) | none; `sessionID` to plugins | `opencode` 1.18.30 | started and listed, `live-verified`; a call `planned`, the host's own run fails on the probe machine | observed too, through a plugin |
| Devin Desktop and the Devin CLI | `~/.config/devin/mcp_config.json` and `.devin/`, and by default Claude Code's `~/.claude.json` | `PostToolUse` with `tool_response`, from Claude Code's hook file too (`spec-verified`) | none; `session_id` to hooks | not captured: sign-in first | Claude Code's registration listed, `live-verified`; start `planned` | observed too, once signed in |
| JetBrains AI Assistant | a settings dialog; no file documented | none | not documented | not probed | `spec-verified` only | mediated only |
| JetBrains Junie | `~/.junie/mcp/mcp.json` and `.junie/mcp/mcp.json`, `mcpServers` | no post-tool event; hooks in early access, CLI only | none documented | `junie-client` 1.0.0, protocol `2025-03-26` | **refused** v0.3.1's `2025-06-18` answer; connected and listed once the server answered `2025-03-26`; both `live-verified`, a call `planned` | mediated only |
| Kiro | `~/.kiro/settings/mcp.json` and `.kiro/settings/mcp.json`, `mcpServers` | `postToolUse` with the result (field named only outside Kiro's docs) | none documented; `session_id` to hooks | not captured: sign-in first | registration `live-verified`; start `planned` | observed too, once signed in |
| Goose | `extensions:` in `~/.config/goose/config.yaml`, `type: stdio` or `streamable_http` | hooks carry no tool output (from source) | **yes**: `AGENT_SESSION_ID` and `_meta.agent-session-id`, `live-verified` | `goose-cli` 1.50.0, after a `server/discover` attempt | started and listed with no model reachable, `live-verified`; a call `planned` | mediated only, with the host's session id |
| Amp | `amp.mcpServers` in `~/.config/amp/settings.json` or `.amp/settings.json` | plugin `tool.result` with `output` (`spec-verified`) | none documented; `thread.id` to plugins | not captured: sign-in first | registration `live-verified`; start `planned` | observed too, through a plugin, once signed in |
| Google Antigravity | `~/.gemini/config/mcp_config.json` and `.agents/mcp_config.json`, `mcpServers` | `PostToolUse` carries no result | none; `conversationId` to hooks | `antigravity-client` v1.0.0, after a `server/discover` attempt | started and listed before sign-in, `live-verified`; a call `planned` | mediated only |
| Kimi, Droid, Grok Build, Hermes, Kilo, Qoder, Qwen Code, Maki | per host, below | per host, below | per host, below | per host, below | `spec-verified` only | per host, below |

## Gemini CLI

**Configuration.** Servers are declared under `mcpServers` in
`~/.gemini/settings.json` (user) or `.gemini/settings.json` (project), with
system defaults and system overrides under
`/Library/Application Support/GeminiCli/`; project settings override user
settings and system overrides win. A stdio server has `command`, `args`,
`env`, `cwd`; a remote server `url` with `type: "sse"` or `"http"` (a `url`
with no `type` is Streamable HTTP) and `headers`; every server may carry
`timeout` in milliseconds, `trust` (skip confirmation), `includeTools` and
`excludeTools`. `$VAR` and `${VAR}` are expanded in settings strings; `~` is
not expanded in `command`. The child's environment is stripped of variables
whose names look like tokens or keys unless `env` names them. The model sees
an MCP tool as `mcp_<server>_<tool>`. `spec-verified`:
<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>
(last commit 2 September 2026) and
<https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md>
(last commit 17 July 2026); the `url` handling from source,
`packages/core/src/tools/mcp-client.ts`.

**Live probe.** Gemini CLI 0.59.0 was installed with `npm i -g
@google/gemini-cli`. `gemini mcp add -s user -e MCP_TEE_LABEL=gemini
commonmeasure <probe script> mcp` answered `MCP server "commonmeasure" added
to user settings. (stdio)` and wrote:

```json
{
  "mcpServers": {
    "commonmeasure": {
      "command": "<the probe script>",
      "args": [
        "mcp"
      ],
      "env": {
        "MCP_TEE_LABEL": "gemini"
      }
    }
  }
}
```

The command's default scope is the project (`-s, --scope … [default:
"project"]`). In a folder the user has not trusted, `gemini mcp list`
refused to start it:

```
Warning: MCP servers are configured but disabled because this folder is untrusted.
User-level servers are also suppressed in untrusted folders to prevent accidental side-effects.
```

With the folder trusted for the run (`GEMINI_CLI_TRUST_WORKSPACE=true`),
`gemini mcp list` printed `✓ commonmeasure: … (stdio) - Connected`. That
command connects with its own client and only pings:

```
"clientInfo":{"name":"mcp-test-client","version":"0.0.1"}
```

Starting the interactive `gemini`, which stopped at `How would you like to
authenticate for this project?` with no method chosen, had already started
the server as the session's client, protocol `2025-06-18`, followed by
`tools/list`:

```
"capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"gemini-cli-mcp-client","version":"0.59.0"}
```

The server ran in the directory `gemini` was started in, and the host added
`GEMINI_CLI` to its environment and no session identifier. Grade:
registration, start and tool listing `live-verified` before sign-in. A call
needs a Google sign-in, a `GEMINI_API_KEY` or Vertex AI credentials:
`planned`.

**Hooks.** Hooks are on by default and live under `hooks` in the same
settings files, and in an extension's `hooks/hooks.json`. Events:
`SessionStart`, `SessionEnd`, `BeforeAgent`, `AfterAgent`, `BeforeModel`,
`AfterModel`, `BeforeToolSelection`, `BeforeTool`, `AfterTool`,
`PreCompress`, `Notification`. Every payload carries `session_id`,
`transcript_path`, `cwd`, `hook_event_name` and `timestamp`; `AfterTool`
adds `tool_name`, `tool_input`, `tool_response` (`llmContent`,
`returnDisplay`, `error`), `mcp_context` for MCP tools (`server_name`,
`tool_name`, `command`, `args`, `cwd`, `url`) and `original_request_name`.
`spec-verified`:
<https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/reference.md>
(last commit 10 April 2026). The installed bundle builds `AfterTool` input
as `tool_name: toolName, tool_input: toolInput, tool_response:
toolResponse, ...mcpContext && { mcp_context: mcpContext }`, and sets
`GEMINI_PROJECT_DIR`, `GEMINI_CWD`, `GEMINI_SESSION_ID` and, "For
compatibility", `CLAUDE_PROJECT_DIR` for every hook command
(`live-verified` from the installed bundle, not run).

A `SessionStart`, `BeforeAgent`, `AfterTool` and `SessionEnd` hook was
added to `~/.gemini/settings.json` and the interactive CLI started, again
with no sign-in. `SessionStart` and `SessionEnd` fired; the first payload
was:

```json
{"session_id":"8195e8c6-94a2-4f8d-b6ca-545246a50ea6","transcript_path":"~/.gemini/tmp/work/chats/session-2026-09-14T16-45-8195e8c6.jsonl","cwd":"<the working directory>","hook_event_name":"SessionStart","timestamp":"2026-09-14T16:45:39.715Z","source":"startup"}
```

and the hook's environment carried `GEMINI_SESSION_ID`, `GEMINI_PROJECT_DIR`,
`GEMINI_CWD`, `GEMINI_PLANS_DIR` and `CLAUDE_PROJECT_DIR`. `live-verified`.
`AfterTool` needs a model call and did not fire: `planned`.

That payload is Claude Code's shape. Fed to `commonmeasure hook
session-start --host claude-code` with a home of its own, the v0.3.1 binary
printed the nudge and recorded `nudge_issued` with `host: claude-code`
(`live-verified`). The Claude Code reader in this repository refuses that
payload twice over: it carries `timestamp`, which the reader refuses as a
field no Claude Code payload carries, and it runs under `GEMINI_SESSION_ID`,
which the reader refuses whatever the payload
([`docs/contracts/host-integration.md`](../contracts/host-integration.md)
§2, `crates/commonmeasure-cli/tests/hook_e2e.rs`). Gemini CLI does not load
`~/.claude/settings.json` by itself, but `gemini hooks migrate` ("Migrate
hooks from Claude Code to Gemini CLI") copies those hooks into its own file,
and an operator who ran it with v0.3.1 got Gemini sessions recorded as Claude
Code sessions.

**Built-in web tools.** `web_fetch` and `google_web_search` are hosted: the
fetch "Uses the Gemini API's `urlContext` for retrieval", falling back to a
local fetch that is still summarised by a model, and search results are
processed by the Gemini API "before returning a synthesized response".
`AfterTool` fires for both, but its `tool_response` is the model's summary
with its sources, not the page. `spec-verified`:
<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/web-fetch.md>
(last commit 1 September 2026) and
<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/web-search.md>
(last commit 13 February 2026). The experimental `directWebFetch` setting
fetches locally and puts the page text in `llmContent` (from source,
`packages/core/src/tools/web-fetch.ts`).

**Extension API.** Extensions (`~/.gemini/extensions/<name>/gemini-extension.json`)
contribute `mcpServers` and hooks, so an extension can observe through the
same events. `spec-verified`:
<https://github.com/google-gemini/gemini-cli/blob/main/docs/extensions/reference.md>
(last commit 14 May 2026).

**Session identity.** Hooks receive `session_id` and `GEMINI_SESSION_ID`;
the server receives neither (`live-verified` above), and the only `_meta`
the client sends is a progress token (from source, `mcp-client.ts`).

**Self-identification.** `gemini-cli-mcp-client` with the CLI version for a
session, `mcp-test-client` 0.0.1 for `gemini mcp list`. Hooks are told
apart from Claude Code's by `GEMINI_SESSION_ID` in the environment, not by
the payload. `--host gemini-cli` would be the exact name.

**Verdict:** mediated tools by a configuration write in a trusted folder;
observed crossings for MCP results and local tools through `AfterTool`,
with the built-in web tools observable only as the model's summary.
Observed too.

**Integration.** One JSON key on the Claude Desktop shape, written at user
scope, plus a `--host gemini-cli` value. The hooks need a reader for
Gemini's tool names and `tool_response` shape. The Claude Code reader
already refuses a payload whose environment carries `GEMINI_SESSION_ID`.

**Stability.** Hooks were built between November and December 2025, their
settings shape changed in January 2026 (a `hooks.enabled` setting was
added, the `hooks` object was made to hold event names only, the legacy
`tools.enableHooks` setting was removed, and hooks were turned on by default
on 21 January 2026), and payload fields were added since (`transcript_path` in December
2025, `mcp_context` in January 2026). MCP servers moved to one `url` field
in December 2025 and MCP tool names changed to the `mcp_` form in March
2026. `spec-verified` from the merge dates of the pull requests the
changelog (<https://github.com/google-gemini/gemini-cli/blob/main/docs/changelogs/index.md>,
last commit 8 September 2026) links. Qwen Code, a fork, shares the shape
(below).

## Zed

**Configuration.** `context_servers` in `~/.config/zed/settings.json` (user)
or `.zed/settings.json` (project). A stdio server has `command`, `args`,
`env`, `timeout` in seconds, `enabled`; an HTTP server `url`, `headers`,
`timeout`, `oauth`. There is no `cwd` field and no command that writes the
file; the Settings screen "will create entries in your settings file". The
file is JSON with comments and trailing commas. Project servers start only
once the worktree is trusted; "Global MCP servers … are installed and
started as usual". `spec-verified`: <https://zed.dev/docs/ai/mcp> and
<https://zed.dev/docs/worktree-trust> (no date on the pages; both files last
changed 8 July 2026); field names from source,
<https://github.com/zed-industries/zed/blob/main/crates/settings_content/src/project.rs>.

**Live probe.** Zed 1.19.2 and Zed Preview 1.20.1 were already running and
share the one settings file. This entry was added to it:

```json
"context_servers": {
  "commonmeasure": {
    "command": "<the probe script>",
    "args": ["mcp"],
    "env": { "MCP_TEE_LABEL": "zed" }
  }
},
```

Both applications reloaded the file on save and, with no restart, started
six servers, three with an open project's root as their working directory
and three with `/`. Every client sent, protocol `2025-11-25`:

```
"clientInfo":{"name":"Zed","version":"0.1.0"}
```

followed by `tools/list`. Zed passed no environment variable of its own.
Restoring the file stopped all six. Whether the two instances were signed in
was not examined; Zed's documentation states "Signing in to Zed is not
required" (<https://zed.dev/docs/authentication>). Grade: registration,
start and tool listing `live-verified`. A call needs a model, either a Zed
sign-in or a provider key: `planned`.

**Hooks.** None. No hook setting exists, and a pull request adding tool-use
hooks to the built-in agent was closed unmerged on 4 April 2026. The
built-in `fetch` runs locally and `search_web` is hosted and "only available
to Zed Pro subscribers using the Zed provider"; neither is observable.
`spec-verified`: <https://zed.dev/docs/ai/tools> (file last changed 1 July
2026) and <https://github.com/zed-industries/zed/pull/52729>.

**Extension API.** An extension can register a context server
(`context_server_command` in `extension.toml`), and Zed plans to deprecate
those "in favor of the official MCP registry"; the extension interface
exports no tool-call event. `spec-verified`:
<https://zed.dev/docs/extensions/mcp-extensions> (file last changed 24 June
2026); the interface from source, <https://github.com/zed-industries/zed/blob/main/crates/extension_api/wit/since_v0.8.0/extension.wit>.

**External agents.** "MCP servers configured in Zed are forwarded to External
Agents via the Agent Client Protocol." Zed sends its enabled servers in
`session/new`, stdio servers as `command`, `args` and `env`, HTTP servers as
`url` and `headers` without OAuth tokens or timeout. The external agent
opens its own connection, so the server sees that agent's `clientInfo`
(`gemini-cli-mcp-client` for Gemini CLI), not Zed's. `spec-verified`:
<https://zed.dev/docs/ai/external-agents> (file last changed 5 August 2026);
the fields from source, <https://github.com/zed-industries/zed/blob/main/crates/agent_servers/src/acp.rs>. Not probed.

**Session identity.** None to the server: tool calls carry no `_meta` (from
source, <https://github.com/zed-industries/zed/blob/main/crates/agent/src/tools/context_server_registry.rs>).

**Self-identification.** `Zed` with the version of Zed's MCP client crate,
0.1.0, not the application version; stable and preview send the same.
`--host zed` would be the exact name.

**Verdict:** mediated only, by a configuration write; the same entry also
reaches any external agent Zed runs.

**Integration.** One configuration write, but into a file with comments and
trailing commas that the operator edits by hand; the writers that exist
rewrite a JSON file whole in their own formatting
([`docs/contracts/host-integration.md`](../contracts/host-integration.md)
§1), which would drop the operator's comments. It needs a writer that
edits JSON with comments in place. One server per open project means one
session per window, with the project root as `cwd`.

**Stability.** From merge dates in the repository: an HTTP transport
(November 2025), servers started through a non-interactive shell (December
2025), worktree trust (December 2025), timeouts (January 2026), OAuth (March
2026), removal of the `settings` field from HTTP servers (15 April 2026, the
one breaking change), `args` optional (June 2026). `spec-verified`, from the
merge dates of those pull requests in <https://github.com/zed-industries/zed>.

## Cline

**Configuration.** Cline's VS Code package ships two builds, a legacy one
and one on its SDK, and a rollout flag picks which runs on a machine
(<https://github.com/cline/cline/blob/main/apps/vscode-rollout/README.md>,
from source). Both read `mcpServers` from a file named
`cline_mcp_settings.json`: the legacy build in VS Code's global storage for
the extension (`~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/`
on macOS), the SDK build, the Cline CLI and the JetBrains plugin in
`~/.cline/data/settings/`, and the SDK build imports the old file. A stdio
server has `command`, `args`, `cwd`, `env`; a remote one `type: "sse"` or
`"streamableHttp"`, `url`, `headers`; each may carry `disabled`, `timeout`
in seconds and `autoApprove`. `${env:VAR}` is expanded; `~` is not. The
documentation names `~/.cline/mcp.json` for the CLI, which the source does
not read. `spec-verified`: <https://docs.cline.bot/mcp/mcp-overview> and
<https://docs.cline.bot/getting-started/config.md> (no dates on the pages);
the paths from source, `sdk/packages/shared/src/storage/paths.ts` and
`apps/vscode/src/services/mcp/schemas.ts`.

**Live probe.** Visual Studio Code 1.137.0 with Cline 4.1.17, installed with
`code --install-extension saoudrizwan.claude-dev`. On the first launch, with
no account and no provider (`"welcomeViewCompleted": false` in
`~/.cline/data/globalState.json`), Cline created the global-storage file:

```json
{
  "mcpServers": {}
}
```

The entry was written into it by hand:

```json
{
  "mcpServers": {
    "commonmeasure": {
      "command": "<the probe script>",
      "args": ["mcp"],
      "env": { "MCP_TEE_LABEL": "cline" }
    }
  }
}
```

Within fifteen seconds, with the Cline panel never opened, Cline started the
server from VS Code's extension host with working directory `/` and sent,
protocol `2025-11-25`:

```
"clientInfo":{"name":"Cline","version":"4.1.17"}
```

then `tools/list`, `resources/list`, `resources/templates/list` and
`prompts/list`. The server answered the tools and returned
`{"code":-32601,"message":"method resources/list not found"}` to the other
three, as it advertises only tools. Restoring the file stopped it. Grade:
registration, start and tool listing `live-verified` before any account. A
call needs a configured provider: `planned`.

**Hooks.** Executable files named after the event, in
`~/Documents/Cline/Hooks/` and `<workspace>/.clinerules/hooks/`, with JSON on
standard input and a thirty-second timeout. Events: `TaskStart`,
`TaskResume`, `TaskCancel`, `TaskComplete`, `PreToolUse`, `PostToolUse`,
`UserPromptSubmit`, `Notification`, `PreCompact`. The hooks page now says
only "See details under SDK Plugins page"
(<https://docs.cline.bot/customization/hooks.md>), so the payload is from
the bundle: the common fields are `clineVersion`, `hookName`, `timestamp`,
`taskId`, `workspaceRoots`, `userId` and `model`, and `PostToolUse` carries
`postToolUse: {toolName, parameters, result, success, executionTimeMs}`
(`live-verified` from the installed legacy bundle, not run). `PostToolUse`
runs for every tool but `attempt_completion`; the SDK build wires
`PostToolUse` with the tool output as `result`, and the CLI adds a
`tool_result` object (`spec-verified`, from source,
`apps/vscode/src/sdk/hooks-adapter.ts` and
`sdk/packages/shared/src/hooks/events.ts`). No hook fired in the probe,
because none runs without a task.

The legacy build's `web_fetch` and `web_search` are not local: they post to
Cline's API and need the Cline provider; their `result` still reaches
`PostToolUse`. The SDK build's `fetch_web_content` fetches locally (from
source, `WebFetchToolHandler.ts` and `sdk/packages/core/src/extensions/tools/executors/web-fetch.ts`).

**Extension API.** `ClineAPI` starts tasks and sends messages; it does not
observe tool calls (from source, `apps/vscode/src/exports/cline.d.ts`).

**Session identity.** `taskId` to hooks; nothing to the server, which
receives `{name, arguments}` only (from source, `McpHub.ts`).

**Self-identification.** `Cline` with the extension version in VS Code; the
Cline CLI sends `@cline/core` 0.0.0 (from source,
`sdk/packages/core/src/extensions/mcp/client.ts`). `--host cline` would be
the exact name.

**Verdict:** mediated tools by a configuration write; observed crossings
with the tool's result through `PostToolUse`. Observed too.

**Integration.** One JSON key on the Claude Desktop shape, but at one of two
paths depending on which build the rollout gave the machine, so an install
must write where the running build reads. Hooks are a new registration form
(executable files, not a JSON entry) and a new reader (camelCase, `taskId`,
the result as a string). The server starts in `/`, so no directory scope
applies.

**Stability.** High. Hooks arrived in v3.36.0 (6 November 2025) and were
expanded, moved and re-toggled in four releases to March 2026; v4.0.0 (26
June 2026) moved the MCP file to `~/.cline/data/settings/`; v4.1 (from 31
July 2026) split the package into two builds with different hook
vocabularies. `spec-verified`:
<https://github.com/cline/cline/blob/main/CHANGELOG.md>.

## OpenCode

**Configuration.** `mcp` in `~/.config/opencode/opencode.json` (also
`config.json` and `opencode.jsonc`), a project `opencode.json` found up to
the git root, or `.opencode/`. A local server has `type: "local"`, `command`
as one array holding the program and its arguments, `environment`, `cwd`,
`enabled`, `timeout` in milliseconds (default 5000); a remote server `type:
"remote"`, `url`, `headers`, `oauth`, and tries Streamable HTTP before SSE.
`opencode mcp add` writes the file. `{env:VAR}` is expanded; `~` is not
expanded in `command`. `spec-verified`:
<https://opencode.ai/docs/mcp-servers/> and <https://opencode.ai/docs/config/>
(both "Last updated: Sep 13, 2026").

**Live probe.** OpenCode 1.18.30, installed with `brew install opencode`.
`~/.config/opencode/opencode.json` was written by hand:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "commonmeasure": {
      "type": "local",
      "command": ["<the probe script>", "mcp"],
      "environment": { "MCP_TEE_LABEL": "opencode" }
    }
  }
}
```

`opencode mcp list` printed `✓ commonmeasure connected` and `1 server(s)`;
the client sent, protocol `2025-11-25`:

```
"capabilities":{"roots":{}},"clientInfo":{"name":"opencode","version":"1.18.30"}
```

followed by `tools/list`, in the directory the command ran in, with
`OPENCODE` and `OPENCODE_PID` added to the environment.

OpenCode offers zero-cost models that need no account. `opencode run -m
opencode/big-pickle "…"` asking for the three tools failed before the model
was reached:

```
Error: {"name": "UnknownError", "data": {"message": "Unexpected server error. Check server logs for details." …}}
```

with the log line `TypeError: undefined is not an object (evaluating
'a.name')` in `SystemPrompt.environment`. The same error followed with
`--pure` (no plugins), with a second free model, in a git repository, and
with `XDG_CONFIG_HOME` pointed at an empty directory so that no OpenCode
configuration was read at all, so the failure is the host's on this
machine and not the registration's. A plugin file placed in
`~/.config/opencode/plugin/` for that run was loaded and called with the
working directory before the failure. Grade: registration, start and tool
listing `live-verified`. A call `planned` until an OpenCode run completes
on the machine, with a free model or a provider key.

**Plugins.** OpenCode has no shell hooks; plugins are modules in
`.opencode/plugins/`, `~/.config/opencode/plugins/` or npm, given `{project,
client, $, directory, worktree}`, with `tool.execute.before` and
`tool.execute.after`. `spec-verified`: <https://opencode.ai/docs/plugins/>
("Last updated: Sep 13, 2026"). `tool.execute.after(input {tool, sessionID,
callID, args}, output {title, output, metadata})` receives the result of the
built-in tools, and for an MCP tool the raw MCP result (`content[]`) (from
source, `packages/opencode/src/session/tools.ts`). `webfetch` fetches
locally; `websearch` calls a hosted MCP endpoint (Exa or Parallel) and is
available with the OpenCode provider or when `OPENCODE_ENABLE_EXA` is set
(<https://opencode.ai/docs/tools/>). Both fire the plugin with their result.

**Session identity.** `sessionID` and `callID` to plugins; nothing to the
server, which receives `{name, arguments}` and answers `roots/list` with
the workspace (from source, `packages/opencode/src/mcp/`).

**Self-identification.** `opencode` with the installed version. Kilo, a
fork, sends `kilo` (below). `--host opencode` would be the exact name.

**Verdict:** mediated tools by a configuration write; observed crossings
with content through a plugin. Observed too.

**Integration.** A configuration write on a shape of its own (`mcp`,
`command` as an array, `environment`). The observed path is the Pi pattern:
a plugin module the install writes, which would pass `tool.execute.after`
to `commonmeasure hook` and needs a reader for its shape, or spawn the
server with `--session <sessionID>` as the Pi extension does.

**Stability.** The `mcp` key and the `tool.execute.*` names have held across
the year's releases; changes were additions (`cwd` for local
servers in v1.17.4, 12 June 2026). A second configuration schema and plugin
API are in progress, and v1.18.24 (28 August 2026) made the current schema
read some of its fields. `spec-verified`:
<https://github.com/anomalyco/opencode/releases>.

## Devin Desktop and the Devin CLI

**What the name covers.** `brew info --cask windsurf` answers with
`devin-desktop (Devin Desktop): 3.10.23` (`live-verified`): the Windsurf
cask installs Devin Desktop, and release 3.9.19 (8 September 2026) states
"Cascade has been removed. Devin Local is now the only agent available in
Devin Desktop". Devin Local is the Devin CLI's agent, which the desktop
starts with `devin acp`. The MCP configuration in
`~/.codeium/windsurf/mcp_config.json` "applies to the legacy Cascade agent
only". `spec-verified`:
<https://docs.devin.ai/desktop/devin-desktop-faq.md>,
<https://docs.devin.ai/desktop/changelog.md> and
<https://docs.devin.ai/desktop/cascade/mcp> (no dates on the pages; the
changelog entries are dated). This section therefore covers the Devin CLI;
the Cascade hooks (`post_mcp_tool_use` with `mcp_result`) are gone with
Cascade and are not described further.

**Configuration.** `mcpServers` in `~/.config/devin/mcp_config.json`,
`.devin/mcp_config.json` and `.devin/mcp_config.local.json`; a stdio server
has `command`, `args`, `env`, `disabled`, a remote one `url`, `transport`
(`http` or `sse`), `headers` and OAuth fields; `disabledTools` per server.
By default (`read_config_from`) it also reads the Windsurf file, Claude
Code's `.mcp.json`, `~/.claude.json` and `~/.claude/settings.json`,
Cursor's `.cursor/mcp.json`, OpenCode's and Zed's. Devin Local "prompts for
approval before calling any MCP tool". Administrators can allowlist servers.
`spec-verified`:
<https://docs.devin.ai/cli/extensibility/mcp/configuration.md>,
<https://docs.devin.ai/cli/reference/configuration/read-config-from.md> and
<https://docs.devin.ai/cli/enterprise/team-settings.md>.

**Live probe.** Devin CLI 3000.10.21, installed with `brew install --cask
devin-cli`. Before anything was registered for it, `devin mcp list` printed:

```
Configured MCP servers:

  • commonmeasure
    Command: ~/.cargo/bin/commonmeasure mcp --host claude-code
```

which is the user-scope entry in the machine's `~/.claude.json`, written for
Claude Code by an earlier `install claude`. Run with `HOME` pointed at a
directory holding a `.claude.json` that names the probe script and a
`.claude/settings.json` with `SessionStart`, `UserPromptSubmit` and
`PostToolUse` hooks that name the hook probe, `devin mcp list` listed the
probe entry, and the interactive `devin` stopped at:

```
How would you like to log in?
❭ 1 Log in with browser
```

with no server started and no hook run. Grade: that the Devin CLI reads
Claude Code's MCP registration `live-verified`; server start, the hooks and
the client name `planned` until a Devin account signs in. Devin Desktop was
not installed: it needs the same account ("you will need to log in with your
Devin account", <https://docs.devin.ai/desktop/getting-started.md>) and runs
the same agent.

**Hooks.** Claude Code's format. Locations: `.devin/hooks.v1.json`, `hooks`
in `.devin/config.json`, `~/.config/devin/config.json`, and Claude Code's
`.claude/settings.json`, `.claude/settings.local.json`, `~/.claude.json` and
`~/.claude/settings.json`; plugins. Events: `PreToolUse`, `PostToolUse`,
`PermissionRequest`, `UserPromptSubmit`, `Stop`, `PostCompaction`,
`SessionStart`, `SessionEnd`. Every payload carries `hook_event_name`,
`tool_name`, `tool_input`, `session_id` and `prompt_id`, and hooks run with
`DEVIN_PROJECT_DIR`; `PostToolUse` adds `tool_response` with `success`,
`output` and `error`, for every tool, MCP tools named `mcp__<server>__<tool>`
and the built-in `webfetch` in the matcher list. Hooks do not load in a
workspace under Restricted Mode. `spec-verified`:
<https://docs.devin.ai/cli/extensibility/hooks/overview.md> and
<https://docs.devin.ai/cli/extensibility/hooks/lifecycle-hooks.md>.

So the four hooks `install claude` writes into `~/.claude/settings.json`
would run inside Devin sessions with `--host claude-code` and a payload the
Claude Code reader accepts, and the MCP entry in `~/.claude.json` would
serve Devin as well, recorded as `host: claude-code`. `planned` until a
signed-in session shows it.

**Extension API.** Devin plugins contribute skills, hooks and MCP servers
(<https://docs.devin.ai/cli/extensibility/plugins/overview.md>); no other
observer. **Session identity.** `session_id` and `prompt_id` to hooks;
nothing documented to the server. **Self-identification.** Not documented
and not captured.

**Verdict:** mediated tools through a registration it already reads, and
observed crossings with the result through `PostToolUse`, once signed in.
Observed too.

**Integration.** No configuration write: Claude Code's registration is read
as it stands. What is needed is for the record to be right: a `--host devin`
value an entry in Devin's own file can carry. The Claude Code reader already
refuses under `DEVIN_PROJECT_DIR` as it does under `CURSOR_PROJECT_DIR`, so
Devin's hooks are not recorded as Claude Code's; the server Devin starts
from `~/.claude.json` still records `host: claude-code`.

**Stability.** The CLI changelog (<https://docs.devin.ai/cli/changelog/stable.md>)
dates `.devin/hooks.v1.json` to 9 April 2026, `session_id` in hooks to 19
July 2026, the dedicated `mcp_config.json` files to 29 July 2026 (servers
moved out of `config.json`), `SessionEnd` to 21 August 2026 and a new
`PreToolUse` field to 10 September 2026; the product changed its name and
its agent within the year.

## JetBrains

### AI Assistant

**Configuration.** Servers are added in the dialog "Settings | Tools | AI
Assistant | Model Context Protocol (MCP)" as a JSON snippet in the
`mcpServers` shape (`command` and `args`, or `url`), with a working
directory and a global or project level; stdio, Streamable HTTP and SSE;
"Import from Claude" reads the Claude Desktop file. The file the dialog
writes is not documented. ACP agents run inside AI Assistant receive the
servers when "Pass custom MCP servers" is set (`~/.jetbrains/acp.json`,
`default_mcp_settings.use_custom_mcp`, default true). `spec-verified`:
<https://www.jetbrains.com/help/ai-assistant/mcp.html> (last modified 14
August 2026) and <https://www.jetbrains.com/help/ai-assistant/acp.html>
(last modified 22 July 2026).

**Hooks, extension API, session identity, self-identification.** No hooks
are documented; no plugin API observes AI Assistant's tool calls (the
`com.intellij.mcpServer.mcpToolset` extension point adds tools to the IDE's
own MCP server, which is a different thing); no session identifier or
client name is documented. `spec-verified` from the pages above.

**Not probed.** The IDE was not installed: AI Assistant's only MCP surface
is the dialog, it has no documented file to write or read back, and it
needs activation before use. The Claude Desktop entry `install
claude-desktop` writes is what "Import from Claude" reads.

**Verdict:** mediated only, registered through the dialog or its import.
Given Junie's result below, whether the IDE's MCP client accepts the
server's protocol version is `planned`.

### Junie

**Configuration.** `mcpServers` in `~/.junie/mcp/mcp.json` (user) and
`.junie/mcp/mcp.json` (project), `command`, `args`, `env`, or `url` and
`headers`; the IDE plugin's MCP page saves to the user file; `--mcp-location`
adds a directory; project trust is checked before servers load.
`spec-verified`:
<https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html> (last
modified 11 September 2026).

**Live probe.** Junie CLI 1468.30, installed with `npm i -g
@jetbrains/junie-cli`. `~/.junie/mcp/mcp.json` was written by hand in the
shape above. The interactive `junie`, stopped at "Sign in with JetBrains to
use Junie with your subscription, or connect an external LLM provider", had
already started the server and sent:

```
{"id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{"experimental":{},"sampling":{}},"clientInfo":{"name":"junie-client","version":"1.0.0"},"_meta":{}},"jsonrpc":"2.0"}
```

The server answered `"protocolVersion":"2025-06-18"`, and Junie sent nothing
more. Its log, `~/.junie/logs/junie.log`, recorded:

```
ERROR i.m.kotlin.sdk.client.Client - Failed to initialize client: Server's protocol version is not supported: 2025-06-18
WARN  c.i.m.l.m.core.mcp.McpClient - Failed to connect to MCP server 'commonmeasure': Error connecting to transport: Server's protocol version is not supported: 2025-06-18
```

Sent directly, the v0.3.1 server answered `2025-06-18` whatever version the
client requested (`2024-11-05`, `2025-03-26`, `2025-06-18` and `2025-11-25`
were tried). The protocol lets a server answer with another version when it
does not support the requested one, and lets the client disconnect; Junie's
client, the MCP Kotlin SDK, does. The clients that asked for `2025-11-25`
(Zed, Cline, OpenCode, Goose, Antigravity) accepted the older answer. Grade:
registration and the refusal `live-verified`. The server in this repository
answers `2025-03-26` to a client that asks for it (`PROTOCOL_VERSIONS` in
`crates/commonmeasure-harness/src/mcp.rs`, pinned by
`crates/commonmeasure-cli/tests/mediated_e2e.rs`): `fixture-tested`. Run
again with that server built from source in place of the probe's binary,
Junie CLI 1468.30, still at the sign-in prompt, sent the same `initialize`,
was answered `"protocolVersion":"2025-03-26"`, sent
`notifications/initialized` and `tools/list`, and logged:

```
INFO  c.i.m.l.m.core.mcp.McpClient - Connected to MCP server 'commonmeasure'
```

Grade: the connection and tool listing `live-verified`; a call needs a
JetBrains sign-in or a model provider: `planned`.

**Hooks.** In early access, CLI only: `hooks` in `~/.junie/config.json`,
project hooks ignored by default; events `SessionStart`, `UserPromptSubmit`,
`PreToolUse`, `Stop`, `StopFailure`, `PermissionRequest`, `SessionEnd`, with
no post-tool event, so no tool result reaches a hook; "ACP and server hosts
do not yet invoke any hooks". `spec-verified`:
<https://junie.jetbrains.com/docs/junie-cli-hooks.html> (last modified 11
September 2026).

**Session identity.** `session_id` on `SessionStart` and `UserPromptSubmit`
"when the session provides them"; nothing to the server; `_meta` in
`initialize` was empty (`live-verified`). **Self-identification.**
`junie-client` 1.0.0, not the Junie version. `--host junie` would be the
exact name.

**Verdict:** mediated only.

**Stability.** The Junie releases on GitHub add the `/mcp` wizard (8 December
2025), hot reload of `mcp.json` (24 February 2026) and project trust before
loading servers (3 August 2026); no release note announces hooks.
`spec-verified`: <https://github.com/JetBrains/junie/releases>.

## Kiro

**Configuration.** `mcpServers` in `~/.kiro/settings/mcp.json` (user) and
`.kiro/settings/mcp.json` (workspace), merged with the workspace winning, and
in a custom agent's configuration. A local server has `command`, `args`,
`env`, `disabled`, `autoApprove`, `disabledTools`; a remote one `url`,
`headers`, OAuth fields. `${VAR}` expansion, and in the IDE only for
variables the user has approved. The IDE and CLI share the files.
`spec-verified`: <https://kiro.dev/docs/mcp/configuration/> (updated 12
September 2026).

**Live probe.** Kiro CLI 2.21.4, installed with `brew install --cask
kiro-cli`. The linked `kiro-cli` failed with `error: failed to launch
~/.local/bin/kiro-cli-chat`; the binary of that name inside the application
bundle was used. `kiro-cli-chat mcp list` showed the two built-in agents with
no servers, so the CLI does not read Claude Code's files. `kiro-cli-chat mcp
add --name commonmeasure --command <probe script> --args mcp --env
MCP_TEE_LABEL=kiro --scope global` answered `✓ Added MCP server
'commonmeasure' to global config in ~/.kiro/settings/mcp.json`:

```json
{
  "mcpServers": {
    "commonmeasure": {
      "command": "<the probe script>",
      "args": [
        "mcp"
      ],
      "env": {
        "MCP_TEE_LABEL": "kiro"
      }
    }
  }
}
```

`mcp status --name commonmeasure` printed the entry and started nothing;
`kiro-cli-chat chat` asked `You are not logged in. Login now?` before
starting any server. `mcp remove --name commonmeasure --scope global` left
`{"mcpServers": {}}`. Grade: registration `live-verified`; start, the call
and the client name `planned` until a Kiro account (GitHub, Google, AWS
Builder ID or IAM Identity Center) signs in. The Kiro IDE was not installed:
it needs the same sign-in and reads the same files.

**Hooks.** JSON files in `.kiro/hooks/` and, for the CLI, `~/.kiro/hooks/`
(`"version": "v1"`, triggers with a matcher and a `command` or `agent`
action), "introduced in IDE 1.0 and CLI 3.0"; the stable CLI 2.x keeps
hooks inside the agent configuration (`agentSpawn`, `userPromptSubmit`,
`preToolUse`, `postToolUse`, `stop`). Command hooks receive JSON on standard
input with `hook_event_name`, `cwd`, `session_id`, `tool_name` and
`tool_input`; Post Tool Use runs "with access to tool results". The field
that carries the result is not named on any Kiro page; an issue on Kiro's
tracker and an AWS Labs capture name it `tool_response`. The matcher
category `web` covers the built-in `web_search` and `web_fetch`.
`spec-verified`: <https://kiro.dev/docs/hooks/> and
<https://kiro.dev/docs/hooks/types/> (updated 2 September 2026),
<https://kiro.dev/docs/cli/2x-reference/>,
<https://github.com/kirodotdev/Kiro/issues/7417> (13 April 2026) and
<https://awslabs.github.io/aidlc-workflows/reference/kiro-ide-hook-payload/>.

**Session identity.** `session_id` to hooks; nothing documented to the
server. **Self-identification.** Not documented; the source is not
published. **Extension API.** Not needed where hooks exist; none documented
for tool calls.

**Verdict:** mediated tools by a configuration write on the Claude Desktop
shape, and observed crossings through `postToolUse`, once signed in.
Observed too.

**Stability.** Low. CLI 2.0 (13 April 2026) and IDE 1.0 (25 June 2026)
introduced new hook formats, global hooks followed in July, CLI 2.21.0 (1
September 2026) lets Kiro Web inject hooks and servers into local sessions,
and CLI 2.21.4 still has both the 2.x and 3.0 hook formats behind flags.
`spec-verified`: <https://kiro.dev/changelog/>.

## Goose

**Configuration.** `extensions:` in `~/.config/goose/config.yaml`, keyed by
name, with `type` (`builtin`, `platform`, `stdio`, `streamable_http`; "SSE is
not supported"), `enabled`, `timeout` in seconds, `available_tools`; a stdio
extension has `cmd`, `args`, `envs`, `env_keys`, a remote one `uri` and
`headers`. Secrets go to the keychain. `goose configure` writes it, and
`goose session --with-extension "…"` adds one for a session. No project-level
file. `spec-verified`: <https://goose-docs.ai/docs/guides/config-files/> and
<https://goose-docs.ai/docs/guides/goose-cli-commands/> (no dates).

**Live probe.** Goose 1.50.0, installed with `brew install block-goose-cli`,
with no `config.yaml`. `goose run --no-session --with-extension
"MCP_TEE_LABEL=goose <probe script> mcp" -t "Say hello"`, with
`GOOSE_PROVIDER=ollama` and no Ollama running, printed `goose is ready` and
had started the server in the working directory. The client first tried the
stateless discovery of a newer protocol revision, then initialised:

```
{"jsonrpc":"2.0","id":0,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"goose-cli","version":"1.50.0"}, …}}}
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25", …,"clientInfo":{"name":"goose-cli","version":"1.50.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"agent-session-id":"20260914_1","progressToken":0}}}
```

The server answered `server/discover` with `method not found` and the rest
normally, and its environment carried `AGENT_SESSION_ID`. Grade: start,
tool listing and the session identifier passed to the server
`live-verified`, with no model reachable. A call needs a provider (a local
Ollama needs no key): `planned`.

**Hooks.** Plugins in `~/.agents/plugins/<name>/hooks/hooks.json` or the
project's `.agents/plugins/`, following the Open Plugins hooks
specification: `SessionStart`, `SessionEnd`, `Stop`, `UserPromptSubmit`,
`PreToolUse`, `PreToolUseResult`, `PostToolUse`, `PostToolUseFailure`,
`BeforeReadFile`, `AfterFileEdit`, `BeforeShellExecution`,
`AfterShellExecution`; documented payload fields include `session_id`,
`tool_name`, `tool_input`, `working_dir` and `tool_call_id`, and no tool
result. `spec-verified`:
<https://goose-docs.ai/docs/guides/context-engineering/hooks/> (no date).
The hook context declares `tool_output`, but the post-tool hook is built
without it (from source, <https://github.com/aaif-goose/goose/blob/main/crates/goose/src/hooks/mod.rs> and
<https://github.com/aaif-goose/goose/blob/main/crates/goose/src/agents/agent.rs>). Goose has no built-in fetch: its
`web_scrape` was removed on 14 August 2026, and web access is the shell or
an extension.

**Session identity.** The only host on this page that passes its session to
the server, twice: `AGENT_SESSION_ID` in a stdio server's environment, and
`agent-session-id`, `agent-working-dir` and `agent-tool-call-request-id` in
`_meta` on requests (`live-verified` for the first two; the others from
source, <https://github.com/aaif-goose/goose/blob/main/crates/goose/src/session_context.rs>). The server takes
`AGENT_SESSION_ID` as the session id when the registration passes no
`--session` ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Where); it does not read `_meta`, because the log is opened once per
process and a per-request value cannot name it.

**Self-identification.** `goose-cli` or `goose-desktop`, with the Goose
version; an ACP client's name replaces it when Goose runs under one (from
source, `mcp_client.rs`). `--host goose` would be the exact name.

**Verdict:** mediated only, with the host's session id offered to the
server.

**Integration.** A configuration write in YAML, a shape no install writes
today. Hooks would record turns, not crossings. The session id is an
opportunity the other hosts lack, and the server takes `AGENT_SESSION_ID`
when no `--session` is given, so the record carries Goose's own id.

**Stability.** SSE removed (12 January 2026), hooks shipped in v1.34.0 (13
May 2026), built-in web scraping removed (14 August 2026), and from v1.47.0
(21 August 2026) Goose does not start local extensions when the provider
"manages own context", as ACP providers do. `spec-verified`:
<https://github.com/aaif-goose/goose/releases>.

## Amp

**Configuration.** `"amp.mcpServers"` in `~/.config/amp/settings.json` or
the nearest `.amp/settings.json`, `command`, `args`, `env` or `url`,
`headers`, `${VAR}` expansion; a workspace server needs `amp mcp approve`.
Servers can also be stored on ampcode.com (`amp mcp remote`, Streamable HTTP
only). `spec-verified`: <https://ampcode.com/docs/customize/mcp> (no date).

**Live probe.** Amp CLI `0.0.1789401648-g1df9a1`, installed with `npm i -g
@ampcode/cli`. `amp mcp list` printed `Could not load MCP servers from Amp
account: API request for mcpListAccountServers failed: 401` and `No local
MCP servers configured.` `amp mcp add commonmeasure --env
MCP_TEE_LABEL=amp -- <probe script> mcp` answered `Added
amp.mcpServers.commonmeasure to ~/.config/amp/settings.json`:

```json
{
  "amp.mcpServers": {
    "commonmeasure": {
      "command": "<the probe script>",
      "args": [
        "mcp"
      ],
      "env": {
        "MCP_TEE_LABEL": "amp"
      }
    }
  }
}
```

`amp mcp doctor` printed `No API key found. Starting login flow...` before
starting any server. `amp mcp remove commonmeasure` left `{}`. Grade:
registration `live-verified`; start, the call and the client name `planned`
until an Amp account signs in (`amp login` or `AMP_API_KEY`).

**Plugins.** No shell hooks. Plugins are modules in `.amp/plugins/` or
`~/.config/amp/plugins/` with events `session.start`, `tool.call`,
`tool.result`, `agent.start`, `agent.end`; `tool.result` "fires after a tool
finishes and before the result is sent back to the model" with `{toolUseID,
tool, input, status, error, output, thread: {id}}` and may replace the
result. Where the built-in `web_search` and `read_web_page` run is not
documented. `spec-verified`: <https://ampcode.com/docs/customize/plugins>
and <https://ampcode.com/docs/plugin-api> (no dates).

**Session identity.** `thread.id` to plugins; nothing documented to the
server. **Self-identification.** Not documented; the CLI is a compiled
binary.

**Verdict:** mediated tools by a configuration write and observed crossings
with the result through a plugin, once signed in. Observed too.

**Stability.** Low. The editor extensions were withdrawn on 5 March 2026,
the CLI was rebuilt with the plugin API on 6 May 2026 and made the default
on 27 May 2026, and remote server definitions arrived on 19 August 2026.
`spec-verified`: <https://ampcode.com/chronicle>.

## Google Antigravity

**Surfaces.** Antigravity 2.0 (a desktop application), the Antigravity CLI
(`agy`), the Antigravity IDE and a Python SDK, with one sign-in; Google also
ships an Antigravity extension for VS Code, JetBrains, Zed and Xcode.
`spec-verified`: <https://antigravity.google/download/> (no date).

**Configuration.** "Antigravity 2.0, IDE, and CLI share a central MCP
configuration" in `~/.gemini/config/mcp_config.json`, or `.agents/mcp_config.json`
in a workspace, key `mcpServers`: `command`, `args`, `env`, `cwd` for stdio,
`serverUrl` (and, per the changelog, `url`) and `headers` for remote, plus
`disabled` and `disabledTools`. It is not Gemini CLI's `settings.json`.
`spec-verified`: <https://antigravity.google/docs/mcp/> and
<https://antigravity.google/docs/cli/gcli-migration/> (no dates).

**Live probe.** Antigravity CLI 1.2.2, installed with `brew install --cask
antigravity-cli`. `agy mcp add --env MCP_TEE_LABEL=agy commonmeasure <probe
script> mcp` answered `Added MCP server "commonmeasure" (stdio)` and wrote:

```json
{
  "mcpServers": {
    "commonmeasure": {
      "args": [
        "mcp"
      ],
      "command": "<the probe script>",
      "disabled": false,
      "env": {
        "MCP_TEE_LABEL": "agy"
      }
    }
  }
}
```

`agy mcp list` printed `commonmeasure  stdio  enabled`. The interactive `agy`,
stopped at `Welcome to the Antigravity CLI. You are currently not signed in.`
and `Select login method`, had already started the server in the working
directory:

```
{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{…,"io.modelcontextprotocol/clientInfo":{"name":"antigravity-client","version":"v1.0.0"},"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}
{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"clientInfo":{"name":"antigravity-client","version":"v1.0.0"},"protocolVersion":"2025-11-25", …}}
```

then `tools/list`, with no environment variable of its own. `agy mcp remove
commonmeasure` left `{"mcpServers": {}}`. Grade: registration, start and
tool listing `live-verified` before sign-in. A call needs a Google sign-in
or a `GEMINI_API_KEY`: `planned`. The desktop application and the IDE were
not installed; they read the same file and need the same sign-in.

**Hooks.** `hooks.json` in `.agents/` or `~/.gemini/config/`: `PreToolUse`,
`PostToolUse`, `PreInvocation`, `PostInvocation`, `Stop`, with
`conversationId`, `workspacePaths` and `transcriptPath`. `PostToolUse`
carries the call (`toolCall`), `stepIdx` and `error` ("Empty if successful"),
and no result; in the IDE it carries `stepIdx` and `error` only. The
built-in `search_web` and `read_url_content` can be matched, not observed.
The CLI's headless `--output-format stream-json` does carry "the call and
its result". `spec-verified`: <https://antigravity.google/docs/hooks/>,
<https://antigravity.google/docs/ide/hooks/> and
<https://antigravity.google/docs/cli/headless/> (no dates).

**Session identity.** `conversationId` to hooks; nothing to the server.
**Self-identification.** `antigravity-client` v1.0.0; the documentation
names none. `--host antigravity` would be the exact name.

**Verdict:** mediated only, by a configuration write on the Claude Desktop
shape.

**Stability.** Low. The changelog (<https://antigravity.google/changelog/>)
carries MCP or hook fixes in most releases from June to September 2026:
`url` accepted beside `serverUrl` (June), MCP timeouts (28 July), hooks that
"could never run are now rejected at load time" (7 August), `PostToolUse`
"firing on non-tool steps" fixed (31 July), comments allowed in
`mcp_config.json` (2 September); the documentation lags it.

## Other hosts Herdr recognises

`spec-verified` only; each from the page cited, read on 14 September 2026.
Devin and Antigravity are covered above.

| Host | MCP configuration | Hooks with a tool result | Client name | Source |
|---|---|---|---|---|
| Kimi CLI (`kimi`) | `mcpServers` in `~/.kimi/mcp.json`; stdio and HTTP | `PostToolUse` with `tool_output`, `session_id`, in `[[hooks]]` of `~/.kimi/config.toml`, beta | not set; the Python SDK default `mcp` 0.1.0 (from source, inferred) | <https://github.com/MoonshotAI/kimi-cli/blob/main/docs/en/customization/hooks.md> (last commit 30 March 2026) |
| Factory Droid (`droid`) | `mcpServers` in `~/.factory/mcp.json` and `.factory/mcp.json`; stdio, HTTP, SSE | `PostToolUse` with `tool_response`, `session_id`, `transcript_path`; MCP tools `mcp__<server>__<tool>` | not documented | <https://docs.factory.ai/harness/hooks> (no date) |
| Grok Build (`grok`) | `[mcp_servers.<name>]` in `~/.grok/config.toml`; stdio, HTTP, SSE; `{{session_id}}` in remote headers | `PostToolUse` with `toolResult`; **reads `~/.claude/settings.json` hooks and `~/.claude.json` servers by default** | `grok-shell-<server>` (from source) | <https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md> (last commit 9 September 2026) |
| Hermes Agent (`hermes`) | `mcp_servers:` in `~/.hermes/config.yaml`; stdio and HTTP | plugin `post_tool_call` with `result` and `session_id`; shell hooks carry event fields in `extra` | not set; SDK default `mcp` (from source, inferred) | <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/hooks.md> (last commit 13 September 2026) |
| Kilo (`kilo`) | `mcp` in `~/.config/kilo/kilo.json`, OpenCode's shape | plugin `tool.execute.after` with the output | `kilo` (from source) | <https://github.com/Kilo-Org/kilocode/blob/main/packages/kilo-docs/pages/automate/extending/plugins.md> (last commit 30 July 2026) |
| Qoder CLI (`qodercli`) | `mcpServers` in `~/.qoder/settings.json` or `.mcp.json`; stdio, SSE, HTTP, WebSocket | `PostToolUse` with `tool_response` and `mcp_context`, `session_id` | not documented | <https://docs.qoder.com/cli/hooks> (no date) |
| Qwen Code (`qwen`) | `mcpServers` in `~/.qwen/settings.json`, Gemini CLI's shape | `PostToolUse` with `tool_response`, `session_id`, on by default | `qwen-cli-mcp-client-<server>` 0.0.1 (from source) | <https://github.com/QwenLM/qwen-code/blob/main/docs/users/features/hooks.md> (last commit 14 September 2026) |
| Maki (`maki`) | `[mcp.<name>]` in `~/.config/maki/mcp.toml`; stdio and HTTP | Lua `tool.<name>.output` slots with `text` and `session_id`, for built-in and MCP tools | `maki` (from source) | <https://github.com/tontinton/maki/blob/main/site/docs/content/hooks/_index.md> (last commit 30 August 2026) |
| Oh My Pi (`omp`) | `~/.omp/agent/mcp.json`; also reads Claude Code's, Codex's, Gemini's, Cursor's and OpenCode's server files | TypeScript `tool_result` event | `omp-coding-agent` 1.0.0 (from source) | <https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/mcp/client.ts> |
| Mastra Code (`mastracode`) | `mcpServers` in `~/.mastracode/mcp.json` or `.mcp.json`; can opt in to `~/.claude.json` | `PostToolUse` with `session_id`; result field not shown | not documented | <https://code.mastra.ai/configuration> (no date) |

Grok Build, Oh My Pi and, when enabled, Mastra Code read Claude Code's
registration the way the Devin CLI does, and Grok Build runs Claude Code's
hooks, so `install claude` reaches their mediated sessions with
`host: claude-code`; the Claude Code reader refuses Grok Build's hooks under
`GROK_SESSION_ID`.

## Order

Ranked for integration among the hosts on this page by the first page's
criteria: (a) whether the mediated tools work with a configuration write
alone, (b) whether hooks exist, and (c) how stable the surface has been over
the last year. Each entry says what integrating it would take: a
configuration write on an existing pattern (the Claude Desktop or Cursor
shape `install` already writes), a new reader (hooks with a payload shape
of their own), or a hosted edge.

1. **Gemini CLI**: one `mcpServers` key on the Claude Desktop shape, and
   server start and listing proven before sign-in; hooks carry
   `tool_response` in Claude Code's shape, and `SessionStart` is proven;
   hook settings changed shape in January 2026 and MCP tool names in March
   2026.
   A configuration write plus a new reader close to Claude Code's; the
   Claude Code reader already refuses under `GEMINI_SESSION_ID`. Qwen Code
   follows on the same work.
2. **OpenCode**: a configuration write on a shape of its own, start and
   listing proven; plugins receive every tool's output; the `mcp` key and
   plugin event names held all year. A new writer, and a plugin on the Pi
   extension pattern with a reader for its shape. Kilo follows on the same
   work. A recorded call waits on the host's own run failure.
3. **Cline**: one `mcpServers` key on the Claude Desktop shape, start and
   listing proven with no account; `PostToolUse` carries the result; high
   churn, two builds reading two paths. A configuration write at the path
   the running build reads, and a new reader with a new registration form
   (executable hook files).
4. **Zed**: one `context_servers` key, start and listing proven, one server
   per project; no hooks, so mediated only; one breaking change in the
   year. A configuration write that must keep the settings file's comments,
   which no writer does today. The entry also reaches external agents Zed
   runs.
5. **Google Antigravity**: one `mcpServers` key on the Claude Desktop shape,
   start and listing proven before sign-in, and one file for the CLI, the
   desktop application and the IDE; hooks carry no result, so mediated
   only; unstable. A configuration write on an existing pattern.
6. **Goose**: start and listing proven with no model; the only host that
   passes its session id to the server; hooks carry no output, so mediated
   only. A new writer for YAML; the server already adopts
   `AGENT_SESSION_ID`.
7. **Kiro**: registration proven with its own command on the Claude Desktop
   shape; hooks with the result; start needs an account; low stability. A
   configuration write on an existing pattern, and a new reader.
8. **Devin CLI and Devin Desktop**: already reads Claude
   Code's registration and hook file, proven for the server entry; start
   needs an account. No write; a `--host devin` value, so its mediated
   sessions are not recorded as Claude Code's. The Claude Code reader
   already refuses its hooks under `DEVIN_PROJECT_DIR`.
9. **Amp**: registration proven with its own command; plugins with the
   result; everything, even `amp mcp doctor`, needs an account; low
   stability. A configuration write on an existing pattern, and a plugin
   with a new reader.
10. **JetBrains Junie**: registration on the Claude Desktop shape proven, and
    v0.3.1's server refused on protocol version, and the server in this
    repository answers the version Junie asks for; no post-tool hook. A configuration write on an existing
    pattern.
11. **JetBrains AI Assistant**: a settings dialog with no documented file;
    no hooks. Nothing to install; the Claude Desktop entry is what its
    import reads, subject to the same protocol question as Junie.

None of these hosts needs a hosted edge for its local agent. Remote server
definitions stored with a vendor (Amp's `amp mcp remote`, Kiro Web's cloud
configuration, Devin's cloud sessions) run away from the operator's machine
and would need one, as the first page's browser and cloud hosts do.
