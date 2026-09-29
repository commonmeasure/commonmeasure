---
title: Sessions, policy and reporting
domain: edge
audience: operator
section: use
---

# Sessions, policy and reporting

Reference for [Getting started](GETTING-STARTED.md). The command examples
and quoted session, credentials and relay output were checked on 0.4.0,
and have not been rerun for this move. Local directory enrolment was rerun on 0.4.6 (29 September 2026).
Hub reporting and approval commands were not rerun; the original page
did not record a checked release for them. Replace example paths, identities and keys
with your own. Provider calls may cost money.

## Record a session of your own work

Open a new Claude Code session and work. No other step is needed: the
observed half needs no credentials and no configuration. A `PostToolUse` hook
fires after every `WebFetch`, `WebSearch` and third-party MCP result, and
records the crossing; for `WebFetch`, with the SHA-256 of exactly the text
that entered the model's context, which makes the row checkable against the
transcript later. Search results are recorded as retrieved but not grounded:
the model saw titles and snippets, not pages, and the record does not claim
more.

Read it back at any time:

```sh
commonmeasure session            # the most recent session
commonmeasure session <id>       # a named one
commonmeasure status             # the policy in force, by digest, and this edge's fleet status
```

A session that fetched one page prints, among other lines:

```text
records    3
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

Each completed turn also records a context snapshot: the host transcript's
own usage counters for the turn's last model call, printed with that basis
named, and an inventory of the window by category estimated from the host's
own transcript records, printed with its basis named. What no basis can see
(a context limit, the free capacity, the resident tool schemas) is said to
be unavailable rather than estimated; the witnessed acquisition is shown
beside the counters as a lower bound; and between two boundaries the report
shows the API-reported change beside the estimated growth of scaffolding,
conversation and acquired content and the remainder it cannot attribute
([`docs/contracts/session-evidence.md`](contracts/session-evidence.md) §Context snapshots).

Observed capture can only watch: the hook fires after the crossing, so
nothing can be refused. The refusable grade of evidence exists only when the
agent asks Common Measure to carry the crossing, and agents use their built-in
tools unless told otherwise. The plugin's `SessionStart` hook therefore adds
one paragraph to every session's context asking the agent to prefer
`context_fetch` and `context_search` over `WebFetch` and `WebSearch`, and to
respect a refusal rather than retrying it with a built-in tool
(`plugin/README.md` §The standing nudge). Each delivery is recorded in the
session log as `nudge_issued` under the wording's versioned identity. It is
a request, not enforcement: nothing blocks the built-in tools, so expect
mediated crossings alongside observed ones.

A mediated fetch checks the operator's source policy first, records the
crossing either way, and hands the agent the bytes plus the hash it just
recorded. Of the in-process processors, the PII detector and
the injection screen run at this crossing, judging the text before the model
sees it, and each invocation is recorded as a `processor_invoked` event
beside the crossing it judged ([`docs/contracts/processor.md`](contracts/processor.md)); the context
optimiser runs at the batch runner's transform stage, the fidelity verifier on
every batch answer, the fidelity judge only in batch runs whose job declares
`fidelity_judge`, and the output provenance labeller only in batch runs
whose job declares `output_provenance`. One limitation:
the host does not tell the MCP server its session id, so mediated crossings
are recorded under a session of the server's own (`local-<timestamp>-<pid>`),
one per server process, alongside the host session's observed record. Both
appear in the console. The plugin's own tools are excluded from observed
capture, so a mediated fetch is never double-recorded.

## Credentials, when you want provider search

`context_search` and the batch runner's `--live` need provider credentials,
which live in `~/.commonmeasure/credentials.env` (`$COMMONMEASURE_HOME/credentials.env`)
beside `policy.json`:

```sh
cat > ~/.commonmeasure/credentials.env <<'EOF'
EXA_API_KEY=…
TAVILY_API_KEY=…
EOF
chmod 600 ~/.commonmeasure/credentials.env
commonmeasure credentials     # which providers are configured, and from where
```

The MCP server loads the file at start, so the mediated tools work in a
session launched from anywhere. The launching environment wins; nothing is
read from the working directory; the file is parsed, never sourced; and a
loaded file is recorded as `credentials_loaded` with path, digest and
variable names. `commonmeasure credentials` lists every provider the
adapters know and the variable each reads. A saved key reaches sessions that
start after it; a running session keeps the keys it started with.

The console's Sources page ([Sources](CONSOLE.md#sources)) lists the same
providers and adds, replaces or removes one key in the file without opening
it. It keeps every other line and sets the file to mode 600.

An unconfigured provider reports

```text
unavailable: exa needs EXA_API_KEY, which is not configured. Set it in
/home/op/.commonmeasure/credentials.env (KEY=VALUE lines, chmod 600), or export
it in the environment that launches the harness — the environment wins where
both name it. No search was attempted.
```

rather than a search that returned nothing without saying why. A configured
search records each result against policy and reports the provider's
observed charge in its own unit (an Exa search reports, for example,
USD 0.007).

One provider needs no credential: `context_search` with
`"provider": "internal"` queries the operator's own corpus, the directory
`COMMONMEASURE_INTERNAL_CORPUS` names, set in the launching environment or in
the same credentials file. Results carry the licence and effective dates
declared in the corpus's `corpus.json`, and the crossings are recorded only
when the corpus's `file://` prefix is named in `record_internal_prefixes`
([Source policy](contracts/source-policy.md)); without it the agent still gets the result and the record is withheld.

The replay mode needs none of this ([Building and replay](INSTALL.md#if-you-build-from-source)). A skill needs no credential either,
and is not reachable from a session tool: it is invoked only by a run, only
with `--live`, and only when `COMMONMEASURE_SKILL_CATALOGUE` names a catalogue
declaring it (`demo/skills/README.md` is a worked example;
[`docs/contracts/provider.md`](contracts/provider.md) §Skill supply is the contract).

## Scopes, engagements, principals and allowances

The source policy at `$COMMONMEASURE_HOME/policy.json` (by default
`~/.commonmeasure/policy.json`) also carries these declarations:

- **Scopes** bind a working directory to an engagement and to the rules
  that govern work there; the mode, constraints and clearance of the
  matching scope apply, and the top level applies where nothing matches
  ([`docs/contracts/source-policy.md`](contracts/source-policy.md) §Scopes and
  principals).
- **Engagements** are the names records, policy and reporting are grouped
  under. `allow_telemetry_egress: true` on a scope is the clearance for that
  engagement's witnessed crossings to leave the machine ([Egress](#egress-only-when-you-ask-for-it)); without it
  nothing leaves.
- **Principals** are authenticated identities: a `principals` list names
  who may exercise the policy, resolved from an authentication basis the
  process cannot rewrite, and a principal that cannot be authenticated is
  refused rather than degraded ([`docs/contracts/source-policy.md`](contracts/source-policy.md)
  §Scopes and principals). That basis is
  the process's operating-system user and is unix-only, so a policy that
  declares `principals` refuses every mediated crossing on Windows; a
  policy declaring none is unaffected there.
- **Allowances** bound a principal's cumulative spend over a declared
  period, enforced from a local transactional ledger before any priced
  acquisition is dispatched ([`docs/contracts/run-output.md`](contracts/run-output.md)
  §`acquisition.quote.allowance`).

For a firm the natural shape is one principal per solicitor and one scope
per matter. Each solicitor works as their own operating-system user on
their own machine, so the `principals` list binds a policy name to that
user's numeric id (`id -u` prints it); each matter is an engagement kept in
its own directory, and a scope matches that directory, names the matter as
the engagement and carries the clearance for its records to leave. The
policy for one solicitor and one matter:

```json
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.legislation.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ],
  "principals": [
    {"principal": "solicitor-a", "os_user": 501, "require_scope": true}
  ],
  "scopes": [
    {
      "match": "/home/solicitor-a/matters/2026-014",
      "principal": "solicitor-a",
      "engagement": "matter-2026-014",
      "allow_telemetry_egress": true
    }
  ]
}
```

The scope declares no constraints of its own, so the two access rules at
the top level govern work in the matter directory. `require_scope: true`
means this solicitor's agent can work only inside a matter directory the
policy names: outside one, every mediated crossing is refused rather than
falling back to the top-level rules. A second solicitor is a second
`principals` entry with their own user id, and a second matter a second
scope; a directory holding client-confidential material gets a scope whose
`constraints` refuse every host. What each field does, and what declaring
any principal changes for every other user of the machine, is
[`docs/contracts/source-policy.md`](contracts/source-policy.md).

[`docs/GLOSSARY.md`](GLOSSARY.md) defines each term in one line.

Where the file comes from a hub rather than your editor, because
`deployment.json` pins the hub's signing key, the edge refreshes it at every
session start and before every relay run, and `commonmeasure doctor` adds a
line naming the revision in force and its expiry. An envelope that has
expired keeps enforcing its policy and that line says `stale since` when;
[`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md) §Cadence
and staleness has the rest. After `connect --managed` into another
organisation, the previous organisation's revision stays in force until the
new organisation's first revision is accepted, and that line names the
organisation it came from (§Re-enrolment in the same contract).

## Import history from before Common Measure

Work done before the plugin was installed left host transcripts behind.
Always look before importing:

```sh
commonmeasure import --dry-run
```

which reports, per host, what a real import would do (`<host>  N new
crossings from M transcripts`) and names the hosts it will not import from,
with reasons: Codex transcripts hold shell command text, and a URL inside a
command is not evidence that anything was retrieved.

```sh
commonmeasure import
```

imports what the transcripts evidence. Every imported row is recorded as
`reconstructed`, read back after the fact with nothing watching at the time,
and never counted with witnessed evidence. Import is idempotent: a second run
imports `0 crossings`.

## Egress, only when you ask for it

Source records stay on your machine unless cleared for reporting. On a fresh
install the relay refuses:

```sh
commonmeasure relay
```

```text
commonmeasure: no telemetry receiver is configured: pass --receiver or set one
in ~/.commonmeasure/relay.json; nothing was projected and nothing was sent
```

There is no default destination and no default hub. A receiver is named in
`~/.commonmeasure/relay.json`, by `commonmeasure connect`, by hand, or for
one run with `--receiver`. The relay then projects witnessed crossings into
Content Telemetry v1.0 batches, spools them durably and delivers them.
Reconstructed crossings never leave, and a refused crossing leaves only as a
count on its session. A session's crossings leave only when the scope that
matched their working directory carries `allow_telemetry_egress: true`
([Source policy](contracts/source-policy.md)). What may leave, under whose clearance, and the wire format are
[`docs/contracts/telemetry-projection.md`](contracts/telemetry-projection.md).

The exception is a source whose licence demands usage reporting. With your
reporting consent ([Installing](INSTALL.md#reporting-consent)) it is
admitted in every scope, including one that sets
`allow_telemetry_egress: false`, and the relay sends the retrieval and
grounding of that crossing and nothing else of its session: not the turn
boundaries, not the other crossings and not the refused count. Without
consent such a source is refused in every scope, a cleared one included, and
the refusal names the source, says it needs reporting and gives the command
that agrees. Withdrawing consent refuses the next fetch of such a source; a
crossing admitted before the withdrawal is still reported, and nothing
already queued is recalled.

Operator terms that name institution identifiers (`terms[].access_context`)
need them on the session, which a Content Telemetry event batch cannot
carry. The relay holds each crossing of a host under such terms and sends
the rest of its session as cleared, and a source on such a host whose
licence demands reporting is refused, since nothing of it would leave.

The first run reads every session log in the home, including sessions
recorded before a receiver was configured, and decides each crossing under
the policy in force at that run. A later run under a wider policy sends
what the wider policy clears and never sends a delivered event twice.

To see what a run would send, without sending, refreshing policy or
changing files:

```sh
commonmeasure relay --dry-run --receiver http://127.0.0.1:9/events
```

```text
dry run: nothing was sent; no state was changed
would deliver 0 events in 0 batches to http://127.0.0.1:9/events (new at the receiver: unknown)
projected 0 of 1 sessions and 0 runs; 0 events would be newly spooled
  1 withheld: no crossing cleared to leave
hosts that would leave: none
forecast policy: /home/op/.commonmeasure/policy.json, as it stands on disk
a real run syncs managed policy and directory grants first, which can change what is cleared and what leaves
```

The four-fetches session is withheld because its policy clears no scope. `--policy <file>` forecasts a draft policy through the same loader.

Each run delivers only the batches that are due. `commonmeasure status` and
`commonmeasure doctor` show queued and dead batches, and
`commonmeasure relay requeue` starts another schedule for dead ones.

Once `relay.json` names a receiver, the relay also runs by itself when a
Claude Code session ends: the `SessionEnd` hook starts `commonmeasure relay`
in the background and returns without waiting. The other hosts send no
session-end event, so with them run `commonmeasure relay` yourself, and a
source whose licence demands usage reporting is refused there, unless a
background relay runs ([Background relay](INSTALL.md#relay-without-a-session-end)). On a managed home, `commonmeasure hosted
service` relays every session in the home on an interval while it runs,
which meets that demand too. Its sessions have the directory
`hosted-service.json` declares as `session_directory`, and are governed and
cleared by that directory's scope as a local session there would be; with
none declared they run under the top-level policy
([host integration](contracts/host-integration.md)). `commonmeasure doctor`
prints the last delivery and how automatic relaying is set up.

## Joining Common Measure Hub

An organisation's machines can enrol with Common Measure Hub, so that one
owner publishes a policy every machine applies and sees the cleared
evidence each one delivers. Setting that up is in the Hub guides:
[Start here](https://commonmeasure.ai/docs/hub/start-here/) takes a new
organisation from sign-in to its first delivery, and
[Connect a Common Measure edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/)
covers enrolling a machine, taking the organisation's policy and enrolling
a project.

On the edge, `commonmeasure connect <hub-url> --token <token>` writes the
receiver and ingest key into `relay.json`, mints the edge's signing key
(`edge-key.json`, which never leaves the machine) and records the hub's key
id in `enrolment.json`, then makes a first relay run. With `--managed` it
also pins the hub's policy signer in `deployment.json` and makes a first
policy sync.
`commonmeasure disconnect` revokes both keys at the hub when it can reach it
and removes the three files. The exchange is
[`docs/contracts/enrolment.md`](contracts/enrolment.md).

## Enrol a project directory

A project directory can be set to record locally only, or to report
through the relay: to the hub on an enrolled edge, otherwise to the receiver
`relay.json` names. With the Claude Code plugin, invoke
`/commonmeasure:enrol`; with Codex, `commonmeasure install codex` adds the
`$commonmeasure-enrol` skill. The agent shows the directory it is working in
and the edge's enrolment, then asks for a project name and a reporting
choice. The same from the command line, in the current directory or a named
one:

```sh
commonmeasure enrol                                   # show the current enrolment
commonmeasure enrol --name "My project" --reporting local
commonmeasure enrol --directory /path/to/project --name "My project" --reporting hub --include-history
```

Each prints the directory's enrolment as JSON. Its `reporting` field is the
state: `not_enrolled`, `local_only`, `relay_config_invalid` (reporting
chosen, and `relay.json` is one the relay refuses; `relay_config_error` says
why), `receiver_missing` (reporting chosen, no receiver configured),
`permitted`, or a reason reporting is held.

- Hub reporting covers the directory and everything below it, including
  eligible witnessed evidence recorded there before; `--include-history`
  acknowledges that. Sibling directories and other Git worktrees of the
  same repository are separate. A local-only ancestor blocks reporting
  below it. Source policy and confidential exclusions still apply.
- On a managed edge the request waits for an owner's approval on the hub's
  **Project reporting** page. `commonmeasure enrol --sync` then fetches the
  policy and the signed approval; a pending or expired approval, or a
  policy that withholds reporting, is reported as such.
- The MCP server resolves its directory when it starts. If it started in
  another directory, restart the host session in the project.
- `commonmeasure enrol --remove` stops reporting without deleting evidence,
  removing keys or disconnecting the edge. Events already delivered stay at
  the receiver.

Expiry and worktree handling are in
[`docs/contracts/directory-enrolment.md`](contracts/directory-enrolment.md).
