---
title: The console
domain: edge
audience: operator
section: use
---

# The console

The console shows the operator's own record: what content the agents took
in, under which policy, and what left the machine.

```sh
commonmeasure serve                              # http://127.0.0.1:4173
commonmeasure serve --listen 127.0.0.1:4180      # another loopback port
```

It is rendered from the session evidence logs in the operator home. An
index of them (`~/.commonmeasure/telemetry.db`) is refreshed before every
answer, so a crossing recorded while the console is open appears on reload.
Deleting the index loses nothing; it is rebuilt from the logs.

The index keeps each line as it first read it. A line changed in a log in
place, such as a credential redacted by hand, stays in the index as read
until the index is deleted. To clear it, stop the console (`Ctrl-C` on
`commonmeasure serve`; where the service runs it,
`commonmeasure service uninstall console`, since the service restarts a
`serve` that is killed), delete `telemetry.db` with its `telemetry.db-wal`
and `telemetry.db-shm` beside it, and start it again (`commonmeasure serve`,
or `commonmeasure service install console` from a shell with the same Edge
home: [Keep the console
running](GETTING-STARTED.md#keep-the-console-running-macos)). A console
left running keeps the deleted index open and serves it until it stops.

A measurement the record does not hold is shown as "unknown", never as zero
or a blank ([`docs/FAIL-POLICY.md`](FAIL-POLICY.md) §7). Terms such as
crossing, engagement, scope and policy mode are in
[`docs/GLOSSARY.md`](GLOSSARY.md).

On macOS, `commonmeasure service install console` keeps it running as a
LaunchAgent for the Edge home of the installing shell, logging to
`logs/console.log` in that home. `commonmeasure service status` reports the
service, the Edge home and log its plist names, and any console on the port
that the service did not start
([`docs/GETTING-STARTED.md`](GETTING-STARTED.md) §5). `commonmeasure update`
stops the service before it replaces the binary the service runs, and starts
it again with the Edge home and log in its plist, whichever home the shell
running `update` selects. It refuses a plist edited since `service install`
wrote it (§1 of the same guide, Updating). `GET /api/version`
answers the running binary's version and process id, which `service status`
compares with the `commonmeasure` on `PATH`.

The console writes four things: the policy mode, a scope's denied hosts,
the attribution rules, and the record of each comparison Compare runs.

## Access

Nothing in the console authenticates. It binds loopback and refuses any
other address without `--allow-remote`. A non-loopback bind exposes every
recorded URL, host, working directory and session, the Compare form and the
policy and attribution edits to anyone who can reach the address, and the
flag prints that as a warning. Requests naming a `Host` other than the
console's loopback address are refused, so a public page cannot reach it by
pointing its own name at 127.0.0.1, and so is a request carrying a foreign
`Origin`.

## Sections

Seven sections, each at its own address: Overview, Record, Agents, Policy,
Sources, Compare and Budget. Each screen names what it was read from and
when: the sessions directory and the time of the read, or for Policy the
policy file and its revision.

### Overview

`/`. Crossings recorded, witnessed by Common Measure, refused by policy,
and delivered by the relay. Beneath them, the most recent crossings by
host, and each engagement with its crossings, attributed by the working
directory each crossing ran in. The Hub card gives the relay's delivery
state, the receiver and the edge key with its standing. Where the stored
hub URL is one nothing is sent to, the standing reads in words beside
`cleartext_hub` or `unusable_hub_url`, and a callout gives the reason and
the remedy as `commonmeasure status` prints them. Where the spool is
refused, a callout states what it still owes in the words `status` uses; a
count it could not complete reads as unknown. Where `enrolment.json` exists
but cannot be read, the key reads as unknown and a callout gives the error
as `status` prints it.

### Record

`/app/record`. The sessions in a rail, grouped by the engagement the
attribution rules (`~/.commonmeasure/attribution.json`) resolve for each,
and the chosen session in a pane. Each session has its own address,
`/app/record?session=<id>`, which works without JavaScript.

- A rail row reads "N crossed" (observed and mediated together), with
  "N refused" and "N breached" when either is non-zero.
- A Claude Code session's mediated crossings are recorded under a separate
  `local-<timestamp>-<pid>` session of the MCP server's own, beside the host
  session's observed record, and the rail says so.
- The pane states the session's witnessed, reconstructed, grounded,
  refused and breached counts, then one card per crossing: URL, grade,
  whether it was grounded, host and licence, and the record's fields where
  the record carries them
  ([`docs/contracts/session-evidence.md`](contracts/session-evidence.md)
  §Crossing):
  - *bytes received*: `retrieved_hash`, the hash of the body the origin
    served, on a mediated fetch that received one;
  - *text extracted*: `content_hash` on a mediated fetch, the hash of the
    whole text taken from the body;
  - *text supplied*: `content_hash` on a search result, a mediated crossing
    that names its `supplier`: the supplier's hash of the text it
    delivered; no page was fetched, so nothing was extracted;
  - *text in context*: `content_hash` on a crossing whose `mode` is
    `observed`, the hash of the text the host's tool returned into context;
    no body was seen, so nothing was extracted;
  - *text in transcript*: `content_hash` on a crossing whose `mode` is
    `reconstructed`, the hash of the tool result as the host transcript
    holds it;
  - *part delivered*: `delivered.hash` and the part's range in characters,
    on a mediated fetch that returned text;
  - *status*: `http_status`, on a mediated fetch that was answered;
  - *identity*: `signed as <key id>` for an enrolled edge, or `unsigned`
    with the recorded reason, on a mediated crossing whose request left the
    machine.
- A refused crossing is a marked card with the reason. A refusal after the
  fetch also shows the hashes it recorded; one before the request has none.
- Witnessed and reconstructed evidence are never totalled together.
- A session that crossed nothing says so and names what it did record:
  turn boundaries, and lines the console could not read.
- `GET /api/sessions/<id>` returns the session's records in log order.
  An `edge_identity` record's `hub` and a `policy_sync` record's
  `policy_url` are given by their origin. A `policy_sync` record's
  `reason` is withheld whole, here and on Agents, where up to and
  including 0.4.1 it could quote the policy URL with its credentials: where
  the reason or the record's `policy_url` holds an `@`, where the
  `policy_url` carries a query or does not parse as a URL, where the
  reason holds the record's `policy_url` verbatim and that URL has a path
  other than `/`, a query or a fragment, as the old timeout did, and where
  an `unavailable` record's reason holds a `"`, as the old refusal of a
  `policy_url` did around the value. The console serves a note in its
  place; the log keeps the reason as written. A line the console could not
  parse is `{"event": "unreadable", "line": <number>, "bytes": <length>}`:
  its 1-based line number in the session log and its length in bytes as
  the console holds it, without the line ending, with none of its text.
  Where the line is not valid UTF-8 this length can differ from its
  length in the log. A torn `edge_identity` or
  `policy_sync` line can hold a hub or policy URL with credentials. The
  log and the console index keep the line as written; read the line in
  the log. Up to and including 0.4.1 the record carried the line as `raw`.

### Agents

`/app/agents`; the same data is `GET /api/agents`. The host sessions on
this machine, with a host's hook log and MCP server log joined by the host
process that started them
([`docs/contracts/session-evidence.md`](contracts/session-evidence.md)
§Host process). A log with no host-process record is listed alone. For
each: what it can be seen doing, the policy it loaded against the policy
the edge holds now, its latest context snapshot, its coverage from the
path table in
[`docs/contracts/host-integration.md`](contracts/host-integration.md) §6,
the edge identity it started under (the hub by its origin alone, the key
and its standing, and the edge's refusal of the hub URL where the
`edge_identity` record carries one), and anything that needs attention,
such as a delivery problem the relay reports, a revoked key or a refused
hub URL, or a running process that has recorded no work for an hour. A
process is shown running only when it is found in the process table; a
missing `session_ended` record is never read as running, and where the
table cannot be read the screen says liveness is unavailable.

### Policy

`/app/policy`. What governs the next crossing, read from
`~/.commonmeasure/policy.json` through the runtime's own loader:

- the mode in force (observe, prefer or strict) and what it does;
- how many engagements are cleared to leave the machine;
- one row per scope: its mode (marked inherited where the scope declares
  none), the engagement it governs, the directories it has covered, its
  denied hosts and its access rules;
- every scope where the engagement the policy declares and the engagement
  the attribution rules report differ, with both names. Neither overrides
  the other.

With no policy file the screen says so and names the path. The console
never creates a policy file.

Edits:

- The mode control and each scope row set a mode.
- The block form adds a host to a scope's denied hosts. A scope that
  inherited the top-level constraints keeps them as its own list from its
  first denied host, and the save says the scope has stopped inheriting.
- The attribution editor replaces the ordered attribution rules.
- `allow_telemetry_egress` and `record_internal_prefixes`, which widen what
  may leave the machine or enter the record, cannot be edited here.

Every form carries the revision of the file it was rendered from. A save
against a file that has changed since is refused with both revisions
stated and nothing written. A policy save goes through the runtime's
loader, so a policy the runtime would refuse is refused with the loader's
reason and the file is left as it was. A saved edit governs the next
crossing and nothing already recorded; a mediated session already running
keeps the policy it loaded until it next starts, and the page says so on
every save. A save answers 200, a conflict or an undeclared policy 409, and
a form or candidate the loader refused 400.

**Forecast.** Before saving, a draft can be tried against the recorded
history: every recorded crossing is put through the runtime's admission
check under the standing and the draft policy, and the table shows what the
draft would newly refuse, newly carry as a breach or newly admit, with
witnessed, reconstructed and previously refused crossings on separate lines.
A draft is a mode for a scope, a denied host for a scope, or an access rule
(a host pattern and an action); for an access rule the forecast shows the
JSON to add to the scope, since the console does not write access rules.
The forecast states:

- the principal it judged as, which is the console process's own, and how
  many crossings were recorded under another principal;
- that reconstructed crossings carry no working directory and are judged
  under the top-level policy;
- how many crossing records carried no URL and could not be judged.

A forecast writes nothing.

### Sources

`/app/sources`. Every provider that takes a key, and whether a key is
present in the operator's credentials. The console checks presence and
never calls the provider. A source says where content came from, not that
the operator may use it; the licence stays unknown unless a provider states
one.

### Compare

`/app/compare`. One query sent to the providers you select, one after
another, asking each for up to five results. None is selected at first.
`internal` queries the configured local corpus; any other provider is a
remote call that needs that supplier's credentials and network access and
may be charged. A configured key does not show that the account has access
or credit. Credentials are read at start from the launching environment and
the home's `credentials.env`; restart the console after changing them. An
invalid credentials file stops it starting.

**Next comparison setup** shows the operator home, directory and principal
the policy is resolved for. Each comparison applies the current local
source policy for that principal, with the same allowance check, admission
and text screens as a mediated search. It does not enrol with a hub, change
source permissions or refresh managed policy. Credentials held by a hub or
by a hosted edge are not available here.

Each comparison is recorded in `~/.commonmeasure/comparisons/<id>.ndjson`,
which the relay does not read:

- recorded: the query and its hash, the policy identity and effective
  policy, the principal, the requested limit, each supplier's outcome, the
  provenance of each result, refusals, charge and latency;
- not recorded: the suppliers' response bodies (only their hashes) and the
  returned snippets, which are screened but left out of the summary.

Unknown cost and latency stay unknown. A supplier's charge covers the whole
request, including results refused after they arrived, because admission
runs after acquisition. An allowance does not bound a supplier that states
no price until the dispatch after it is exhausted
([`docs/FAIL-POLICY.md`](FAIL-POLICY.md) §7). A failed evidence write stops
the remaining providers; nothing is retried or sent to a hub. A record with
only a start shows an incomplete attempt, which may still have been charged.

History links reopen earlier comparisons. **Source record** expands the
policy, processor and crossing evidence behind a result, and **Download
private source record** saves it.

**Export results** downloads a ZIP: a self-contained HTML report, summary
and per-case CSV tables and a JSON manifest, built from the recorded
evidence. The query text is left out unless **Include query text** is
selected; read that text before sharing the file. A comparison that
queried the internal corpus, or that returned internal or private results
from a remote provider, cannot be exported. The export makes no supplier
call and needs no hub account. Its format and the
`GET /api/compare/export` endpoint are
[`docs/contracts/comparison-export.md`](contracts/comparison-export.md).

Compare is a retrieval check. Answer quality is measured by a batch run
([`docs/contracts/run-output.md`](contracts/run-output.md)), which the
console does not display: `commonmeasure inspect <dir>` prints a run's
dossier, including its spend.

`POST /api/compare` takes `{"query":"...","providers":["internal"]}`; an
empty, repeated or unknown selection is refused before anything is sent.
`GET /api/compare` lists the recorded comparisons and
`GET /api/compare?comparison=<id>` reads one. These routes are under the
same loopback, `Host` and `Origin` rules as the rest of the console.

### Budget

`/app/budget`; the same data is `GET /api/budget`. The recorded context
footprint by engagement, the acquisition caps the policy declares (labelled
declared), and each principal's allowance beside its standing in the
runtime's ledger, read through the same loader and ledger that enforcement
uses. Session logs carry no acquisition charge per engagement, so there is
no spend total per engagement, and the screen says so. An allowance whose
declaration or ledger cannot be read is shown as unreadable.

The context footprint, here, on each session and in the internal-use view,
sums `estimated_tokens` over the witnessed crossings, observed and mediated,
each basis apart; for a session with a context snapshot, it is the figure
`commonmeasure session` states. A reconstructed crossing's estimate is of
the text in a transcript, not of what entered context, so its tokens are
shown apart as the reconstructed figure (`reconstructed_estimated_tokens`
and its companions in the JSON) and never added to the witnessed one; a
session made only of imported crossings has a reconstructed figure and no
witnessed one. A refused crossing counts in neither, even where it records
an estimate, because its text was withheld.

## The guide

`/guide` serves the guide to context window optimisation and
`/guide/state-of-the-evidence` its evidence companion: self-contained pages
with no scripts or external assets. `commonmeasure guide <dir>` writes the
public version of the same pages, with the product panels removed, as
standalone HTML files.
