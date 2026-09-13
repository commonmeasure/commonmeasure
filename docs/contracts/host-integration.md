---
title: Host integration contract
---

# Host integration contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This is the document an organisation reads to put Common Measure inside an
agent system it builds or runs itself. Common Measure is not a harness and not
an agent framework: it is the component between an agent and the content
the agent reads. A **harness** is the agent system the operator runs, bought
or built. A **host** is the specific program the integration attaches to;
Claude Code, Codex and Pi are the hosts integrated today. Terms like
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

The observed path is a set of lifecycle hooks. It needs no credentials and
no policy file, and it never breaks the agent: unreadable input yields no
record and the process exits zero whatever happened (`crates/commonmeasure-cli/src/main.rs`
`hook`, `crates/commonmeasure-harness/src/hook.rs`).

The mediated path is an MCP server that offers three tools, `context_fetch`,
`context_search` and `context_status` (`crates/commonmeasure-harness/src/mcp.rs`
`tool_definitions`). An agent that calls them instead of its own tools gives
the runtime the crossing before it happens, which is the only point at which
policy can refuse. Both paths append to the same session log; every record
carries which path wrote it ([`docs/contracts/session-evidence.md`](session-evidence.md) §Crossing).

Under `observe` mode the mediated tools record everything and refuse
nothing; that is the state with no policy file
(`crates/commonmeasure-harness/src/policy.rs`). `prefer` records and steers; `strict`
refuses what the rules do not allow ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md)).

`--host` accepts `claude-code`, `codex` and `pi` (`crates/commonmeasure-cli/src/main.rs`
`HOSTS`). An unknown name is rejected by the argument parser; the hook
command defaults an unparseable surface to Claude Code.

The binary writes, checks and removes its own registration with a host
(`crates/commonmeasure-harness/src/registration.rs`,
`crates/commonmeasure-cli/tests/install_e2e.rs`):

| Command | Claude Code | Codex | Pi |
|---|---|---|---|
| `commonmeasure install <host> [--binary PATH]` | the four hooks in `~/.claude/settings.json` and the MCP server at user scope in `~/.claude.json` (`$CLAUDE_CONFIG_DIR` honoured), each naming the binary's resolved absolute path; the state file is written first and restored if the settings write fails | one `[mcp_servers.commonmeasure]` table in `~/.codex/config.toml` (`$CODEX_HOME` honoured) | one extension, `extensions/commonmeasure/index.ts` under `~/.pi/agent` (`$PI_CODING_AGENT_DIR` honoured), naming the binary; at each session start it spawns `mcp --host pi --session <Pi's session id>` and registers the server's tools with Pi |
| `commonmeasure uninstall <host>` | exactly those entries removed; every other key keeps its value | exactly that table removed; every other table, key and comment kept byte for byte | the extension and its directory removed |
| `commonmeasure doctor [<host>]` | per host: what is registered and where, the binary each entry names and the version it reports when run, a plugin installed beside it and whether its install path and marketplace directory still exist, whether the sessions directory is writable, whether the policy file loads | the same | the same, from the extension's `const BINARY` line |

The absolute path is written because a host's hook environment does not
share the login shell's `PATH`. Replacing the binary at the registered path
changes what the next session runs with no reinstall. The two Claude Code
JSON files are rewritten whole, so their keys come back in sorted order and
their formatting is this writer's; the Codex TOML keeps its bytes. `install
claude` refuses while a Common Measure plugin is enabled in the settings
file, because the plugin declares the same hooks and two registrations
record every crossing twice. The operator home is never touched by any of
the three.

## 2. What a host must supply

Each session-evidence event is defined by what the host supplies, not by
which hook writes it. The table states the minimum for any host and what
the three integrated hosts provide today.

| Event | Any host must supply | Claude Code | Codex | Pi |
|---|---|---|---|---|
| `crossing_observed` | a hook after each tool call, with `session_id`, `cwd`, `tool_name`, `tool_input`, `tool_response` as JSON on stdin; `agent_type`/`agent_id` when the call ran in a subagent; the host's turn identifier where it has one | `PostToolUse` hook, matcher `WebFetch\|WebSearch\|mcp__.*`, written by `install claude` or declared in `plugin/hooks/hooks.json` | none registered: Codex's hooks documentation (`https://learn.chatgpt.com/docs/hooks`) states that hosted tools such as its web search do not use the local tool path and fire no hook, and its shell reaches the web as command text, so a matcher could witness only third-party MCP results; that reading rests on the document, no Codex session having been recorded (`DECISIONS.md` §Integration and ownership) | none: Pi has no web tool of its own to observe |
| `crossing_mediated`, `crossing_refused`, `processor_invoked` | run the MCP server and route the agent's fetch and search through its tools; the server takes `cwd` from the directory it was started in | the user-scope entry `install claude` writes, or `.mcp.json` in the plugin | the `[mcp_servers.commonmeasure]` table `install codex` writes | the extension `install pi` writes, which spawns the server with Pi's session id in the session's working directory (`demo/host-sessions/pi/`) |
| `turn_started` | a hook at prompt submission with `session_id`, `cwd`, `transcript_path` and the host's turn identifier (`prompt_id` or `turn_id`) where it has one | `UserPromptSubmit` hook, `prompt_id` | not registered | not supplied |
| `prompt_sources` | the same hook with the `prompt` text; without it, every mediated crossing's `named_by` is `unknown` | `UserPromptSubmit` hook, `prompt` | not registered | not supplied |
| `turn_completed` | a hook at the end of a turn with the same fields | `Stop` hook, `prompt_id` | not registered | not supplied |
| `context_snapshot` | a transcript in the shape `crates/commonmeasure-harness/src/snapshot.rs` reads: the provider's usage counters beside each model call | `Stop` hook reads the transcript at `transcript_path` | not supplied | not supplied |
| `nudge_issued` | a hook at session start whose stdout the host adds to the session's context; `source` names how the session began | `SessionStart` hook | not registered | not supplied |
| `crossing_reconstructed` | a transcript that records tool results, importable after the fact | `~/.claude/projects` | reported not importable, with the reason | reported not importable, with the reason |

Facts behind the table:

- **Session id.** A hook uses the host's `session_id`; a missing one is
  recorded as `unknown-session`. The MCP server uses `--session` when given
  and otherwise generates `local-<millis>-<pid>` (`main.rs` `uuid_like_session`).
  A host that wants its hook records and its mediated records in one log
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
endpoint; the only HTTP server is the operator console, which reads
evidence and takes the writes `DECISIONS.md` §Execution and evidence lists
(`crates/commonmeasure-console`). The mediated path exists only as MCP over stdio.

The socket is a contract some harnesses already expose from their side:
a harness posts the text it is about to admit to an operator-configured
endpoint and acts on the verdict. The QM harness does this with
`SECURITY_SCREEN_BACKEND=proxy`, posting `{text, hook, metadata}` and
expecting `{score, threshold, primary_outcome}`, with a shadow mode that
audits agreement between its own model and the proxy per event, and every
tool call from its adapters landing in one `ToolContext` seam
([`docs/knowledge-base/qm-harness-review.md`](../knowledge-base/qm-harness-review.md)).

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
  Nothing waits on a network call to rule on a crossing.
- **The relay is the only egress.** `commonmeasure relay` sends cleared records
  to a receiver the operator configured and refuses to run without one
  ([`docs/GLOSSARY.md`](../GLOSSARY.md) §Relay). Records leave only under a scope with a named
  governing engagement and explicit clearance ([`docs/contracts/session-evidence.md`](session-evidence.md)).
- **The hub is never in the decision path.** Common Measure Hub, a separate
  hosted service, receives what the relay sends and shows an organisation its
  fleet's evidence. Its absence never relaxes local policy, and no crossing
  waits on it (`DECISIONS.md` §Product).
- **The model is never the runtime's.** In a harness session the host's own
  model answers; the runtime never calls one. In a batch run the model is
  reached through the gateway named by `COMMONMEASURE_INFERENCE_ENDPOINT`
  ([`docs/GETTING-STARTED.md`](../GETTING-STARTED.md)).

## 6. Verification state of each path

States are the ones [`docs/contracts/provider.md`](provider.md) §Verification states
defines and are never collapsed.

| Path | State | Evidence |
|---|---|---|
| Claude Code, observed (hooks) | `fixture-tested` | `crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the host's payload shapes |
| Claude Code, mediated (MCP over stdio) | `fixture-tested` | `crates/commonmeasure-cli/tests/mediated_e2e.rs` drives the server against a loopback origin; refusal, hashing, private-address floor and duplicate exclusion are asserted |
| Claude Code, reconstructed (import) | `fixture-tested` | `crates/commonmeasure-cli/tests/import_e2e.rs` |
| Claude Code, registration (`install`, `uninstall`, `doctor`) | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` drives the real binary against a home of the test's own; the session a real Claude Code session recorded after `install claude` is `demo/host-sessions/claude-code/`, read by `crates/commonmeasure-cli/tests/recorded_sessions.rs` |
| Codex, registration and mediated | `spec-verified` | the `[mcp_servers.commonmeasure]` table `install codex` writes matches Codex's configuration reference and is pinned by `crates/commonmeasure-cli/tests/install_e2e.rs`; no Codex session has run against it |
| Pi, registration and mediated | `fixture-tested` | `crates/commonmeasure-cli/tests/install_e2e.rs` writes, reads back and removes the extension; the session a real Pi session recorded through it is `demo/host-sessions/pi/`, read by `crates/commonmeasure-cli/tests/recorded_sessions.rs` |
| Screening-proxy socket | `planned` | no implementation |
| Crates linked directly | no state claimed | the crates are the binary's own dependencies; no external consumer is evidenced |

Live sessions on an operator's machine are the operator's private record and
are not committed here, so no path above claims `live-verified` from this
repository alone. `demo/arc/regenerate.sh` produces a complete session store
offline from the same binary, which is the committed demonstration of both
paths end to end ([`docs/RUN-THE-DEMONSTRATION.md`](../RUN-THE-DEMONSTRATION.md)).
