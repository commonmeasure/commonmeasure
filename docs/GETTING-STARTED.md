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

With a Common Measure Hub organisation, one line installs the binary,
registers it with every agent host on the machine, connects the machine to
the hub and relays its first evidence. Put your hub's address in the line
below and run it in the project directory whose sessions the hub should
see:

```sh
cd ~/code/your-project
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh -s -- --connect HUB
```

The connect step prints a code and the hub page to approve it on, and opens
the page where the desktop has an opener. Approve the code there, signed in,
and the run goes on. To enrol someone else's machine, an owner mints a token
on the hub's Enrol this machine card and adds `--token TOKEN` to the line.

After the install and the [reporting consent](INSTALL.md#reporting-consent)
question, it prints one line per step:

- `hosts:` the hosts found and registered, each as `commonmeasure install
  <host>` registers it, and those not found, for which nothing is written.
- `connect:` first the page and the code to approve, unless the line has a
  token; then the hub, the organisation and the policy revision in force, or
  `waiting for revision` while the organisation has published none. A code
  refused in the hub, one that expires and an unreachable hub stop the run
  here; run the line again for a new code.
- `enrol:` hub reporting for the directory. The line does not confirm the
  directory's existing history for you; it prints the `commonmeasure enrol
  … --include-history` command that does. Run from your home directory or
  `/`, nothing is enrolled. A directory enrolled earlier is reported as it
  stands, such as `awaiting approval` until an owner approves it on the
  hub's Project reporting page.
- `service:` the background relay, which delivers reports for the first
  run's own session and for every host that sends no session-end event
  (Codex, Pi, Claude Desktop, Cursor, Copilot CLI, VS Code). Where no relay
  or hosted service holds the edge home, macOS gets the background relay
  service, installed as `commonmeasure service install relay` installs it,
  and the step waits until it runs. A relay service that already serves
  another home is left in place, and the line names the command that moves
  it. On Linux the line names the command to run under your service
  manager, `commonmeasure relay --every 300`. If the service cannot be
  installed, the line gives the reason and the run goes on.
- `fetch:` one governed fetch of `https://commonmeasure.ai/`, in a session
  of the installer's own: the source record names its host as
  `commonmeasure-first-run`, never one of your agent hosts. The page's
  licence demands reporting, so without reporting consent it is refused and
  the line names `commonmeasure consent agree`; it is also refused unless a
  background relay or hosted service holds the edge home, which on Linux
  means one you started before the line. A refusal is reported once and not
  retried.
- `relay:` what was delivered to the hub.
- `evidence:` the hub's Fleet evidence address, then `console:` the local
  console's.

A step that fails stops the run, names the remedy and exits 3; the binary
stays installed. The token goes to the hub and is printed or stored nowhere,
including in a hub refusal. A successful hub response that echoes it is
refused before being accepted or saved.
`--token` needs `--connect`; empty or whitespace-only values are refused
before any download, and neither combines with `--update`.

Without a hub, or to do it step by step: steps 01 and 02 of [Install Common
Measure](https://commonmeasure.ai/install/), install Edge and connect your
agent.

## 2. Record a session of your own work

Step 03 of [Install Common Measure](https://commonmeasure.ai/install/): make a read and see the record.

## 3. Credentials, when you want provider search

Step 05 of [Install Common Measure](https://commonmeasure.ai/install/): add a provider key.

## 4. Walkthrough without an agent host

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

To read the same record in the browser, run `commonmeasure serve --listen
127.0.0.1:18473` with the scratch home still selected and open that address
([The console](CONSOLE.md)). When finished, return to your usual home:

```sh
unset COMMONMEASURE_HOME
```

If you had set a custom home before the example, restore that value instead.
The scratch directory keeps the example's records until you remove it.

## 5. The console

Step 06 of [Install Common Measure](https://commonmeasure.ai/install/): open the console.

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

Codex and other hosts without a session-end event need a
[background relay](INSTALL.md#relay-without-a-session-end) for sources whose
licences require automatic usage reporting. A configured receiver and your
[reporting consent](INSTALL.md#reporting-consent) are still required.

After configuring a receiver, run on macOS:

```sh
commonmeasure service install relay
```

On other platforms, run `commonmeasure relay --every 300` under your service
manager. `connect --managed` offers to install the relay on macOS when it is
missing and no hosted service holds the home. With non-terminal input or
output, any non-empty `CI` value, or `--no-relay-offer`, it prints the command
without prompting. Moving another home's relay requires explicit agreement;
that home loses its background reporting. An installation failure leaves
enrolment successful and prints the retry command. Check `commonmeasure status`
or `commonmeasure doctor codex` for whether the relay is running. Without an
automatic relay, Codex refuses sources whose licences demand usage reporting.

### Joining Common Measure Hub

During the pilot, Common Measure Hub is open by invitation: ask your
organisation's owner for one, or join the [waiting list](https://commonmeasure.ai/waitlist/?source=edge)
for a new organisation. Follow [Start here](https://commonmeasure.ai/docs/hub/start-here/) for a new
organisation, or [Connect a Common Measure edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/)
for an existing one. To connect a machine that is already installed, run
this on it and approve the code it prints in the hub, signed in:

```sh
commonmeasure connect https://hub.example --managed
```

Nothing is written before the approval. A refused or expired code, or an
unreachable hub, ends the command with the reason and the command to run
again. To enrol another person's machine, an owner mints a token on the
hub's Enrol this machine card and that machine runs `commonmeasure connect
https://hub.example --managed --token TOKEN`.
[Edge enrolment](OPERATING.md#joining-common-measure-hub) explains the local
keys, managed policy and disconnection.

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

[^checked]: Policy, MCP fetches, record reading and doctor commands rerun on
    29 September 2026 with the build from `17b0daba`, in scratch homes. No
    interactive host session, paid search or update was rerun.
