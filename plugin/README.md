# The Common Measure plugin

The Claude Code plugin is one of two ways to register Common Measure with a
host so that it records the content the agent takes in and, through the
mediated tools, applies the operator's policy before the content moves. The
other way, and the one to use when the `commonmeasure` binary is on the
machine, is the binary's own registration:

```sh
commonmeasure install claude     # hooks and the MCP server, by absolute path
commonmeasure doctor             # what each host has registered, checked
commonmeasure uninstall claude   # exactly those entries removed
```

`docs/contracts/host-integration.md` §1 states what each command writes.
The plugin route below exists for installs through a Claude Code
marketplace or from the standalone archive. Use one route or the other, not
both: each carries the same four hooks, and two registrations record every
crossing twice. `install claude` refuses while the plugin is enabled.

What any host must supply to integrate is
`docs/contracts/host-integration.md`. Terms like crossing, engagement,
scope and policy mode are defined in `docs/GLOSSARY.md`.

**Not a processor** (`docs/contracts/processor.md`). A processor is a
replaceable stage around a ContextJob; a harness plugin integrates the product
with a host.

## What the plugin holds

The plugin is a registration, not a distribution: a manifest
(`.claude-plugin/plugin.json`), the hook declarations (`hooks/hooks.json`),
the MCP server entry (`.mcp.json`) and one launcher
(`bin/commonmeasure-launch`). It carries no binary. The launcher finds the
binary on the machine, in this order: a prebuilt binary beside it, which only
the standalone archive bundles; `commonmeasure` on `PATH`; then
`~/.local/bin/commonmeasure`, where `install.sh` places it, because a host's
hook environment does not always share the login shell's `PATH`. With no
binary found, a hook exits zero after a note on stderr and the agent
continues; the MCP entry exits non-zero, because a mediator that is silently
absent would leave the agent believing its crossings were governed.

Replacing the binary on the machine changes what the next session runs, with
no reinstall of the plugin.

## Install

From a release, with no checkout, `install.sh` places the binary and can
unpack the standalone archive beside it (`docs/RELEASE.md` §Installing from a
release). From this checkout, with the binary already installed
(`docs/GETTING-STARTED.md` §8):

```sh
claude plugin marketplace add "$PWD"   # from the repository root
claude plugin install commonmeasure@commonmeasure
```

The plugin declares its own hooks and its own MCP server, so nothing edits
your global settings, and removing the plugin removes all of it. Check what
Claude Code holds and what the launcher resolves to:

```sh
claude plugin details commonmeasure
commonmeasure doctor claude
```

`commonmeasure` in these commands is the binary on your `PATH`. `install.sh`
puts it in `~/.local/bin` and prints the line to add when that directory is
not on `PATH`; it edits no shell profile. Where the binary is not on `PATH` —
a marketplace or archive install with nothing else on the machine — run the
plugin's own launcher in its place, at
`~/.claude/plugins/cache/commonmeasure/commonmeasure/<version>/bin/commonmeasure-launch`,
which takes the same subcommands; that is the path the archive's `INSTALL.md`
gives.

`doctor` names the plugin, whether it is enabled, and any direct registration
beside it.

For `context_search`, put provider keys in `~/.commonmeasure/credentials.env`
(`docs/GETTING-STARTED.md` §3; the rules are in `DECISIONS.md` §Integration
and ownership). Check what is configured with:

```sh
commonmeasure credentials
```

`context_fetch` needs no credentials, so the mediated path works before any
of this.

## The standalone archive

To hand the plugin to someone with no binary, no repository and no toolchain,
package it with the release binaries inside:

```sh
sh plugin/build.sh                 # build the binaries the archive bundles
sh plugin/package.sh               # -> dist/commonmeasure-plugin-<version>.tar.gz
```

The archive is its own marketplace: the recipient unpacks it and runs
`claude plugin marketplace add ./` then `claude plugin install
commonmeasure@commonmeasure` from inside it. Its `INSTALL.md` gives these
instructions, states the privacy position, and lists the binaries the archive
bundles; the list is interpolated at packaging time, not a fixed example. The
launcher prefers a bundled binary beside it, so the archive works on a
machine with no other binary. Every release publishes the same archive,
packaged from the release's own binaries for every supported platform
(`docs/RELEASE.md`). The binaries are build outputs and are gitignored; the
checkout's plugin directory never holds one.

## Disabling and uninstalling

- **Pause it** without losing the installation:
  `claude plugin disable commonmeasure@commonmeasure`. Hooks stop firing and the
  mediated tools disappear from the session; `enable` restores both.
- **Remove it**: `claude plugin uninstall commonmeasure@commonmeasure` (and
  `claude plugin marketplace remove commonmeasure` to forget the marketplace).
  The plugin declares its hooks and MCP server in its own manifest, so
  uninstalling removes every surface it added; it never writes to your global
  settings.
- **Uninstalling keeps the record.** `~/.commonmeasure/` (sessions, policy,
  relay spool) is the operator's evidence, not the plugin's cache, and
  uninstalling does not delete it. Delete the directory to delete the
  record. `commonmeasure disconnect` revokes an enrolled edge's keys at the
  hub and removes `relay.json`, `edge-key.json` and `enrolment.json`, so the
  edge keeps recording locally and sends nothing; an edge configured by hand
  deletes `relay.json` alone.
- A session that is already running keeps its loaded copy until
  `/reload-plugins` or a restart, in both directions — after an install and
  after an uninstall.
- **A plugin whose marketplace directory has moved loads nothing.** Claude
  Code reports the plugin as failed to load and no hook fires, so sessions
  record nothing and nothing says so in the session. `commonmeasure doctor
  claude` reports the plugin's installed path beside any direct registration;
  the direct registration names the binary by absolute path and does not
  depend on a marketplace directory.

## Capture modes

Both modes record; only the mediated mode can refuse. Every record carries
which mode produced it, as an explicit field, not as something inferred from
the code path that wrote it.

### Observed — `hooks/hooks.json`

`PostToolUse` on `WebFetch|WebSearch|mcp__.*`, plus `UserPromptSubmit` and
`Stop`. Needs no credentials and no configuration, so it starts recording as
soon as you work. (The same file declares a fourth hook, `SessionStart`, which
is delivery rather than capture — §The standing nudge.) `commonmeasure
install claude` writes the same four hooks into the user settings file.

`PostToolUse` fires *after* the crossing, so a hook can record what the agent
read but cannot refuse it. What it records:

- `WebFetch` — retrieved **and** grounded, with the SHA-256 of the text that
  entered the model's context. The hash is of the text, not of the transport
  wrapper: Claude Code delivers `{"bytes": …, "code": …, "result": "<text>"}`
  while the context carries only `result`, so a hash of the wrapper cannot be
  matched against the transcript.
- `WebSearch` and MCP results — retrieved only. The model saw titles and
  snippets, not pages, and nothing ties a returned URL to bytes that entered
  context. Claiming grounding here would put a citation in the record with
  nothing behind it.
- Nothing from this plugin's own MCP tools, under either spelling: Claude
  Code namespaces the plugin-installed server as
  `mcp__plugin_commonmeasure_commonmeasure__*`, and a server registered
  directly (`install claude`, or `.mcp.json`) appears as
  `mcp__commonmeasure__*`. The hook matcher includes `mcp__.*`, which catches
  both, so a mediated fetch would otherwise appear twice — once correctly as
  mediated, and once as an observed crossing that scraped every URL out of
  the page body it had just returned.
- Subagent identity, when the host supplies it. `agent_type` and `agent_id` are
  Claude Code's own discriminators; main-agent calls carry neither, and neither
  is inferred from payload shape.
- The host's turn identifier (`prompt_id`), carried as `turn_id` on the
  crossing and on each turn boundary, so crossings group under the turn that
  caused them without the prompt entering the record
  (`docs/contracts/session-evidence.md` §Turn boundaries).

The hook fires for `WebFetch` and `WebSearch` and the matcher covers both;
every successful call of either in a session whose hooks ran is recorded. A
call the host reports as failed fires `PostToolUseFailure`, which is not
registered, so a fetch that failed leaves no observed record.

`UserPromptSubmit` records, beside the turn boundary, which URLs the prompt
named (as hashes, never text) and any statement pasted material carries: an
inline RSL licence, a `Content-Usage:` line, or a C2PA manifest embedded in
pasted text. A mediated fetch of a URL the prompt named records `named_by:
user`; one no prompt named records `agent`. The record's shape is
`docs/contracts/session-evidence.md` §Prompt sources.

Capture is best-effort by contract: the hook exits zero whatever happens, and an
unreadable payload produces no records. Capture failures must not interrupt the
agent.

### Mediated — `.mcp.json`

`context_fetch`, `context_search` and `context_status`, served over stdio. These
run *before* the crossing, so policy can refuse.

- `context_fetch` needs no credentials. It fetches over the real transport,
  checks the operator's source policy first, records the crossing either way,
  and hands the agent the bytes plus the hash it recorded.
- `context_search` needs a provider credential, loaded at start from
  `$COMMONMEASURE_HOME/credentials.env` under the rules in `DECISIONS.md`
  §Integration and ownership. Without a credential it reports `unavailable`,
  naming the missing variable and the file to create; it does not return an
  empty search result. Provider `internal` is the exception: it queries
  the operator's own corpus — the bounded directory named by
  `COMMONMEASURE_INTERNAL_CORPUS`, set in the session's environment (`.mcp.json`
  carries no `env` block; the server inherits the harness's) or in the same
  credentials file — with a
  deterministic query, no network and no charge, and its results carry the
  licence and effective date the corpus's own `corpus.json` declares. For
  the crossings to be *recorded*, the corpus's `file://` prefix must also be
  named in `record_internal_prefixes` (see §Policy) — without that consent
  the agent still gets its result and the record is withheld.
- `context_status` reports the session, where its evidence is written, the
  policy in force and which providers are configured.

The host starts the MCP server without a session identifier, so mediated
crossings are recorded under the server's own `local-*` session id, in a
separate log from the hooks' records for the same conversation
(`docs/contracts/host-integration.md` §2).

## The standing nudge

Agents default to their built-in tools unless asked otherwise. The refusable
grade of evidence arises only when the agent uses the mediated tools, so the
`SessionStart` hook adds the request to every session.

A `SessionStart` hook (`hooks/hooks.json`) prints one paragraph on stdout,
which the host adds to the session's context on every start: new sessions,
resumes, and the rebuilds after `/clear` and compaction. It asks the agent to
prefer `context_fetch` and `context_search` over `WebFetch` and `WebSearch`,
to treat an unavailable mediated tool and a policy refusal as different
answers, and to respect a refusal rather than retrying it with a built-in
tool. The wording is versioned (`mediation-nudge/3`,
`crates/commonmeasure-harness/src/nudge.rs`), and each emission is recorded in the
session log as `nudge_issued` (`docs/contracts/session-evidence.md`), so a
later review can distinguish sessions that were asked from sessions that were
not.

Its limits, stated because the record depends on them:

- **A nudge, not enforcement.** Nothing blocks the built-in tools, the agent
  is free to ignore the request, and observed capture keeps recording
  built-in use either way. Only the mediated path can refuse, and only when
  the agent calls it.
- **Emission is what is witnessed.** The host adds `SessionStart` stdout to
  the context; that act is the host's, and the `nudge_issued` record's basis
  says so rather than claiming injection.
- **Hosts without the hooks never see it.** Codex and Pi are mediated-only
  (§Codex and Pi), so their sessions carry the tools but not the nudge.
- **Best-effort like all capture.** The hook exits zero whatever happens; an
  unwritable record store loses the issuance record, not the nudge, and a
  missing binary means no nudge and no interruption (the launcher's rule).

It is installed and removed with the registration, plugin or direct:
`disable` stops it, `uninstall` removes it, and nothing is written to any
`CLAUDE.md`.

## Codex and Pi

Codex gets the mediated half only, registered by the binary:

```sh
commonmeasure install codex      # one [mcp_servers.commonmeasure] table in ~/.codex/config.toml
commonmeasure uninstall codex
```

The table names the binary, the arguments `mcp --host codex`, and
`default_tools_approval_mode = "approve"`. Codex asks before every MCP tool
call unless the table says otherwise, and its non-interactive runs
(`codex exec`) refuse a call that would ask, so without that line the
mediated tools are unusable unattended and cost a click per fetch
interactively. `doctor codex` reports whether the line is there. One table
serves three programs: the Codex CLI, the ChatGPT desktop app (which
carries Codex) and the Codex IDE extension all read `~/.codex/config.toml`
and start the server from it.

Every Codex session then carries `context_fetch`, `context_search` and
`context_status` under the same operator policy as Claude Code, with
crossings recorded as `host: codex` under the server's own `local-*` session
id, each carrying the client's own name and version (`codex-mcp-client`,
with the version telling the CLI from the desktop app;
`docs/contracts/session-evidence.md` §Client identity). Codex has lifecycle
hooks, and Common Measure registers none: Codex's hooks documentation
states that hosted tools such as its web search do not use the local tool
path and fire no hook, and its shell reaches the web as command text with
no response a hook could attribute to a URL, so an observed matcher there
could witness only third-party MCP results, which are retrieved-not-grounded
rows. Codex sessions therefore have no observed floor, no turn boundaries
and no nudge, and the record claims nothing about any of them
(`DECISIONS.md` §Integration and ownership).

Pi gets the mediated half through an extension, because Pi has no MCP
client of its own:

```sh
commonmeasure install pi         # ~/.pi/agent/extensions/commonmeasure/index.ts
commonmeasure uninstall pi
```

The extension names the binary by absolute path. At each session start it
spawns `commonmeasure mcp --host pi --session <Pi's session id>` in the
session's working directory, lists the server's tools over stdio and
registers each with Pi under its own name, so the agent calls
`context_fetch`, `context_search` and `context_status` as on any other host
and every call is carried by the server under operator policy. Crossings
are recorded as `host: pi` under Pi's own session id. A server that cannot
start leaves the session without the tools and says so once in Pi's
notifications; nothing answers in its place. Pi has no web tool of its own,
so there is nothing to observe: no observed floor, no turn boundaries, no
nudge.

## Claude Desktop and Cursor

Claude Desktop gets the mediated half only, in the application's own file:

```sh
commonmeasure install claude-desktop   # mcpServers.commonmeasure in claude_desktop_config.json
commonmeasure uninstall claude-desktop
```

Claude Desktop loads the entry at its next start and has no hook surface,
so nothing is observed. It starts one server for its chat client and one
for its local agent mode; each server that makes a call leaves its own
session, recorded as `host: claude-desktop` with the client's own name
(`claude-ai` or `local-agent-mode-commonmeasure`), and a server asked for
nothing leaves no file; `commonmeasure session` shows which. The
server's working directory is `/`, so no directory-keyed policy scope
applies to a Claude Desktop session.

Cursor gets both halves, in its two global files:

```sh
commonmeasure install cursor    # ~/.cursor/mcp.json and ~/.cursor/hooks.json
commonmeasure uninstall cursor
```

The server is `mcp --host cursor`; the hooks are Cursor's `sessionStart`,
`postToolUse`, `beforeSubmitPrompt` and `stop`, each running `hook <event>
--host cursor`, the same four moments the Claude Code registration hooks.
Cursor's `postToolUse` carries every tool's output, so a third-party MCP
result is an observed crossing under Cursor's `conversation_id`; our own
tools are not observed a second time; the nudge reaches the session as the
JSON field Cursor reads at session start. Cursor can load
`~/.claude/settings.json` hooks too (its settings' "include third-party
plugins" switch, off by default), and its documentation does not say which
payload shape those hooks receive, which is why every hook command
`install claude` writes says `--host claude-code` and the reader refuses a
payload that is not Claude Code's shape, or any payload when Cursor's
`CURSOR_PROJECT_DIR` is in the environment, recording and printing
nothing. `uninstall` for either host deletes no file and leaves the
`mcpServers` and `hooks` objects in place, empty if ours was the only
entry.

## Policy

`$COMMONMEASURE_HOME/policy.json`, or `~/.commonmeasure/policy.json`. If the file
is absent the mode is observe: record everything, refuse nothing.

```json
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "allowed_source_host", "host": "www.gov.uk"},
    {"kind": "denied_source_host", "host": "tracker.example"}
  ],
  "allow_private_hosts": false,
  "principals": [
    {
      "principal": "research-agent",
      "os_user": 1001,
      "require_scope": true,
      "policy_mode": "strict"
    }
  ],
  "scopes": [
    {
      "match": "/work/personal",
      "principal": "research-agent",
      "engagement": "personal",
      "allow_telemetry_egress": true
    }
  ]
}
```

`principals` binds a policy name to the process's effective numeric operating-
system user. Common Measure reads that credential from the process boundary;
`USER`, `LOGNAME` and `COMMONMEASURE_PRINCIPAL` cannot select authority. The last
may be shown as an asserted label for diagnosis only. A principal with
`require_scope: true` can use only a directory scope that explicitly names
that principal; an unmatched directory fails closed instead of inheriting the
top-level policy. The converse holds for every principal, `require_scope` or
not: a scope naming a principal is that principal's authority, so anybody else
whose working directory falls inside it is refused there rather than passed on
to the next scope whose match string also appears in the directory. That next
scope is usually the broader one, and a broader scope is usually the looser
one, so passing on would relax a policy the operator did not declare relaxed.
A scope naming nobody governs whoever works in it. Principal mode,
constraints and private-host settings overlay the top
level first, then the matched directory scope overlays them.

Declaring any principal changes what happens to everyone else. An OS user with
no binding holds no delegated authority, so every mediated crossing under it is
refused and `context_status` names the reason — including where the top-level
`policy_mode` is `observe`, because a mode decides what happens to a breach and
not whether an unauthorised identity may act. Bind every OS user that
should be able to work, service and CI accounts included.

Contradictory bindings are refused at load, before the server starts: two
principals sharing a name or an OS user, a scope naming a principal that is not
declared, and a principal with `require_scope: true` that no scope names, which
could never be admitted anywhere.

That credential is unix-only. The Windows binary has no effective uid
to read and Common Measure does not read a process token yet, so it authenticates
nobody: a policy file declaring `principals` refuses every mediated crossing
there and `context_status` names the reason, instead of falling through to
the top level. A policy file declaring no principal is unaffected —
directory scopes and everything else on this page work as described on every
platform.

`policy_mode` decides what happens to a breach, never whether it is recorded.
`strict` refuses; `observe` and `prefer` carry the crossing with the breach on
the record. The constraint vocabulary is `commonmeasure_types::Constraint` and it is
enforced by the same `commonmeasure_runtime::policy` functions the batch runner uses —
there is no second policy engine.

Every constraint kind but one is a set, and its order means nothing. The
exception is `access_rule`, the ordered form of host policy:

```json
{
  "constraints": [
    {"kind": "access_rule", "host": "docs.example.com", "action": "allow"},
    {"kind": "access_rule", "host": "*.example.com", "action": "refuse"},
    {"kind": "access_rule", "host": "publisher.example", "action": "require_licence", "licence": "rsl:publisher/2026"},
    {"kind": "access_rule", "host": "*", "action": "require_mediation"}
  ]
}
```

Access rules are read in the order written and the first whose `host`
matches the source's host decides; a later rule for the same host is never
reached, so the exception above stands and the same exception written after
the wildcard would not. `host` is an exact host, `*.example.com` for a
domain and every host beneath it, or `*` for every host; a source with no
host, such as a document from the operator's own corpus, matches no rule.
The actions: `allow` passes the host, including past an `allowed_source_host`
list that does not name it; `refuse` is a breach; `require_licence` admits
the source only when the supplier declared exactly the named licence, an
unknown licence being unknown rather than permitted; `require_mediation`
states that a host is to be read only where policy rules before the content
moves, which every mediated fetch and every batch-run source does, so it
passes those like `allow`, while an observed crossing, which nothing ruled
on, is recorded without being checked against it. Set it where the policy
file should state that intent; where a host must not be read at all, use
`refuse`. The order of checks at admission is fixed:
the `denied_source_host` set first, because a denial is absolute and no rule
allows past it, then the access rules, then the `allowed_source_host` set,
then `required_licence`. The rules in a scope's `constraints` govern that
scope alone, like every other constraint there. A rule whose host pattern
names no host, or whose `require_licence` names no licence, is refused at
load.

`allow_private_hosts` lets the mediated tools reach loopback and private
addresses, for an operator running a local documentation server. It is off by
default and it does **not** affect observed capture, which holds a floor:
passive capture sees everything and never asks, so localhost, private networks
and `file://` stay out of the record regardless of this setting.

The PII detector scans the text of every mediated crossing and records what
it found, as categories and offsets, never the identifiers themselves. What
`strict` does with a finding depends on where the text came from. On an
internal or private source (a named internal prefix, a loopback or private
address reached under `allow_private_hosts`, the operator's own corpus)
`strict` refuses the crossing, as it does every breach. On a public source
the crossing is admitted with the finding recorded in `breach`: a public
page's published contact addresses are not the personal data the detector
exists to keep out of a model, and a firm whose first admitted source is a
government page should not find it refused for the department's email
address. `refuse_on_pii: true`, at the top level or on a scope, restores
the refusal on every source for an operator that wants it; it is off by
default. `observe` and `prefer` carry the finding on every source, as they
carry every breach.

The floor is a default, and there is one way to lower it:
`record_internal_prefixes` names internal prefixes
(`"https://rag.example.internal/"`, `"file:///corp/kb/"`, each required to end
with `/`) whose crossings are recorded like public traffic, for observed and
mediated capture alike. Nothing the list does not match is affected, an absent
or empty list means the floor holds everywhere, and matched crossings are
marked `internal` in the record so egress never projects them.
`docs/contracts/session-evidence.md` is the authority on what that means.

`terms` names agreements the operator holds with sources, by host:

```json
{
  "terms": [
    {"host": "publisher.example", "reference": "agreement-42", "requires_reporting": true,
     "access_context": [{"scheme": "ror", "value": "https://ror.org/013meh722"}]}
  ]
}
```

A terms entry is a statement of fact about an agreement, referenced by an
identifier the operator chooses; the runtime never checks it. Where one names
the host of a mediated fetch, it governs over any preference the source
publishes: the source's statements are still read and recorded, none is
enforced, and the crossing's licence is the reference. `requires_reporting`
says the agreement itself requires usage reporting; `access_context` names the
institution identifiers the agreement attributes usage to, which are the
operator's to declare and never name a person. A scope may carry its own
`terms`, replacing the top-level list as `constraints` does. What the record
carries is `docs/contracts/session-evidence.md` §Source declarations.

Telemetry is a second, independent permission. A scope binds its matching cwd
to `engagement`; `allow_telemetry_egress` must be explicitly true before a
witnessed public crossing in that engagement can enter the configured relay.
An absent policy, unmatched cwd, unnamed scope, or omitted flag stays local.
Client engagements therefore default to no egress rather than inheriting a
weaker machine-wide mode.

That `engagement` is the governing engagement, the only engagement name the
runtime and the relay can read (`context_status` reports it as
`governing_engagement`); the name the console reports work under is resolved
separately from `attribution.json` (`DECISIONS.md` §Session policy and
egress).

A policy file that cannot be parsed is an error, not a silent fallback to
permissive. That includes a key the installed binary does not know: an
unknown key fails the whole file (`policy.json is not a valid policy: unknown
field "engagement"`), so a policy written for a newer binary refuses every
mediated crossing on an older one until that binary is reinstalled.

On a managed edge, one whose `deployment.json` pins a hub's signing key
(`docs/contracts/policy-envelope.md`), the file is the hub's desired policy
and the edge keeps it current itself: it fetches the signed envelope once
at each session's start, through the `SessionStart` hook where the host has
one and otherwise at the MCP server's start, waiting at most three seconds,
and again before each `commonmeasure relay`; `commonmeasure policy sync`
does the same on demand. An
envelope that has expired keeps enforcing the policy it carried; the
session record, `doctor` and `status` say `stale since` its expiry until
the hub renews it or publishes a later revision. Nothing about a refresh is
said to the agent, and a hub that cannot be reached changes nothing.

## What this does not contain

No binary, no provider credentials and no routing logic beyond the operator's
declared policy. Credentials come from the environment and the operator's own
`~/.commonmeasure/credentials.env` (`DECISIONS.md` §Integration and ownership)
and travel only in request headers.
