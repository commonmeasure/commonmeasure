---
title: "Getting started: from the installer to your own evidence"
domain: edge
audience: operator
section: get-started
---

# Getting started: from the installer to your own evidence

Record a session, apply a source policy and read an admitted crossing and a
refusal. No Hub account or provider key is needed for this walkthrough.
Your host supplies the model. Common Measure records and rules on the
content the agent acquires.[^checked] Terms such as crossing and admission
are defined in the [glossary](GLOSSARY.md).

## 1. Install the binary and register it with Claude Code

You get the binary and the host registration that records your next session.
If you came from the site's installation steps, go straight to the check.

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
commonmeasure install claude
commonmeasure doctor claude
```

The installer's release-download example was checked on 0.4.4 on macOS
(Apple silicon); it has not been rerun here. Registration and `doctor` were
rerun with the current build in a scratch home. Registration prints these
lines (absolute paths replaced with `<home>` and `<binary>`):

```text
claude-code: five hooks (SessionStart, PostToolUse, UserPromptSubmit, Stop, SessionEnd) registered in <home>/.claude/settings.json, each naming <binary>
claude-code: MCP server commonmeasure registered at user scope in <home>/.claude.json, naming the same binary
claude-code: a running session picks this up on its next start
```

Checkpoint: `doctor` names the registered hooks and MCP server. Act on any
`!` or `?` beside them before starting a new session. A missing console or
receiver does not stop local recording.

With a terminal, the installer asks once whether to report to sources whose
licence requires it; without that consent those sources are refused. Change
the answer at any time with `commonmeasure consent agree` or `withdraw`
([Reporting consent](INSTALL.md#reporting-consent)).

[Installing and updating](INSTALL.md) covers platforms, checksums and
pinning. [Connect an agent host](integrate/host.md#register-with-the-host)
covers other hosts and the alternative Claude Code plugin registration.
Use only one Claude Code registration route to avoid duplicate records.

### Updating

When upgrading from 0.4.5 or earlier, that release's `update` still refuses
while hosts run the binary. Run the installer once instead.
See [Updating](INSTALL.md#updating) for the commands and service behaviour.

## 2. Record a session of your own work

You get a source record of the content your agent acquired.
Open a new Claude Code session and ask it to fetch a public page with
`context_fetch`. Then read the record:

```sh
commonmeasure session
commonmeasure status
```

`session` shows the most recent session. To select another, pass its id.
A recorded one-page host example, checked on 0.4.0, includes these lines;
paths, identities and individual crossings are omitted:

```text
records    3
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

Your counts depend on the tools your agent used. Hooks record built-in tool
calls after they happen; only the mediated tools can refuse a crossing.
The session-start instruction asks the agent to prefer mediated tools, but
it cannot block the host's built-in tools. MCP crossings may appear under a
separate `local-*` session because the host does not pass its session id to
the MCP server. Both records appear in the console.

Checkpoint: find the fetched URL and its evidence grade. An observed
crossing establishes what the hook saw, not that policy admitted it.
[Session records](OPERATING.md#record-a-session-of-your-own-work) explains
hashes, context snapshots and the separate host and MCP records.

## 3. Credentials, when you want provider search

You can see which suppliers are configured without making a paid call.
Skip this step if you only need page fetches.

```sh
commonmeasure credentials
```

With no keys, the report includes these lines (other providers and the
credentials-file guidance are omitted):

```text
· exa          unavailable (EXA_API_KEY is not set)
· tavily       unavailable (TAVILY_API_KEY is not set)
```

The report names each provider, its required variable and whether it is
configured. A missing credential means search is unavailable, not that the
search found no results. `context_fetch` needs no provider key.

Checkpoint: before asking for provider search, confirm that provider is
configured. Put keys in `$COMMONMEASURE_HOME/credentials.env` (by default
`~/.commonmeasure/credentials.env`), restrict the file to its owner and
restart the host session so the MCP server loads it, or add a key from
the console's Sources page (§5). Supplier calls may cost money. [Credentials](OPERATING.md#credentials-when-you-want-provider-search)
has the file format, environment precedence and the local corpus option.

## 4. Policy: refusing a crossing before it happens

You get a refusal before the edge requests a page your policy excludes.
Use a scratch home for this example so it does not replace your own policy:

```sh
export COMMONMEASURE_HOME=$(mktemp -d)
cat > "$COMMONMEASURE_HOME/policy.json" <<'POLICY'
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*.people.com", "action": "allow"},
    {"kind": "access_rule", "host": "*.theguardian.com", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ]
}
POLICY
commonmeasure policy check "$COMMONMEASURE_HOME/policy.json"
```

The check prints these fields; the path and digest are omitted:

```text
mode          strict
constraints   4
scopes        0
principals    0
terms         0
```

A checkout holds the same policy in `demo/policy/four-fetches.json`.
The first matching access rule applies. The final rule refuses every other
host. A policy that cannot be parsed is an error, not a permissive fallback.
No policy selects observe mode; source declarations and private-address
restrictions still have their own rules in the
[source policy contract](contracts/source-policy.md).

Send two fetches through the real MCP transport without needing a host:

```sh
{
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"walk","version":"1.0"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"context_fetch","arguments":{"url":"https://www.gov.uk/government/organisations"}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"context_fetch","arguments":{"url":"https://www.economist.com/"}}}'
} | commonmeasure mcp --host claude-code --session door-walkthrough
```

The admitted result includes the following fields. The JSON-RPC wrapper,
page text, hashes, token estimate, declarations and record metadata are
omitted. The page and its declarations may change between runs.

```json
{
  "url": "https://www.gov.uk/government/organisations",
  "http_status": 200,
  "policy": "Admitted; no constraint excluded it."
}
```

The refused result is a tool error. With the wrapper removed and the scratch
home replaced by `<home>`, its reason is:

```text
refused before the crossing: access rule 4 (*) refuses host www.economist.com. (operator policy in <home>/policy.json)
```

Read both crossings back:

```sh
commonmeasure session door-walkthrough
```

These count lines omit the path, host, policy identity and crossing detail:

```text
crossings  0 observed, 1 mediated, 1 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

Checkpoint: the refused crossing has a reason and no content hash. The
admitted crossing carries the hash of the text returned to the agent.
[Four fetches, two refused](guide/four-fetches.md) holds the full four-fetch
example, including a publisher's payment terms. For directory rules,
principals and spend limits, see
[Scopes, engagements, principals and allowances](OPERATING.md#scopes-engagements-principals-and-allowances).

## 5. The console

You can read the same source record in the browser.
Keep the scratch home selected and run:

```sh
commonmeasure serve --listen 127.0.0.1:18473
```

In another terminal with the same `COMMONMEASURE_HOME`, run:

```sh
commonmeasure console --listen 127.0.0.1:18473
```

The status line is below; the process id is replaced with `<pid>` and the
page list is omitted:

```text
✓ console: http://127.0.0.1:18473 answers, Common Measure 0.4.6, pid <pid>
```

Open the reported loopback address, `http://127.0.0.1:18473`. This example
uses a separate port so it can run beside your usual console.
Under **Record**, select `door-walkthrough`; its refused card names the
policy rule. A new crossing appears on reload. The **Policy** page shows
the current source policy and lets you edit a personal policy.

Checkpoint: the console and command-line record show the same admitted and
refused crossings. [The console](CONSOLE.md) describes its pages and address
options. Stop `serve` with Ctrl-C when finished. In the walkthrough shell,
return to your usual home with:

```sh
unset COMMONMEASURE_HOME
```

If you had set a custom home before the example, restore that value instead.
The scratch directory keeps the example's records until you remove it.

### Keep the console running (macOS)

Use the [login service](INSTALL.md#keep-the-console-running-macos) when you
want the console to start at login. Its reference covers installation,
status, logs and restarts after an update.

## Later, when you need them

## 6. Import history from before Common Measure

[Import history](OPERATING.md#import-history-from-before-common-measure)
starts with a dry run. Imported crossings are reconstructed from host
transcripts, kept separate from witnessed evidence and never relayed.

## 7. Egress, only when you ask for it

[Reporting](OPERATING.md#egress-only-when-you-ask-for-it) explains the
receiver, clearance, dry run and retry queue. There is no default receiver.
Read it before enabling reporting: a first relay considers earlier records
too, under the policy in force when it runs.

### Relay without a session end

Hosts without a session-end event need a
[background relay](INSTALL.md#relay-without-a-session-end) for sources whose
licences require automatic usage reporting. A configured receiver and your
[reporting consent](INSTALL.md#reporting-consent) are still required.

### Joining Common Measure Hub

Follow [Start here](https://commonmeasure.ai/docs/hub/start-here/) for a new
organisation, or [Connect a Common Measure edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/)
for an existing one. [Edge enrolment](OPERATING.md#joining-common-measure-hub)
explains the local keys, managed policy and disconnection.

## 8. If you build from source

[Building and replay](INSTALL.md#if-you-build-from-source) covers the Rust
toolchain, installation from a checkout and batch runs. A host uses its own
model; batch inference needs an explicitly configured gateway. A missing
provider or gateway produces an unavailable result naming the gap.

## 9. Enrol a project directory

[Directory enrolment](OPERATING.md#enrol-a-project-directory) lets you choose
local recording or reporting for a project. Reporting includes eligible
history only with acknowledgement; a managed edge also needs the owner's
approval. Other worktrees are separate directories.

[^checked]: Policy, MCP fetches, record reading, credentials, registration,
    doctor and console commands rerun on 29 September 2026 with the build
    from `17b0daba`, in scratch homes. The host-session excerpt was checked
    on 0.4.0; release installation on 0.4.4. No interactive host session,
    paid search, update or login-service installation was rerun.
