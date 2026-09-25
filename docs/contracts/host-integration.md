---
title: Host integration contract
domain: edge
audience: integrator
section: reference
---

# Host integration contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This is the document an organisation reads to put Common Measure inside an
agent system it builds or runs itself. Common Measure is not a harness and not
an agent framework: it is the component between an agent and the content
the agent reads. A **harness** is the agent system the operator runs, bought
or built. A **host** is the specific program the integration attaches to;
Claude Code, Codex, Pi, Claude Desktop, Cursor, the Copilot CLI and VS Code
are the hosts integrated today; hosts that reach MCP servers only from their
vendor's cloud, such as Microsoft 365 Copilot, use the hosted service (§1, The
hosted path); and a Chrome extension
observes browser answer surfaces (§2, Browser answer surfaces). Terms like
crossing, admission, principal and scope are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).
Every claim below names the code or test it rests on. The binary is
`commonmeasure` and its home directory is `~/.commonmeasure`.

## 1. The two ways in

A host reaches the runtime in one of two ways, and they are not equal
(`crates/commonmeasure-harness/src/lib.rs`).

| Way in | Command | When it runs | What it can do |
|---|---|---|---|
| **Observed** | `commonmeasure hook <event> --host <name>`, payload on stdin | after the host's tool call has completed | record what the agent read; it cannot refuse |
| **Mediated** | `commonmeasure mcp --host <name> [--session <id>]`, JSON-RPC over stdio | before the content moves | apply policy, refuse, record |
| **Mediated, hosted** | `commonmeasure hosted service` (or `hosted serve --origin <origin>`), Streamable HTTP at `<origin>/mcp/<host>` | before the content moves, for a host that reaches MCP servers only from its vendor's cloud | the same, for the principal a bearer token names |

The observed path is a set of lifecycle hooks. It needs no credentials and
no policy file, and it never breaks the agent: unreadable input yields no
record and the process exits zero whatever happened (`crates/commonmeasure-cli/src/main.rs`
`hook`, `crates/commonmeasure-harness/src/hook.rs`).

The mediated path is an MCP server that offers four tools, `context_fetch`,
`context_search`, `context_status` and `context_enrol` (`crates/commonmeasure-harness/src/mcp.rs`
`tool_definitions`). An agent that calls them instead of its own tools gives
the runtime the crossing before it happens, which is the only point at which
policy can refuse. Each path appends to the session log named by the
identifier it holds: on a host that passes its session identifier to the
server (Pi, Goose) that is one log; on one that does not (Claude Code, Codex)
the hooks write under the host's identifier and the server under one it
mints, and the two are joined by the host process that started both
([`docs/contracts/session-evidence.md`](session-evidence.md) §Host process).
Every record carries which path wrote it (§Crossing).

### The hosted path

The same server, served over HTTP to the hosts that reach an MCP server
only from their vendor's cloud (`crates/commonmeasure-cli/src/hosted.rs`).

- The path is `<origin>/mcp/<host>` for the host words `claude-connector`,
  `chatgpt`, `m365-copilot` and `copilot-cloud-agent`. `<origin>` is scheme
  and authority only, lower-case, default port dropped. The word is the
  `host` on every record of a session through it. Any other path is `404`
  naming the endpoints served.
- Streamable HTTP with JSON responses only. `POST` carries one JSON-RPC
  message; `DELETE` ends the session; `GET` and every other method are
  `405` with `Allow`. This server initiates nothing, so it opens no event
  stream.
- The endpoint serves `context_fetch`, `context_search` and
  `context_status`, each declaring `readOnlyHint`, and negotiates protocol
  revisions `2025-06-18` and `2025-11-25`. `context_enrol` is not
  advertised and a call to it is answered as an unknown tool: it acts on
  the directory the server runs in, which a hosted session does not have.
  An `initialize` asking for another revision is answered with
  `2025-11-25`.
- `initialize` without `Mcp-Session-Id` mints a session,
  `hosted-<milliseconds>-<128 random bits in hex>`, returned in that
  header and bound to the bearer that opened it. `initialize`,
  `notifications/initialized`, `tools/list` and `ping` are answered
  without opening anything on disk; the session is opened, and its
  identity recorded, at its first other request
  ([`docs/contracts/session-evidence.md`](session-evidence.md) §Where).
  A session idle for thirty minutes ends; the process ending ends them
  all.
- Every request is checked in this order, and the first failure answers:

  | Check | Refusal |
  |---|---|
  | the path names a served host word | `404`, naming the endpoints |
  | the method is `POST` or `DELETE` | `405` with `Allow: POST, DELETE` |
  | `Origin`, when present, is the edge's origin or one the operator listed | `403` |
  | a `Bearer` token is present and verifies (§The bearer token) | `401` with `WWW-Authenticate` naming the resource metadata and the check that failed |
  | `MCP-Protocol-Version`, when present, is `2025-06-18` or `2025-11-25` | `400` naming the revisions served |
  | a request other than `initialize` names a session in `Mcp-Session-Id` | `400` |
  | the session is known, not ended, opened by this bearer on this endpoint | `404` with one wording for all four, so no session's existence is disclosed |
  | the body is one JSON object | `400`, a JSON-RPC error owed to no request |

- `GET <origin>/.well-known/oauth-protected-resource/mcp/<host>` serves the
  endpoint's protected resource metadata (RFC 9728) without a token:
  `resource` is the endpoint URL exactly as above, `authorization_servers`
  the one issuer pinned at enrolment.

`commonmeasure hosted service` is the deployed form. It reads
`~/.commonmeasure/hosted-service.json`: `origin`, `hosts` (the words
served), and optionally `listen` (default `127.0.0.1:8765`),
`allowed_origins`, `interval_seconds` (default 300) and `session_directory`.
The last is an absolute existing directory explicitly enrolled locally under
the service account, with a current binding. It is the operator-declared scope
for every session on this service, never client-supplied. A `hosted_scope`
record states that basis. It enables no reporting by itself: local reporting
opt-in, a current signed Hub owner reporting approval and the source-policy/privacy checks
remain necessary. Omission preserves no-directory sessions. Relay clearance
still resolves under the service OS principal. Policies restricted to bearer
principals or their owned scopes can therefore withhold reporting even when
acquisition succeeds; that configuration is not verified. The Microsoft 365
Copilot verification (§6) used top-level policy with no principal bindings or
scopes. The service refuses to start
without that file, unenrolled, under local deployment mode, or while
another process holds `~/.commonmeasure/hosted-service.lock`, and it holds
the private-address floor whatever the policy says: `allow_private_hosts`
and `record_internal_prefixes`
([`docs/contracts/source-policy.md`](source-policy.md) §Fields) admit no loopback,
private, link-local or `.internal` address, `context_status` reports
`policy.private_floor: "held"`, and the refusal says no policy setting lifts
it. On its interval it ends idle sessions, refreshes managed policy, runs the
relay ([`docs/contracts/policy-envelope.md`](policy-envelope.md) §Cadence
and staleness) and refetches the issuer's keys. `commonmeasure doctor` and
`commonmeasure status` print the service line: not configured, configured
and not running, or running with the lock held, with the origin, the
endpoints and the interval. `commonmeasure hosted serve --listen <addr>
--origin <origin> [--allow-origin <origin>]... [--host <word>]...` is the
command-line form: the same transport, the floor left to the policy, no
lock and no interval work.

### The bearer token

Two kinds of token are accepted on a hosted endpoint
(`crates/commonmeasure-cli/src/hosted_tokens.rs`). The record names the
kind in `authentication_basis`
([`docs/contracts/session-evidence.md`](session-evidence.md) §Crossing).

An **access token** is what the hub issues as the OAuth authorisation
server: a JWT, `alg: EdDSA`, `typ: at+jwt`, header `kid` the RFC 7638
thumbprint of the hub's Ed25519 signing key. Its claims are `iss` (the
hub's origin, the `hub` of `enrolment.json`, trailing slash ignored), `sub`
(the person's hub user id, non-empty), `aud` (one string, the endpoint URL
exactly as the resource metadata's `resource`; an array naming it is also
accepted), `org` (the organisation id, equal to `enrolment.json`
`organization.id`), `client_id`, `scope`, `iat`, `exp` and `jti`. The edge
checks, in order and each refused by name: three segments; `alg`; `iss`
before any key lookup, so a foreign token costs no fetch; `kid` present and
published; the signature; `exp`, and `nbf` when present, with sixty seconds
of leeway; `aud`; `org`; `sub`. `client_id`, `scope`, `iat` and `jti` are
not checked. Keys are read from the JWKS at the `jwks_uri` the issuer's
`/.well-known/oauth-authorization-server` names, OKP Ed25519 entries by
`kid`, cached in `~/.commonmeasure/hosted-jwks.json` under the issuer, and
refetched for an unknown `kid` at most once a minute and, under `hosted
service`, once per interval. A key the hub withdraws therefore verifies
until the next refetch; no revocation list is read, and no request is made
to the hub while a crossing is ruled on.

An **edge token** is issued by the edge itself for a host with no OAuth
(the Copilot cloud agent): `commonmeasure hosted token issue <label>
[--host <word>]` prints `cmet_` and 32 random bytes once and keeps only
`sha256:<hex>` with the label, the issue time and any binding in
`~/.commonmeasure/hosted-tokens.json`. With `--host` the token is accepted
on that endpoint alone, and a presentation elsewhere is refused naming the
binding; without it the token is accepted on every endpoint. `revoke
<label>` refuses the token at its next request, with no restart, and a
label is issued once. `list` prints each label, its issue time, its
standing and its binding.

Under `observe` mode the mediated tools record everything and refuse
nothing; that is the state with no policy file
(`crates/commonmeasure-harness/src/policy.rs`). `prefer` records and steers; `strict`
refuses what the rules do not allow ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md)).

`--host` on the hook command accepts `claude-code`, `codex`, `pi`, `cursor`
and `copilot-cli`, and the three browser surfaces `chatgpt-web`,
`google-ai-overview` and `bing-copilot-search`
(`crates/commonmeasure-cli/src/main.rs` `HOSTS`); on the MCP server it
accepts `claude-code`, `codex`, `pi`, `claude-desktop`, `cursor`,
`copilot-cli` and `vscode` (`MCP_HOSTS`), one for each registration
`install` writes. An unknown name is rejected by the argument
parser with the list, never recorded under the default; the hook command
defaults an unparseable surface to Claude Code. The value is the `host` on
every record. The program's own name and version, from the `clientInfo` it
sends in the protocol's `initialize` request, are recorded beside it
([`docs/contracts/session-evidence.md`](session-evidence.md) §Client identity),
which is what tells the Codex CLI from the ChatGPT desktop app when both
run from one `[mcp_servers]` table.

The binary writes, checks and removes its own registration with a host
(`crates/commonmeasure-harness/src/registration.rs`,
`crates/commonmeasure-cli/tests/install_e2e.rs`):

| Command | Claude Code | Codex | Pi |
|---|---|---|---|
| `commonmeasure install <host> [--binary PATH]` | the five hooks in `~/.claude/settings.json` and the MCP server at user scope in `~/.claude.json` (`$CLAUDE_CONFIG_DIR` honoured), each naming the binary's resolved absolute path; the state file is written first and restored if the settings write fails | one `[mcp_servers.commonmeasure]` table in `~/.codex/config.toml` (`$CODEX_HOME` honoured), naming the binary, `mcp --host codex`, and `default_tools_approval_mode = "approve"`, without which Codex asks before every call and its non-interactive runs refuse the tools; the table is read by the Codex CLI, the ChatGPT desktop app and the Codex IDE extension | one extension, `extensions/commonmeasure/index.ts` under `~/.pi/agent` (`$PI_CODING_AGENT_DIR` honoured), naming the binary; at each session start it spawns `mcp --host pi --session <Pi's session id>` and registers the server's tools with Pi |
| `commonmeasure uninstall <host>` | exactly those entries removed; every other key keeps its value | exactly that table removed; every other table, key and comment kept byte for byte | the extension and its directory removed |
| `commonmeasure doctor [<host>]` | per host: what is registered and where, the binary each entry names and the version it reports when run, a plugin installed beside it and whether its install path and marketplace directory still exist (an enabled plugin whose files exist is reported as the registration; the double-recording warning is given only beside a direct registration), whether the sessions directory is writable, whether the policy file loads | the same, and whether the approval mode is on the table | the same, from the extension's `const BINARY` line |

Before the per-host lines, `doctor` prints the relay's delivery state for the
operator home: queued, dead and delivered batches; the last delivery from
`relay/receipts.json` with its age (`last delivery: 2026-09-16T10:00:00.000Z,
6d 2h ago`, `none recorded`, or why it is unknown), naming the receiver only
where it is not the configured one the egress lines already name; and whether
the relay runs without a person. That last line is `at each Claude Code
session end` only where `relay.json` names a receiver, the marker file
`relay/manual` is absent, and a Claude Code registration that fires the
`SessionEnd` hook is in place — its own entry in the settings file or an
enabled plugin that loads. Otherwise it is `off` with the first of those that
does not hold, and where the marker is the cause it adds that a source whose
licence demands usage reporting is refused while the marker is there
([`docs/contracts/session-evidence.md`](session-evidence.md) §Source
declarations).

Two more hosts take the same commands. `install claude-desktop` writes one
`mcpServers.commonmeasure` entry (the binary's absolute path, `mcp --host
claude-desktop`) into Claude Desktop's configuration file
(`~/Library/Application Support/Claude/claude_desktop_config.json` on
macOS, `%APPDATA%\Claude\` on Windows, `~/.config/Claude/` elsewhere),
which the application loads at its next start; Claude Desktop has no hook
surface, so the registration is mediated only, and the application starts
one server for its chat client and one for its local agent mode; each
server that makes a call leaves its own session naming its client, because
the client's identity is recorded at the first crossing and a server that
is asked for nothing leaves a log holding its start records only
([`docs/contracts/session-evidence.md`](session-evidence.md) §Client
identity). `install cursor` writes the server into `~/.cursor/mcp.json`
(`type: stdio`, `mcp --host cursor`) and four hooks into
`~/.cursor/hooks.json` (`sessionStart`, `postToolUse`, `beforeSubmitPrompt`,
`stop`, each `hook <event> --host cursor`), Cursor's names for the four
moments the Claude Code registration hooks; `uninstall` removes exactly
those entries and deletes nothing: the `mcpServers` object, the `hooks`
object and each file stay, empty where ours was the only entry, because
the host may have written them; `doctor` reads both back.

`install copilot` writes one `mcpServers.commonmeasure` entry into the
Copilot CLI's `~/.copilot/mcp-config.json` (`$COPILOT_HOME` honoured), with
`type: local`, the binary, `mcp --host copilot-cli` and `tools: ["*"]`, the
fields the CLI's own `copilot mcp add` writes. It writes four hooks into
`~/.copilot/hooks/commonmeasure.json`, a file of this product's own in the
directory from which the CLI loads every `*.json` file: `sessionStart`,
`postToolUse` with the matcher `web_fetch|web_search`, `userPromptSubmitted`
and `agentStop`. Each runs the binary directly (`exec`, with the arguments
`hook <event> --host copilot-cli`), so no shell quotes the path on any
platform. The camelCase event names select the CLI's camelCase payload
(§2). The same `mcp-config.json` is the registration for two more programs:
the GitHub Copilot app's documentation states that servers configured for
the Copilot CLI are available in it
(<https://docs.github.com/en/copilot/how-tos/github-copilot-app/customize-github-copilot-app>),
and VS Code's MCP configuration reference states that its Agent Host reads
the file natively
(<https://code.visualstudio.com/docs/agents/reference/mcp-configuration>).
`uninstall copilot` removes the entry, leaving the file and its
`mcpServers` object, and removes this product's hook entries; the hook file
is deleted when nothing else is left in it, and the hooks directory, which
is the CLI's, stays.

`install vscode` writes one `servers.commonmeasure` entry (`type: stdio`,
the binary, `mcp --host vscode`) into VS Code's user-profile `mcp.json`
(`~/Library/Application Support/Code/User/mcp.json` on macOS,
`%APPDATA%\Code\User\` on Windows, `~/.config/Code/User/` elsewhere). The
file is written directly rather than through `code --add-mcp`, which can
add an entry but not remove one and needs the `code` command on `PATH`;
VS Code's reference documents the file as the configuration a user edits.
VS Code accepts comments in the file and this writer would drop them, so a
file with a comment is refused and nothing is written. VS Code starts the
server the first time a chat uses it, and forwards the entry to its Agent
Host, where the Copilot harness runs, so Copilot agent mode in VS Code
uses this same registration. No hook is registered for VS Code. Its hooks
run only through the Copilot Chat extension, which also loads Claude Code's
hook files and ignores their matchers, and its hooks reference does not
state that a `PostToolUse` input carries the tool's result
(<https://code.visualstudio.com/docs/agent-customization/hooks>). A hook
registered there would fire on every tool call with nothing it is known to
be able to record, so the decision waits for a session with a Copilot plan
to show what the payload carries.

Every JSON file named above is rewritten whole, so every other key keeps
its value and the file comes back in this writer's formatting. A file the host or
the operator wrote in another formatting comes back with the same content
and different bytes; the byte-for-byte round trip holds only for a file
that was already in this writer's format.

`install chrome` writes one native messaging host manifest,
`ai.commonmeasure.browser.json`, into Chrome's user-level
`NativeMessagingHosts` directory (`~/Library/Application
Support/Google/Chrome/` on macOS, `~/.config/google-chrome/` on Linux),
and the same file for Chromium and Brave where that browser's directory
already exists, so nothing is created for a browser not in use. The file
names the binary's absolute path, `type: stdio`, and one allowed origin,
the Common Measure extension (`chrome-extension://hojjbnoeobjkjklcdhhnncmmojmcneig/`),
whose id is fixed by the public key in `browser/manifest.json`. Chrome
starts the binary itself with that origin as the only argument, which the
binary reads as `commonmeasure native-host`. The file is wholly this
writer's, so it is written byte for byte; `uninstall chrome` removes the
file where it names this host and leaves the directory; `doctor chrome`
reads each file back, runs the binary it names and says whether the
extension's origin is allowed. Chrome reads the directory under the user
data directory it runs with, so a browser started with `--user-data-dir`
looks for the file there and not in the default location. On Windows,
Chrome finds native messaging hosts through the registry, which `install
chrome` does not write; it refuses there and writes nothing. Whether the
extension is loaded is the browser's own record, which `doctor` does not
read; the extension's popup says whether it reaches the binary.

The absolute path is written because a host's hook environment does not
share the login shell's `PATH`. Replacing the binary at the registered path
changes what the next session runs with no reinstall. The two Claude Code
JSON files are rewritten whole, so their keys come back in sorted order and
their formatting is this writer's; the Codex TOML keeps its bytes. `install
claude` refuses while a Common Measure plugin is enabled in the settings
file, because the plugin declares the same hooks and two registrations
record every crossing twice. The operator home is never touched by
`install`, `uninstall` or `doctor` for any host.

## 2. What a host must supply

Each session-evidence event is defined by what the host supplies, not by
which hook writes it. The table states the minimum for any host and what
Claude Code, Codex and Pi provide today.

| Event | Any host must supply | Claude Code | Codex | Pi |
|---|---|---|---|---|
| `crossing_observed` | a hook after each tool call, with `session_id`, `cwd`, `tool_name`, `tool_input`, `tool_response` as JSON on stdin; `agent_type`/`agent_id` when the call ran in a subagent; the host's turn identifier where it has one | `PostToolUse` hook, matcher `WebFetch\|WebSearch\|mcp__.*`, written by `install claude` or declared in `plugin/hooks/hooks.json` | none registered: Codex's hooks documentation (`https://learn.chatgpt.com/docs/hooks`) states that hosted tools such as its web search do not use the local tool path and fire no hook, and its shell reaches the web as command text, so a matcher could witness only third-party MCP results; Codex hooks do carry `tool_response` for MCP tools, so a third-party fetch server could be observed, and none is registered | none: Pi has no web tool of its own to observe |
| `crossing_mediated`, `crossing_refused`, `processor_invoked` | run the MCP server and route the agent's fetch and search through its tools; the server takes `cwd` from the directory it was started in | the user-scope entry `install claude` writes, or `.mcp.json` in the plugin | the `[mcp_servers.commonmeasure]` table `install codex` writes | the extension `install pi` writes, which spawns the server with Pi's session id in the session's working directory |
| `turn_started` | a hook at prompt submission with `session_id`, `cwd`, `transcript_path` and the host's turn identifier (`prompt_id` or `turn_id`) where it has one | `UserPromptSubmit` hook, `prompt_id` | not registered | not supplied |
| `prompt_sources` | the same hook with the `prompt` text; without it, every mediated crossing's `named_by` is `unknown` | `UserPromptSubmit` hook, `prompt` | not registered | not supplied |
| `turn_completed` | a hook at the end of a turn with the same fields | `Stop` hook, `prompt_id` | not registered | not supplied |
| `context_snapshot` | a transcript in the shape `crates/commonmeasure-harness/src/snapshot.rs` reads: the provider's usage counters beside each model call | `Stop` hook reads the transcript at `transcript_path` | not supplied | not supplied |
| `nudge_issued` | a hook at session start whose stdout the host adds to the session's context; `source` names how the session began | `SessionStart` hook | not registered | not supplied |
| `host_process` | nothing beyond starting the hook and the server as child processes: each reads its own ancestry from the operating system's process table ([`session-evidence.md`](session-evidence.md) §Host process) | `SessionStart` hook and the MCP server's start | the MCP server's start; no hook is registered, and the server takes no session identifier from Codex, so its log joins nothing | the MCP server's start; Pi passes its session identifier, so there is one log and nothing to join |
| `session_ended` | a hook at session end with `session_id`, and the host's reason word as `reason` where it has one | `SessionEnd` hook, `reason` | not registered | not supplied |
| `crossing_reconstructed` | a transcript that records tool results, importable after the fact | `~/.claude/projects` | reported not importable, with the reason | reported not importable, with the reason |

Cursor supplies the observed path's events under its own names and
shapes, which the hook command reads when told `--host cursor`:
`postToolUse` carries `tool_name`, `tool_input` and `tool_output` (the
tool's result as JSON text) for every tool, so a third-party MCP result is
a `crossing_observed`, retrieved and not grounded, and our own tools under
Cursor's `MCP:<tool>` spelling are excluded; `beforeSubmitPrompt` carries
`prompt` and gives `prompt_sources` and `turn_started`; `stop` gives
`turn_completed`; `sessionStart` receives the nudge as the JSON field
`additional_context` and gives `nudge_issued`. The session identity is
Cursor's `conversation_id`, the turn identifier its `generation_id`, the
working directory its `cwd` or the first of `workspace_roots`. Cursor's
built-in web tool is not named in its documentation's matcher list, so its
results are not claimed as observed until a hook has run against one.

The hook command reads a session-end event as `session-end` from any host
it reads the other events from, and where `relay.json` names a receiver and
the `relay/manual` marker is absent it starts a relay run in the background
without waiting for it
([`docs/contracts/telemetry-projection.md`](telemetry-projection.md) §Relay at
session end). `install` registers it for Claude Code only. Cursor and the
Copilot CLI each document a `sessionEnd` event in their hooks
references, and `install cursor` and
`install copilot` do not register it, so neither records `session_ended`;
their `sessionStart` hook does record `host_process`. Whether VS Code's
Copilot Chat sends a session-end event is not established here. Because
no other host's install sends the event, a mediated session under any
`--host` word but `claude-code`, or under `claude-code` from a client that
does not announce itself as `claude-code` (Goose, registered by hand with no
`--host`, is one), has no automatic delivery, and a licence's reporting
demand is unmet and refused there, unless a running `hosted service` holds
the same home and relays it on its interval
([`docs/contracts/session-evidence.md`](session-evidence.md) §Source
declarations). A host
that runs Claude Code's `SessionEnd` command from its hook files is
refused or accepted by the rules below, as at the other four events.

The Copilot CLI supplies the observed path's events under its camelCase
names and payload, which the hook command reads when told `--host
copilot-cli`
(<https://docs.github.com/en/copilot/reference/hooks-reference>):
`postToolUse` carries `toolName`, `toolArgs` and
`toolResult.textResultForLlm`, the text the model was given, so a
`web_fetch` is a grounded `crossing_observed` hashed over that text and a
`web_search` records each result URL, retrieved and not grounded; the
record keeps the CLI's tool names. The matcher names only those two tools,
so our own tools (spelt `commonmeasure-context_fetch` in the CLI's
`<server>-<tool>` form) and third-party MCP tools never reach the hook.
`userPromptSubmitted` carries `prompt` and gives `prompt_sources` and
`turn_started`; `agentStop` gives `turn_completed`; `sessionStart` receives
the nudge as the JSON field `additionalContext` and gives `nudge_issued`
with the CLI's `source`. The session identity is the CLI's `sessionId`; the
CLI supplies no turn identifier, so none is recorded. The reader accepts
only that shape: a payload without `sessionId`, or carrying `session_id`,
`tool_name` or Cursor's `conversation_id`, records nothing and prints
nothing. The hooks reference does not name `web_fetch`'s arguments; the
installed CLI's bundle declares `url`. The same bundle carries a hosted web
search for one model provider that does not run as a local tool, so a
`web_search` is observed only when it runs locally.

A hook command told `--host claude-code` refuses a payload carrying
Cursor's fields, the Copilot CLI's camelCase fields, or `timestamp` or
`tool_result`, and records nothing, and at session start prints nothing.
Cursor and VS Code load Claude Code's hook files and run their commands
with payloads of their own, and the Copilot CLI loads a repository's
`.claude/settings.json`. VS Code's Copilot Chat, and the Copilot CLI for a
hook configured under Claude Code's event names, send a snake_case payload
with Claude Code's own field names (`session_id`, `hook_event_name`,
`tool_name`, `tool_input`) beside a `timestamp`, which Claude Code's hooks
reference lists for no event
(<https://code.claude.com/docs/en/hooks>); `timestamp` is what tells the
two apart (`crates/commonmeasure-harness/src/hook.rs`
`HookInput::from_payload`).
Field names alone do not identify Claude Code, because other hosts run
Claude Code's hook commands and some send Claude Code's own shape. A hook
command told `--host claude-code` refuses whatever the payload looks like
when it finds another host's variable in its environment:

| Variable | Host | Why that host runs Claude Code's hooks |
|---|---|---|
| `CURSOR_PROJECT_DIR` | Cursor | loads Claude Code's hook files when its "include third-party plugins" switch is on, and its documentation does not say which shape those hooks receive |
| `GEMINI_SESSION_ID` | Gemini CLI | `gemini hooks migrate` copies Claude Code's hooks into Gemini's settings; its hooks reference lists the variable (<https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/index.md>) |
| `DEVIN_PROJECT_DIR` | the Devin CLI | runs the hooks in `~/.claude/settings.json` by default, with payloads in Claude Code's shape (<https://docs.devin.ai/cli/extensibility/hooks/overview.md>) |
| `GROK_SESSION_ID` | Grok Build | scans `~/.claude/settings.json` for hooks by default (<https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md>) |

Gemini CLI and Grok Build also set `CLAUDE_PROJECT_DIR` for every hook, so
that variable does not identify Claude Code. The refused hook records
nothing and at session start prints nothing, because naming the host from
the variable would record a host the command was not told
(`crates/commonmeasure-harness/src/hook.rs` `FOREIGN_ENVIRONMENT`). A
Claude Code payload that is not JSON still gets the nudge, as §2 states; a
payload that is JSON and refused gets nothing.

The mediated half is not refused this way. The Devin CLI, Grok Build and
Oh My Pi read Claude Code's `~/.claude.json`, so they start the server that
entry names, with `--host claude-code`, and its records say
`host: claude-code`; the client's own name in `client` is what tells them
apart (`docs/contracts/session-evidence.md` §Client identity).

### Browser answer surfaces

ChatGPT on the web, Google AI Overviews and Bing Copilot Search answer
with the model's own web search, which crosses no tool this product can
offer, so none of them can be mediated. The extension in `browser/`
observes what each page shows and sends the binary one message per
answer over Chrome native messaging (`crates/commonmeasure-harness/src/browser.rs`,
`crates/commonmeasure-cli/tests/browser_e2e.rs`):

```json
{"host": "chatgpt-web", "session": "<conversation id>",
 "retrieved": ["https://…"], "cited": ["https://…"]}
```

The binary answers each message with `{"recorded": <n>, "session": "<id>"}`,
or `{"recorded": 0, "error": "<reason>"}`, and a message `{"status": true}`
with its version and whether a session log can be written. The same message
on stdin to `commonmeasure hook post-tool-use --host <surface>` is read the
same way.

| Event | ChatGPT on the web | Google AI Overviews | Bing Copilot Search |
|---|---|---|---|
| `crossing_observed` | the conversation's event stream, teed in the page and parsed in the extension (`browser/parse.js`): search-result links and citation links | the external links in the rendered overview block | the citation links in the rendered Copilot answer block, unwrapped from Bing's click redirects |
| session identity | the stream's `conversation_id` | none | none |
| every other event | not supplied | not supplied | not supplied |

How a message becomes records:

- **One observed crossing per distinct source**, `retrieved` and `cited`
  together, under the privacy floor and the named internal prefixes as
  every observed crossing is. Each is `grounded: false` with no
  `content_hash` and no `estimated_tokens`: no surface exposes the page text
  the model read, so nothing about it is claimed. Grounding is never
  recorded from these surfaces.
- **A citation is recorded as the retrieved crossing it also is.** The
  session-evidence contract has no field that says a source was cited in the
  answer, so that fact is not recorded; a citation missing from `retrieved`
  is still recorded once as retrieved.
- **No `tool`, `cwd` or `turn_id`.** A page shows sources, not the tool call
  that found each; a browser runs in no working directory, so no policy
  scope and no governing engagement resolves for these crossings, and the
  relay, which projects only crossings under a scope cleared for egress,
  never sends them; no surface names the turn.
- **Session.** ChatGPT's conversation id is the session. Google and Bing
  expose no identity for a search, so each answer is a session the binary
  names `local-<millis>-<pid>`, as the MCP server names one; a later
  message carrying sources the page added to the same answer is a second
  session. A session identifier that is not a plain name (letters, digits,
  `-`, `_`, `.`, not leading `.`) is refused and nothing is recorded,
  because it names a file and a page controls it.
- **What the record rests on.** The extension records what the page
  delivered. A script running in the ChatGPT page can post to the channel
  the capture script uses, and Google and Bing control the markup read, so
  a page could put a source in the record that its model never saw. Only
  the Common Measure extension may start the binary.

Facts behind the table:

- **Client identity.** The MCP server records the `clientInfo` name and
  version the client sends in `initialize` once, as `client_identified`,
  and on each mediated crossing as `client`; a client that sends none
  leaves neither. `host` stays the registration's word.
- **Session id.** A hook uses the host's `session_id`; a missing one is
  recorded as `unknown-session`. The MCP server uses `--session` when given,
  then `AGENT_SESSION_ID` from its environment (which Goose sets), and
  otherwise generates `local-<millis>-<pid>` (`main.rs` `mcp_session_id`).
  An identifier that is not a plain name is refused by both paths
  ([`docs/contracts/session-evidence.md`](session-evidence.md) §Where). A
  host that wants its hook records and its mediated records in one log
  must pass its own session id to both.
- **Working directory.** Hooks read `cwd` from the payload. The MCP server
  reads its own current directory once at start, resolves the policy scope
  against it, and stamps it on every mediated crossing
  (`main.rs` `serve_mcp`). A host must therefore launch the server in the
  directory the work belongs to.
- **Principal.** The mediated path resolves the principal from the process's
  effective operating-system user (`policy.rs` `Principal::current`). A label
  in the payload or the environment is never authority. On a platform where
  no user can be read, the basis is recorded as `unavailable`, and a policy
  file that declares principals refuses every mediated crossing there
  ([`docs/contracts/session-evidence.md`](session-evidence.md) §Crossing). Observed capture claims
  no principal.
- **Turn identifier.** A hook payload's `prompt_id` (Claude Code) or
  `turn_id` (Codex) is recorded as `turn_id` on the boundary and on each
  observed crossing in the turn; nothing else about the turn is read, and
  the boundary declares `privacy_level: minimal`
  ([`docs/contracts/session-evidence.md`](session-evidence.md) §Turn boundaries).
- **Payload shape.** `HookInput` (`hook.rs`) reads `session_id`,
  `transcript_path`, `cwd`, `hook_event_name`, `tool_name`, `tool_input`,
  `tool_response`, `agent_type`, `agent_id`, `prompt_id`, `turn_id`, `source`
  and `prompt`. Every field is
  optional; a host that supplies more is not rejected, and a host that
  supplies less produces fewer records. The event name comes from the
  command argument when the payload omits `hook_event_name`.
- **Which tools are observed.** `grounding::ToolKind::classify` maps the
  host's tool name to a fetch, a search or a third-party MCP result. A tool
  it does not know yields no crossing rather than a guessed one. The
  runtime's own mediated tools are excluded from observation so a crossing
  is not recorded twice.
- **Private addresses.** Observed capture never records loopback, private
  network, `.local`, `.internal` or `file://` crossings unless the operator
  names a prefix in `record_internal_prefixes`. A host cannot lower that
  floor (`policy.rs`).

## 3. The screening-proxy socket

Status: `planned`. No code in this repository serves an HTTP screening
endpoint. The binary runs two HTTP servers: the operator console, which reads
evidence and takes the writes [`docs/GLOSSARY.md`](../GLOSSARY.md) §Console lists
(`crates/commonmeasure-console`), and the hosted service, which serves the
mediated path as MCP over Streamable HTTP (§1, The hosted path). Neither
accepts a screening request.

Some harnesses already expose this socket from their side: the harness
posts the text it is about to admit to an operator-configured endpoint,
typically with the hook and some metadata, expects a score against a
threshold, and acts on the verdict, sometimes in a shadow mode that records
agreement between its own screen and the proxy.

What Common Measure offers on that socket, when built:

- **Request.** The harness posts the candidate text, the URL or source
  identifier, the host and session id, and the working directory, over
  HTTPS on loopback or a private network.
- **Response.** The admission ruling as the runtime already computes it for
  the mediated path (`commonmeasure_runtime::policy::Ruling`): admitted, or refused
  with the named rule, plus each processor's finding namespaced under the
  processor ([`docs/contracts/processor.md`](processor.md) §Evidence).
- **Timeout and failure.** The rule follows the policy mode in force for the
  scope the working directory resolves to: under `strict` a timeout or an
  unreachable endpoint is a refusal; under `observe` and `prefer` the harness
  proceeds and the runtime records an `evidence_gap` naming the window it
  could not judge ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §2).
- **Evidence.** One `crossing_mediated` or `crossing_refused` record and one
  `processor_invoked` record per processor, in the same session log the
  MCP server writes, with the hash of the text judged.

An endpoint that returned a score without the ruling and the record would
not be this product; the ruling and the record are the contract.

## 4. Embedding the crates

The workspace crates are the same code the binary runs
(`crates/*/Cargo.toml`). A harness written in Rust can link them directly:

| Crate | What it exposes |
|---|---|
| `commonmeasure-types` | the value types: `ContextJob`, `Constraint`, `PolicyMode`, `ContextEnvelope`, `AllowanceDeclaration`, `Money` |
| `commonmeasure-runtime` | `policy` (the admission rules and `Ruling`), `run` (a job becoming a run), `evidence` (`EvidenceLog`, `RunDirectory`), `allowance` (the reservation ledger), `processor`, `replay`, `selection` |
| `commonmeasure-supply` | `SupplyAdapter`, `supplier_from_environment`, `IMPLEMENTED_PROVIDERS`, `required_variable` |
| `commonmeasure-harness` | `SessionLog`, `SessionPolicy`, `HookInput`, `HostSurface`, `capture`, `mcp::McpServer`, `import` |
| `commonmeasure-relay` | the Content Telemetry projection and delivery |
| `commonmeasure-console` | the console: the SQLite index over the evidence logs and the loopback server |

The crates are not a stable API. They are published nowhere, carry no
semantic-versioning promise, and their public items change when the binary
needs them to. An integrator who links them takes the workspace at a commit
and re-reads this table on update. The stable surfaces are the command line,
the MCP tools, the policy file and the evidence records, each governed by
its own contract in this directory.

## 5. What runs where

- **Everything decides locally.** Policy, admission, the allowance ledger,
  the processors and the evidence log run in the operator's process on the
  operator's machine, reading `~/.commonmeasure` (or `$COMMONMEASURE_HOME`).
  Managed source-policy enforcement does not wait on a Hub call.
- **The relay is the only egress.** `commonmeasure relay` sends cleared records
  to a receiver the operator configured and refuses to run without one; on a
  local edge the session-end hook starts the same command when a session ends,
  and the hosted service runs it on its interval, unless the operator wrote
  the `relay/manual` marker
  ([`docs/GLOSSARY.md`](../GLOSSARY.md) §Relay). Records leave only under a scope with a named
  governing engagement and explicit clearance ([`docs/contracts/telemetry-projection.md`](telemetry-projection.md) §Selected coverage).
- **A hosted edge is the same edge on another machine.** `commonmeasure
  hosted service` runs one enrolled, managed edge per organisation in the
  operator's or Common Measure Ltd's cloud, holding the operator record on
  that machine's disk and taking its policy from the hub as any managed
  edge does. Its relay and policy refresh run in the process on an
  interval. Where the machine sits beside a cloud metadata service the
  private-address floor is held whatever the policy says (§1). A hosted
  session has no client working directory. With no declared
  `session_directory`, no directory scope matches and its crossings remain
  private. An explicitly enrolled service directory uses the existing signed
  reporting-approval path (§1); it creates no automatic egress permission.
- **Local policy remains authoritative.** Common Measure Hub receives cleared
  records and coordinates managed policy. Current acquisition does not require
  Hub entitlement issuance. The planned organisational entitlement route may
  require online issuance or validation; it must fail unavailable when required
  authority cannot be obtained. Hub absence never relaxes local policy.
- **The model is never the runtime's.** In a harness session the host's own
  model answers; the runtime never calls one. In a batch run the model is
  reached through the gateway named by `COMMONMEASURE_INFERENCE_ENDPOINT`
  ([`docs/GETTING-STARTED.md`](../GETTING-STARTED.md)).
- **A host can report what it did with the content.** A host that drives the
  MCP server itself can opt in to the `commonmeasure/observe` ingress
  ([session evidence](session-evidence.md#acquisition-handles)) before
  fetching, fetch through durable acquisition handles, and submit hash-only
  context and output observations. The edge appends them to the same session
  file as the crossings. Loopback sources used this way stay private under
  the relay's address floor.

## 6. Verification state of each path

States are the ones [`docs/contracts/provider.md`](provider.md) §Verification states
defines and are never collapsed.

| Path | State | Evidence |
|---|---|---|
| Claude Code, observed (hooks) | `fixture-tested` | `crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the host's payload shapes |
| Claude Code, mediated (MCP over stdio) | `live-verified` (bounded configuration) | Claude Code 2.1.273 on macOS 26.4 admitted the public Common Measure page and refused `example.com` in an interactive session under hosted policy and a signed reporting approval. The host tool response hash matches the source record; the selected session reached the correct Hub organisation and a separate unselected host session stayed local. The trial ran against the hosted Hub; its configuration, limits and redacted records are not published. Loopback refusal and privacy-floor tests remain in `mediated_e2e.rs`. |
| Claude Code, reconstructed (import) | `fixture-tested` | `crates/commonmeasure-cli/tests/import_e2e.rs` |
| Claude Code, registration (`install`, `uninstall`, `doctor`) | `live-verified` for installation and use; remaining operations `fixture-tested` | The same trial ran `install claude` into an isolated configuration directory and loaded the generated hooks/MCP files into Claude Code 2.1.273 using its explicit settings/config options, and the interactive session of the mediated Claude Code row ran through them. `install_e2e.rs` still covers removal and diagnosis. |
| Codex CLI, registration and mediated | `fixture-tested` | the `[mcp_servers.commonmeasure]` table `install codex` writes, with `default_tools_approval_mode = "approve"`, is pinned by `crates/commonmeasure-cli/tests/install_e2e.rs`; the session a real Codex CLI run (`codex exec`, client `codex-mcp-client` 0.154.0) recorded through that table, one mediated fetch of `https://example.com`, is read by `crates/commonmeasure-cli/tests/recorded_sessions.rs` from a recording that is not published |
| ChatGPT desktop app (Codex), the same registration | `spec-verified` | Codex's documentation states the app reads the same table; no session through the app is recorded here |
| Claude Desktop, registration | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the `mcpServers` entry around the operator's keys, content-exact, and byte for byte for a file already in this writer's format; the application's own MCP log on one operator machine recorded the server start and tool listing from that entry, which is the operator's record and not committed |
| Claude Desktop, mediated | `planned` | no session through the application is recorded; the server it starts is the Claude Code one, but the row is earned by a recorded session and none exists |
| Cursor, registration | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the server and the four hooks around a foreign server and hook, content-exact, and byte for byte for files already in this writer's format; Cursor's own log on one operator machine recorded the server start from `~/.cursor/mcp.json`, not committed |
| Cursor, mediated | `planned` | no session through Cursor is recorded |
| Cursor, observed | `spec-verified` | `crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the payload shapes Cursor's hooks documentation states (a third-party MCP result recorded under `conversation_id`, our own tools excluded, the nudge as `additional_context`, the prompt and stop boundaries) and with foreign shapes and Cursor's environment fed to the Claude Code reader; no payload recorded from Cursor exists, so the shapes are the documentation's and not a fixture |
| Copilot CLI, registration | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the `mcp-config.json` entry beside a foreign server, byte for byte, and the hook file with its four hooks, byte for byte, keeping an entry someone else added to it; on one operator machine the CLI listed the entry (`copilot mcp list`: `commonmeasure (local)`) and refused to start it, "1 MCP server was blocked by policy", because the account has no Copilot plan; not committed |
| Copilot CLI, observed | `spec-verified` | `crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the camelCase payload shapes the CLI's hooks reference states (`web_fetch` grounded under `sessionId`, `web_search` retrieved, our own tool excluded, the nudge as `additionalContext`, the prompt and stop boundaries) and refuses other shapes; no payload recorded from the CLI exists, and no hook ran on the operator machine because the session ended at the missing plan |
| Copilot CLI, mediated | `planned` | the server was refused before it started; a call needs an account whose Copilot plan allows MCP |
| GitHub Copilot app, the Copilot CLI registration | `spec-verified` | the app's documentation states that servers configured for the Copilot CLI are available in it; the app was not installed and no session is recorded |
| Copilot agent mode in VS Code, the VS Code and Copilot CLI registrations | `spec-verified` | VS Code's MCP configuration reference states that VS Code forwards its configured servers to the Agent Host, where the Copilot harness runs, and that the Agent Host reads `~/.copilot/mcp-config.json` natively; which of the two entries it starts when both exist is not documented, and `doctor vscode` says so; a call needs a Copilot plan; no hook is registered (§1) |
| VS Code, registration | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the `servers` entry beside a foreign server and `inputs`, byte for byte, keeps the content of a tab-indented file as VS Code writes it, and refuses a file with a comment; on one operator machine VS Code, opened with the entry in place, created the server's output log `mcpServer.mcp.config.usrlocal.commonmeasure.log`, empty because the server starts on first use in a chat; not committed |
| VS Code, mediated | `planned` | no session is recorded; a call needs a chat with a model, through Copilot or a model key for VS Code's own harness |
| Codex IDE extension, the same registration | `spec-verified` | Codex's documentation states the extension reads the same table, and the installed extension's bundle resolves that file; no session through the extension is recorded here |
| Acquisition handles and host observations (`commonmeasure/observe`) | `fixture-tested` | `crates/commonmeasure-harness/src/observations.rs` tests opt-in, handles surviving a reopened log, duplicate and failed-write rejection, observations bound to admitted bytes, and a killed writer's output joining its acquisition; `crates/commonmeasure-cli/tests/mediated_e2e.rs` drives the ingress through the real binary; `crates/commonmeasure-relay/src/project.rs` tests projection counts and privacy with public-shaped fixture records. No host outside this repository's tests is recorded using it, and no supplier or independent receiver acceptance is claimed. |
| Pi, registration and mediated | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the extension; a real Pi session recorded through it is read by `crates/commonmeasure-cli/tests/recorded_sessions.rs` from a recording that is not published |
| Gemini CLI, mediated | `planned` | no `install` for the host; a hand-written `mcpServers` entry started the server and listed the tools before sign-in on one operator machine; no session through it is recorded |
| Gemini CLI, observed | `planned` | no reader for Gemini's `AfterTool` exists; a hook Gemini runs from Claude Code's registration is refused under `GEMINI_SESSION_ID` and, carrying `timestamp`, by its shape (`crates/commonmeasure-cli/tests/hook_e2e.rs`) |
| Zed, mediated | `planned` | no `install` for the host; a `context_servers` entry started one server per open project on one operator machine; no session through it is recorded |
| Cline, mediated | `planned` | no `install` for the host; an entry in `cline_mcp_settings.json` started the server before any account on one operator machine; no session through it is recorded |
| OpenCode, mediated | `planned` | no `install` for the host; `opencode mcp list` connected to a hand-written entry on one operator machine; the host's own run failed before a call |
| Goose, mediated | `planned` | no `install` for the host; a session extension started the server and passed `AGENT_SESSION_ID`, which the server takes as the session id when no `--session` is given (`crates/commonmeasure-cli/tests/mediated_e2e.rs`); no session through Goose is recorded |
| Google Antigravity, mediated | `planned` | no `install` for the host; an `agy mcp add` entry started the server before sign-in on one operator machine |
| JetBrains Junie, mediated | `planned` | the server answers `initialize` with the `2025-03-26` Junie's client asks for (`crates/commonmeasure-cli/tests/mediated_e2e.rs`); on one operator machine Junie refused a server that answered `2025-06-18`, and connected to and listed the tools of one that answered `2025-03-26`; no call through Junie is recorded |
| Devin CLI, Kiro, Amp, mediated | `planned` | registration written with each host's own command; each stops at sign-in before starting a server |
| Chrome, registration (`install chrome`, `uninstall`, `doctor`) | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes the manifest byte for byte for Chrome and Brave around another host's manifest, skips a browser whose directory does not exist, reads it back and removes it; `crates/commonmeasure-harness/src/registration.rs` checks the allowed extension id against the key in `browser/manifest.json`; on one operator machine Chrome loaded the unpacked extension and its popup reached the binary through the written manifest, which is the operator's record and not committed |
| ChatGPT on the web, observed | `fixture-tested` | `browser/test/parse.test.js` holds the stream parser to the assertions of the Rust parser it was ported from, over a compact stream in the shape ChatGPT sends, and asserts it produces the message `crates/commonmeasure-cli/tests/browser_e2e.rs` feeds the real binary over native messaging framing; no ChatGPT turn has been recorded through the extension |
| Google AI Overviews, observed | `planned` | the message path is the one `crates/commonmeasure-cli/tests/browser_e2e.rs` drives; on the live results page the overview's links are opaque `/goto?url=` tokens that name no destination, so the page reader finds no source and records nothing |
| Bing Copilot Search, observed | `fixture-tested` | the message path is driven by `crates/commonmeasure-cli/tests/browser_e2e.rs`; the page reader has no committed test; one search on `bing.com/search` on an operator machine recorded five observed crossings from the Copilot answer through Chrome, which is the operator's record and not committed |
| Hosted edge, transport and resource server (`hosted serve`, `hosted service`) | `fixture-tested` | `crates/commonmeasure-cli/tests/hosted_mcp.rs` drives the real binary over HTTP against a loopback origin and a loopback issuer that serves a JWKS and signs tokens: a fetch recorded under the subject, two concurrent sessions, the refusal table, fifteen token checks refused by name, a revoked and a bound edge token; `crates/commonmeasure-cli/tests/hosted_service.rs` runs the service against a loopback hub: the three refusals to start and the locked home, five private addresses refused under a managed policy that admits them, the relay and the key refresh on the interval with no command run |
| Claude custom connector (`/mcp/claude-connector`), mediated | `planned` | no `initialize` from the host is recorded; which `Origin` and `clientInfo` it sends is unknown until one is |
| ChatGPT (`/mcp/chatgpt`), mediated | `planned` | the same |
| Microsoft 365 Copilot (`/mcp/m365-copilot`), mediated | `live-verified` | Personal package 1.0.5: static OAuth with explicit resource, S256 and confidential-client authentication; approved AnyApp/HomeTenant vault setting. Sydney 1.0.0 negotiated MCP 2025-11-25. A paired session acquired an allowed public page and refused a denied host under `oauth_subject`; approved metadata and refusal count reached Hub. `recorded_sessions.rs` reads back the redacted capture, which is not published. Package 1.0.6 discovers the tools dynamically, with no pinned definitions; status, admission and refusal are verified through it. Output-use observation and Studio are unverified. |
| Copilot cloud agent (`/mcp/copilot-cloud-agent`), mediated | `planned` | the same; authenticates with an edge token, so it needs no authorisation server |
| Screening-proxy socket | `planned` | no implementation |
| Crates linked directly | no state claimed | the crates are the binary's own dependencies; no external consumer is evidenced |

Operator sessions that are not published do not establish a live claim here,
except where a row above names what was verified and how. The bounded hosted
Claude Code trial keeps its redacted source records and raw evidence
unpublished. `demo/arc/regenerate.sh` produces a complete session store
offline from the same binary, which is the committed demonstration of both
paths end to end (`demo/RUN-THE-DEMONSTRATION.md`).


## Directory enrolment surfaces

[Directory enrolment](directory-enrolment.md) adds a Claude plugin command,
an explicitly invoked Codex skill and a local MCP prompt. `context_enrol`
requires the host project directory explicitly and refuses a missing/root
identity or disagreement with the MCP process directory. `context_status`
reports the process cwd. A CLI enrolment in another directory does not retarget
an already-running server. MCP advertises `prompts` and implements `prompts/list`
and `prompts/get`; the host must expose prompts to make them user-invocable.
Plugin/Codex invocation is specification-verified, with fixture-tested packaging
and production CLI/MCP boundaries; no interactive host verification is claimed.
