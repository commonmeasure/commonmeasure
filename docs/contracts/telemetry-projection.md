---
title: Content Telemetry projection contract
domain: network
audience: integrator
section: reference
---

# Content Telemetry projection contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

- **Owner:** Network. The relay's spool and delivery are the Edge's code,
  described here because they decide what a receiver is sent and when.
- **Producer:** the edge's relay (`crates/commonmeasure-relay`), from the
  source record ([session evidence](session-evidence.md)) and from published
  runs named to it ([run output](run-output.md)).
- **Consumer:** the receiver named in `relay.json`: the operator's hub once
  the edge is enrolled ([enrolment](enrolment.md)), or any receiver that
  accepts a Content Telemetry `event_batch`. The hub delivers each content
  owner's events onward ([onward delivery](onward-delivery.md)).
- **State:** built. Projection, spooling and delivery are `fixture-tested`:
  recorded inputs through the real relay transport to a loopback receiver
  (§Verification). The ignored tests in
  `crates/commonmeasure-cli/tests/instance_e2e.rs` deliver to a running hub;
  no recorded run of them is kept here.
  Acceptance by an independent conforming receiver running its current code
  is not recorded (NET-6).

The source record is private. The relay selects from it, under the operator's
clearances, the events Content Telemetry defines, and sends them to one
receiver. Nothing else in the record leaves the machine. Terms such as
crossing, engagement, scope and receiver are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

## What is projected

The relay emits Grounding-level events only: `content_retrieved`,
`content_grounded`, `turn_started` and `turn_completed`. It emits no citation,
presentation, reproduction or engagement claim. The pinned schemas are listed
in `schema/SOURCE.md`; `crates/commonmeasure-relay/src/project.rs` implements
the selection, `crates/commonmeasure-relay/src/wire.rs` the wire types, and
`conformance/` freezes the emitted batches.

A batch is an `event_batch` document of one session, `schema_version` `1.0`,
with at most 500 events:

| Member | Value |
|---|---|
| `session_id` | the session identifier where it is a UUID, otherwise a UUID derived from it; a run's identifier for a run |
| `agent_id` | the `key_id` of the session's first `edge_identity` record, or `commonmeasure`, pinned per session at first delivery (§The agent id on the wire); always `commonmeasure` for a run |
| `started_at` | the earliest event's timestamp |
| `refused` | the session's refused count (§The refused count on the wire); absent on a run's batches |
| `events` | the events |

Each event carries `id`, `type`, `timestamp` (UTC, milliseconds) and
`source_role` `agent`. An event's `id` is a UUID derived from the record it
projects (the session, the record's position in the log and the event kind;
for a run, the run, plan and rank), so a redelivery carries the same `id` and
a receiver counts it once.

- **`content_retrieved`**, one per cleared crossing: `content_url` (the
  crossing's URL without user information); `license_ref` where the crossing's
  licence is `declared`, also without user information, since a licence URL
  resolved against the page's URL keeps any the page URL carried;
  `content_telemetry_id` (below); `instance` (§Instance reference).
- **`content_grounded`**, for a crossing that grounded: `data.scope`
  `session`, `data.content_hash` and the ingestion measure (§Ingestion
  measure). `data.content_hash` is the hash of the whole text extracted for
  the agent, of which one fetch result may carry only a part, as session
  evidence `delivered` records; it is never `retrieved_hash`, the hash of the
  body as served. A crossing whose context entry the host
  observes is grounded by the observation instead, with `data.scope` `turn`
  and the observation's representation hash (§Grounding from host
  observations).
- **`turn_started`**, **`turn_completed`**: a turn boundary (§Selected
  coverage).
- **A run** named to the relay projects each admitted source with a URL as a
  retrieval, and as a grounding where it has a content hash, all at the run's
  start time.

`content_telemetry_id` is the UUID a mediated fetch sent as its
`Content-Telemetry-ID` request header (Content Telemetry section 7.2,
[session evidence §Source declarations](session-evidence.md#source-declarations)),
so an owner that logged the header can match the retrieval event to its own
log line. The relay carries it on the retrieval event and on no other.

## Custom fields

Content Telemetry allows an emitter's own members in an event's `data`. The
relay adds these, and two members outside `data`, which the pinned schemas
admit because they do not close their objects:

| Field | Where | Value |
|---|---|---|
| `data.commonmeasure-projection` | every event | the projection's conformance level and coverage (§Selected coverage) |
| `data.commonmeasure-supplier` | retrieval and grounding events of a search result or a run's plan | the supplier that served it (`exa`, `ozone`, …); withheld for the operator's own corpus (`internal`) and for skills (`skill:<name>`) |
| `data.contextops-host-tool` | session events | the host word ([session evidence §Client identity](session-evidence.md#client-identity)); on retrieval and crossing grounding events, that of the session's first witnessed crossing, for the whole session; on turn events and observation grounding, the record's own |
| `data.token_basis` | grounding events with `tokens_ingested` | the recorded basis of the count (§Ingestion measure) |
| `data.commonmeasure-output-associations` | `turn_completed` with an output observation | `{"count": n}` (below) |
| `data.commonmeasure-crawl-delay` | retrieval events of a mediated fetch sent under a host's `Crawl-delay` above zero | `{"delay_ms": n}`: the delay this edge kept for the host that answered (the reading's `honoured_ms`, at most 60000), read from the delay ruling on the crossing's `robots` evaluation ([session evidence §Source declarations](session-evidence.md#source-declarations)). The ruling can be read when its `host` matches the host that answered, its `delay_ms` is an integer from 1 to 60000 and its `outcome` is `clear` or `waited`; other members of the ruling are not read, and their values do not affect the field. Absent where the group states no delay, states `Crawl-delay: 0` or a value that cannot be read, where the ruling is missing or cannot be read, on every other event and on a run's events. The hub forwards `data` onward, so a content owner receives its own host's delay on its own retrievals |
| `instance` | retrieval and grounding events, beside `data` | the instance reference, to the issuing hub only (§Instance reference) |
| `refused` | the batch, beside `events` | the session's refused count (§The refused count on the wire) |

`data.commonmeasure-output-associations.count` counts the associations of a
host's output observation
([session evidence §Output-association observations](session-evidence.md#output-association-observations))
whose acquisitions and context entries pass selected coverage and privacy
checks. It is distinct from the number of `content_grounded` events, and a
receiver counts it once per boundary event ID. It is absent on boundaries
without an output observation, so a missing observation never becomes a
measured zero. It remains a Grounding-level extension: the relay makes no
standard `content_cited` event and no Citation-conformance claim. The
boundary needs its own clearance and at least one eligible source event, as
§Selected coverage requires.

## Selected coverage

`commonmeasure:telemetry-selection:v1` is the opaque reference for the selection
rule in this section. Coverage is **selected** for each emitted event type;
the relay never declares complete coverage. Each event carries the following
informational declaration in the schema's extension container:

```json
{"data": {"commonmeasure-projection": {
  "conformance_level": "grounding",
  "coverage": {"mode": "selected", "terms_ref": "commonmeasure:telemetry-selection:v1"}
}}}
```

The extension describes the event's projection. It does not add a standard
batch field or replace an emitter manifest. An emitter manifest advertising
this projection uses `telemetry.conformance_level: grounding` and a
`telemetry.coverage` entry for each of the four event types, with
`mode: selected` and the same `terms_ref`. Manifest publication belongs to the
participant operating the endpoint; the relay does not publish one.

The relationship scope is the source activity selected for the operator's
configured receiver. A witnessed session crossing is cleared by its recorded
working directory. Without a directory selection, the directory must match a
policy scope that names a governing engagement and sets
`allow_telemetry_egress` ([session evidence §Crossing](session-evidence.md#crossing)).
With one, the directory's enrolment and current consent decide, a matched
scope that sets the flag false vetoes (§Directory reporting consent). In
either case a directory the policy cannot resolve safely does not clear: an
unbound principal, a scope owned by another principal, or a symbolic link or
linked worktree that resolves to another scope. Managed reporting approvals also apply
where directory reporting is configured. A session any of whose cleared
crossings falls under operator terms naming institution identifiers is
withheld whole ([session evidence §Source declarations](session-evidence.md#source-declarations)). A run explicitly named to the relay contributes its admitted
sources. Both paths exclude private addresses and named internal prefixes.
A session also excludes crossings marked internal, reconstructed crossings,
failed fetches and non-2xx responses. Refused sources leave only as the existing
session count; rejected run sources do not leave. Prompts, answers, evaluator
output, costs and private record detail are excluded. Unobserved host activity
and records missing a usable URL or timestamp cannot establish an event.
These exclusions mean absence of an event cannot establish absence of use.

A turn boundary requires its own clearance, resolved from `payload.detail.cwd`.
It accompanies a session only when at least one source event is eligible.
A neighbouring public crossing never clears a private boundary. The projection
copies the recorded `turn_id` where supplied, normalises the timestamp and
carries `turn: {"privacy_level": "minimal"}`. It excludes the working directory,
transcript path, privacy rationale and all conversation content. A boundary
without a recorded `minimal` declaration or usable timestamp is excluded; the
relay does not invent missing host boundaries or turn identifiers. Turn event
IDs derive from the session, original log position and boundary kind, so
changed clearance cannot renumber previously delivered events.

## Instance reference

A `content_retrieved` or `content_grounded` event whose source record carries
the `instance` reference ([session evidence §Where](session-evidence.md#where)) carries it as an event-level member, beside
`data` rather than inside it:

```json
{"instance": {"issuer": "https://hub.example", "id": "5c2f8d8e-7a41-4a0b-9e55-1f6b3c9d2e70", "revision": 1}}
```

`issuer` is the registration service's origin without a trailing slash, `id`
the instance identifier and `revision` the integer binding revision held when
the source record was written. Each event takes the reference of its own
source record: a crossing's events take the crossing's, and a grounding event
projected from a host `context_entered` observation takes the observation's.
Records written after a renewal therefore carry the later revision, and events
projected from earlier records keep the earlier one.

The member is not a standard Content Telemetry field. It is projected only
when the origin (scheme, host and port) of the receiver the relay is
projecting for equals the recorded issuer: after enrolment the configured
receiver is the hub's origin followed by its telemetry path, and the issuer is
that origin. For any other receiver the member is absent and the batch is
otherwise identical. The hub uses the member to join the event to the
instance's reporting duties and removes it before onward delivery (tested
in the hub's own repository, not here).

No member is projected for a record without a reference, for a reference that
is not a string issuer, a non-empty string identifier and an integer revision,
for a turn boundary, or for a published run, whose summary carries no
reference. A session with no registration projects exactly what it did before
this member existed. Absence of the member claims nothing about registration.
Event identifiers do not depend on it, so an event delivered before this
member existed is not delivered again and its duty is not joined by it.

The issuer check is made when the batch is projected and again on the
document each delivery posts. A batch can wait in the spool across a change of
receiver (`--receiver`, or an enrolment with another hub rewriting
`relay.json`); delivery removes the member from every event whose issuer is
not the origin of the receiver it is posting to, beside the `agent_id`
rewrite. A batch carries no digest or signature over its events, so nothing
is invalidated. The spooled file stays as queued, so a later delivery to the
issuer still carries the member. A batch projected for a receiver that was
not the issuer has no member to restore, and a later delivery of it to the
issuer joins no duty.

Delivery state is kept by event identifier, whatever the receiver
(§Delivery state). Events another receiver accepts are recorded delivered and
are not sent to the issuer by a later run, so the issuing hub reads nothing
for them and a reporting duty of that instance cannot be met from them. The
relay's report counts the members delivery withheld from batches that held
one (`RelayReport.instance_references_withheld`), `commonmeasure relay`
prints a warning when the count is not zero, and a dry run forecasts the
same. A run in which another batch failed carries the same count on its
failure (`DeliveryFailure.instance_references_withheld`) and the command
prints the warning after the failure, because the accepted batches' events
are recorded delivered whatever happened to the rest. The count does not cover events projected for a receiver that was not
the issuer, which never carried the member.

## Supplier scope

A receiver may be scoped to named suppliers: `relay.json` takes a `suppliers`
list, such as `["ozone"]` for a supplier's own telemetry server.

- Only events whose `data.commonmeasure-supplier` names a listed supplier
  leave for that receiver: the supplier's retrievals and any recorded
  grounding of what it served. The operator's own fetches, other suppliers'
  results and turn boundaries stay home.
- A scoped receiver gets grounding only where the source record holds it.
  Mediated search records a supplier's results as retrievals and records no
  grounding for them, so from mediated search a scoped receiver gets
  retrieval events only.
- A batch for a scoped receiver has no `refused` field. The count covers the
  session's refusals of every source, and a supplier is owed nothing about
  sources it did not serve; a zero would state a fact about the session that
  is not true (§The refused count on the wire). No batch is sent to a scoped
  receiver only to carry a moved count.
- An absent list, or `null`, means every cleared event and the refused
  count, as for any receiver. Enrolment writes no list, so the operator's own
  hub is unscoped.
- An empty list, `[]`, is scoped to no supplier: the receiver is sent
  nothing, and nothing is queued or recorded delivered for it.
- Any other value is a load error, and the relay sends nothing until it is
  corrected: a string, an object, a list holding anything but names, or an
  empty name. The harness reads `relay.json` with the relay's parser, so the
  same file leaves a reporting demand unmet with the load error as the
  reason ([session evidence §Source declarations](session-evidence.md#source-declarations)).
- The scope belongs to the receiver `relay.json` names, however a
  `--receiver` override spells it. The two are one receiver when they reach
  one endpoint (below); an override to another endpoint is not scoped.
- The spool is shared by every receiver the relay has been pointed at. A
  queued batch is narrowed again on the document each delivery posts to a
  scoped receiver, as the instance reference is; a batch left with no event
  is held undelivered for a later unscoped receiver. The spooled file stays
  as queued. Events narrowed out are not recorded delivered, so a later
  unscoped relay projects them from the session log again.
- A page this edge or the host fetched names no supplier, so a scoped
  receiver never carries it. A run's fetch through a supplier's `fetch` names
  that supplier, and its receiver gets the retrieval and any recorded
  grounding. A reporting demand on a fetched page's licence or the
  operator's terms is ruled unmet while `relay.json` sets `suppliers`
  ([session evidence §Source declarations](session-evidence.md#source-declarations)).

### One receiver

The relay posts each batch to the receiver with its trailing slashes
removed, followed by `/events`, and parses that URL with the WHATWG URL
rules. Two receiver URLs are one receiver when the URLs posted to are equal
after that parse (`commonmeasure_harness::relay_config::same_receiver`).
Scope selection uses this comparison alone. These spellings are one
receiver:

| Difference | Example | Handled by |
|---|---|---|
| Trailing slash, one or more | `/telemetry/`, `/telemetry` | the `/events` derivation |
| Empty path | `https://r.example`, `https://r.example/` | the URL parse |
| Scheme or host case | `HTTPS://R.Example/t` | the URL parse |
| Explicit default port | `https://r.example:443/t` | the URL parse |
| Dot segments | `/x/../t`, `/./t` | the URL parse |
| Host spelling | IDNA names, IPv4 and IPv6 address forms | the URL parse |
| Credentials in the URL | `https://user@r.example/t` | not sent by the transport |
| Fragment | `https://r.example/t/events#x` | not sent by the transport |
| Trailing dots on the host, one or more | `https://r.example./t`, `https://r.example../t` | removed for the comparison only |

A host with its trailing root dot reaches the same server: the transport
resolves the absolute name, sends it in `Host` as written, and the TLS
client drops the dot from the server name and matches the certificate. The
comparison removes every terminal dot, reading the host as a crossing's host
is read (`commonmeasure_harness::grounding::host_of`). More than one is not a valid DNS
name, and treating two spellings as one receiver can only apply a scope. A different scheme, port, path, path case or query
is another receiver.

`relay.json` is refused at load when its receiver URL has a query or a
fragment (the `/events` suffix would land inside it rather than on the
path), credentials (the transport never sends them; the key belongs in
`api_key`), a scheme other than `http` or `https`, or no host. A
`--receiver` override is not refused for these; it is compared as above, and
one the transport cannot post to matches nothing.

The load error names the fault and the receiver's origin (scheme, host and
port, such as `https://hub.example:8443`), and no other part of the URL: a
key can sit in the credentials, the query or a tokenised path, and the error
reaches `status`, `doctor`, directory status, the source record and, as the
reason for an unmet reporting demand, the agent. A receiver that is not a
URL is named by no part of itself. `status` and `doctor` name the receiver of
the last recorded delivery by its origin in the same way, or as "an earlier
receiver" where it has none, and show a recorded delivery error that quotes a
receiver URL without the URL: `relay/receipts.json` written by 0.4.1 holds
the receiver as it was configured, and a `--receiver` override is kept as
given. A host can itself be a capability, since
some webhook services put the token in the host name, so for such a receiver
the origin shown is sensitive too.

The comparison is by URL. Two host names that resolve to one server, such
as `localhost` and `127.0.0.1`, are two receivers to it.

The list narrows what this edge sends. It is not the authority on what a
supplier may see: that follows the supplier's grant, and an operator may
narrow within it but never widen it (owner decision, 22 September 2026).
Nothing here checks the list against a grant yet.

## Ingestion measure

A grounding event carries `tokens_ingested` only when its source record has a
non-negative integer count and a non-empty recorded `token_basis`. The relay
copies both, preserving `token_basis` as a custom event `data` field retained
from the existing wire format. It is not a standard field. Session counts come
from `payload.estimated_tokens` and `payload.token_basis`; published run counts
come from each admitted source's `tokens` and `run.token_basis` in `summary.json`.
The projection never substitutes a current estimator for a missing recorded
basis. A grounding event projected from a host context-entry observation
carries neither. An absent count or basis leaves both fields absent, and a recorded zero
with a basis remains zero. `characters/4` and `whitespace-words` are estimates,
not model-tokeniser counts or measurements comparable between emitters.

The relay does not emit `chars_ingested`: it reads hashes and counts from the
record, not the captured text. Multiplying a rounded token estimate would not
recover the Unicode code points placed in context. Existing `content_hash`
continues to identify the captured content; page text never enters telemetry.

## Delivery state

The relay retains each projected batch in `relay/spool/outbound.ndjson`.
`queued_at` records when it entered the spool, and each line carries its spool
`index` as its first member. Private metadata records `queued`,
`delivered` or `dead`, the attempt count, next attempt time, last attempt time,
last error and a separate `hold_reason` for each spool index. These fields
never enter the Content Telemetry document. A batch with no metadata is queued
and unclaimed.

The metadata is a snapshot and a journal. Each state change is one line
appended to `relay/spool/outbound.delivery.journal` and made durable before
the operation returns; the line carries the batch's whole state, so replaying
it twice changes nothing. When the relay or `relay requeue` closes the spool it
folds the journal into `relay/spool/outbound.delivery.json`, which then holds
every retained batch, and replaces the journal with a header carrying the next
`generation`, the next spool index and the count of pruned deliveries. After a
clean close the journal holds its header alone; a spool that has never
recorded a state change has no journal. A reader reads the journal before and
after the queue and snapshot and reads again if the generation changed, so it
never combines one generation's snapshot with another's journal. It also reads
again when the journal states a batch its copy of the queue lacks, which a
writer that enqueued and claimed during the read produces. After eight
attempts the read fails with an explicit error, and status reports delivery
state as unavailable with unknown counts. A relay stopped before it closes
leaves the journal in place; the next reader and writer replay it, and the
next clean close folds it.

An enqueue or journal line cut short by a crash was never reported as
written, and the next writer removes it. An unterminated final queue line
that parses as JSON was not cut short, because no strict prefix of a line
parses: it is checked like every other line. A whole entry is given its
newline, since a restored file may end that way; if it was an interrupted
enqueue, its event ids count as spooled and nothing is projected twice. Any
other such line is refused or reported as damage, as below, and never
removed. Every queue line is
checked to be a whole entry, and every journal line parsed, whenever the spool
is read, so damage before the last line of either file is an explicit error
from the relay, `relay requeue`, a dry run and status, and no line is skipped
or removed. One damaged queue line therefore stops the delivery of every
queued batch until it is repaired. Removing the line does not repair it: the
metadata still names its index. The number in the report is the batch's spool
index, the line's `index` member. The batch's metadata is the snapshot
member and the journal records that carry that index. For a damaged line of a
delivered batch, replace the line with a placeholder that keeps its index, for
example
`{"index":7,"origin":"damaged line replaced","directory_selection":false,"document":{"events":[]}}` for
spool index 7; the spool then opens, delivery resumes and the placeholder is
pruned under the retention rule below. For a damaged line of an undelivered
batch the payload is lost. While no relay runs, remove the line, or empty it
and keep its newline, and remove the batch's entries from the snapshot and the
journal. Every other line names its own index, so no other batch moves. Then
run the relay, which projects the events again where the session log still
reads and the clearance in force admits them. A damaged line that has lost the
start of its `index` member is reported as naming no index, and a line whose
index is not above the one before it as out of order, each by its line number
counted from zero; the remedies are the same, and where the snapshot or the
journal has an entry for the batch, its index is the one no other line
carries.
Metadata left for a line that was emptied or deleted is reported as naming
batches the queue lacks. A journal append that fails is cut back to the last
durable record; that process records no further change and its close folds
and prunes nothing. A close that cannot fold the journal loses no state and
writes its reason to `relay/spool/outbound.close-error`, which status reports
as `close_error` until a later close succeeds; writing the reason is best
effort. The file is read for status alone: its first 4 KiB, with bytes that
are not UTF-8 shown as replacement characters and control characters as
spaces. A file that does not read is reported as `close_error_unreadable`
with `close_error` null, because it does not show whether the close failed;
removing it is safe. Neither stops a relay run, `relay requeue` or the status
counts.

Delivered batches are pruned at close. A delivered batch is released once its
accepted attempt is 30 days old, or once more than 1,000 delivered batches are
retained, lowest spool index first; the second rule also bounds deliveries
whose time is unknown. Index order is enqueue order: a batch requeued and
delivered today is released before a newer batch delivered last week. Thirty
days is therefore not a minimum: an edge that delivers more than 1,000 batches
in the period keeps less. Private batch metadata records
`instance_references_withheld`, the number of `instance` members the accepted
attempt withheld (§Instance reference); the field is absent until the batch is
accepted, and may be absent on an older accepted batch, when delivery withheld
no member. A batch with a count above zero
is released by the 30-day rule only, because its retained payload is the only
spooled copy that carries the member for the issuing hub, and a remedy that
sends it from the spool needs a period that does not shrink with delivery
volume. After release the member can be recovered only by projecting the
session record again. Two exemptions apply to released batches. A delivered
batch whose session has no pinned agent identifier
(`relay/session-agents.json`) is kept, because its payload is the only record
of the identifier first sent. The remaining batches are removed when they make
up at least one eighth of the retained batches, so the queue file is not
rewritten for every delivery. Queued, held and dead batches are never pruned.
The journal names the batches to prune before any file loses them; a crash
part-way is completed from that line, and readers treat the named batches as
pruned meanwhile. A pruned batch's payload and metadata are gone from the
spool; its event ids stay in `delivered.idx`, its source stays in the session
record, and it stays in the delivered batch count. Only the journal holds the
count of pruned deliveries. Before the queue first loses a batch, a prune
writes `relay/spool/outbound.pruned`, which carries no count and which the
relay never removes; a manual recall of the spool removes it with the journal,
or the emptied spool's count reads unknown. If the journal is missing beside
that file, or beside a queue whose indices have a gap, the delivered batch
count is unknown, not the retained part of it, and stays unknown. The file
matters where the queue shows nothing: after a prune of every batch, or of the
newest batches alone. A spool pruned only by a version from before the file
has the gap as its one sign, so in those two cases it reads zero once its
journal is missing. A spool index is never given to a second batch while the
journal exists.

The relay does not read a spool that holds `relay/spool/outbound.ack` or a
queue line without an `index`. A queue line that is a whole entry without an
`index`, the final line included whether or not it ends in a newline, or an
`outbound.ack` beside the queue, stops the relay,
`relay requeue`, a dry run and status with an error naming the line or the
file and the remedy; nothing is sent and nothing on disk changes. Status and
doctor then report `refused_spool`, read from the lines that carry an index
and their recorded states: `outstanding`, the batches recorded as queued or
dead (and, with no `outbound.ack`, those with no recorded state); `unknown`,
the batches with no recorded state beside an `outbound.ack`, which may
record them as accepted; and `unindexed`, the lines without an index.
Neither that file nor those lines is read to count them. `incomplete` is null
when every indexed line and every recorded state was accounted for.
Otherwise it says why not: the recorded states name a batch no indexed line
carries (as the ordinary read's check finds; the position of a line without
an index may be such a batch), a line is neither a whole indexed entry nor
one without an index, or a file did not read. The three counts then cover
only what was read and are lower bounds. Status and doctor state that no
indexed batch is outstanding only when all three counts are zero and
`incomplete` is null. `refused_spool` is null for any other spool.

When `relay/spool` is moved aside, the next relay run that names no
`--session` projects again, from every session log under `sessions/` that
reads and under the consent, approvals and policy in force, every event
`relay/delivered.idx` does not record as accepted, so no accepted event is
queued again other than to carry a refused count (§The refused count on the
wire). A run with `--session` projects only the sessions it names. A run's
events are projected again only when the relay is given that run's `--run`.
`delivered.idx` holds event ids, not payloads, so an outstanding event whose
session log or run input is gone is not sent again: the relay exits
successfully, status and doctor do not count the event, and it stays in the
saved spool, undelivered. The batch counts start again from the new spool; the
delivered event count, which `delivered.idx` holds, does not.

A batch with `queued_at: null` is held with a reason naming it and never
sent: a rewrite of the spool may have filled in its `directory_selection`, so
its consent provenance is unknown. Once no other batch is queued or dead,
moving the spool aside projects its events again under current consent where
its session log remains.

Before sending, the relay durably claims the attempt and its next deadline.
The first retry waits 60 seconds, which exceeds the HTTP client's 30-second
exchange budget. The delay doubles to a ceiling of 3,600 seconds, so a lasting
outage receives at most one attempt per hour per batch. Ten attempts bound
automatic work while allowing recovery from transient outages. The normal
schedule reaches its tenth attempt after 14,580 seconds (4 hours 3 minutes).
A failed tenth attempt becomes `dead`. If the process stops after claiming an
attempt, that batch becomes eligible at its persisted deadline; after the final
claim, the next due invocation marks it dead with its retained error. A claim
whose outcome was never written explicitly records that acceptance is unknown.

Any `2xx` status marks a batch delivered; any other status, or no answer, is
a failure. The Content Telemetry standard (`schema/`) defines no response
body, so a body is optional, and the hub's onward delivery applies the same
rule ([onward delivery](onward-delivery.md)). When the body is JSON with an
unsigned `events_created`, the relay reports that count of newly recorded
events. Otherwise the count is unknown, never zero, and the run's report
states "new at the receiver: unknown" if any batch in it was accepted without
a count. A zero count is valid for a redelivery. Acceptance of a later batch
does not acknowledge earlier queued or dead batches. Later batches may arrive before
earlier ones, including within the same session; receivers deduplicate event
ids and retain the highest refused count. A crash after acceptance but before
local acknowledgement may send the same event identifiers again. Undelivered
payloads, dead batches included, remain on disk. Where a directory selection
applies to the home or the batch, current directory consent, reporting approvals and source
policy are checked again when the batch is due. The relay sends the cleared subset and records
acceptance for that delivery, leaving withheld event ids out of `delivered.idx`.
Private batch metadata records `delivered_subset: true` when the accepted
attempt omitted events from the retained batch, and `false` when it included
all of them. The field is absent until the batch is accepted, and may be
absent on an older accepted batch, whose subset is then unknown.
Those ids can be projected again if clearance returns. If no events remain
cleared, the batch stays queued with a separate hold reason and consumes no
HTTP attempt; the last delivery error is preserved. Repeated unchanged holds
do not rewrite metadata. A held batch is rechecked on later due invocations,
and the first due invocation after clearance returns (a new opt-in, or a
re-approval on a managed edge) sends it. A batch is rechecked only when due, so
one in retry backoff from an earlier failed attempt is neither held nor sent
before its deadline, whatever its clearance
(`crates/commonmeasure-cli/tests/hosted_service/reporting.rs`).

Queued, held and dead batches are undelivered. A held reporting obligation has
no automatic grace period. `commonmeasure status`, `commonmeasure doctor` and
the console show queued, dead and delivered **batch** counts, oldest queued age,
next attempt and last error. The existing delivered and pending **event** counts
remain separate; pending includes dead batches. Unreadable delivery state is
reported as unavailable, with unknown counts. A batch held for having no
`queued_at` has an unknown queue age.
`next_attempt_at` reports only a persisted deadline for an unheld queued batch;
it remains null when none exists. Status and doctor distinguish due batches
from policy holds, and report the hold reason separately from delivery errors.
The status report exposes `held` and `due_now` batch counts and `hold_reason`
separately. A batch whose session log does not read at the directory recheck
is held the same way and counted in `held`; its reason states that the log
does not read, and status, doctor and the console print it under the same
"policy hold" label.
A relay forecast includes only batches currently due and cleared for delivery:
the batches earlier runs spooled that are due, and the events this run would
project, turn boundaries included. `commonmeasure relay --dry-run` leads with
the events and batches that would be delivered and their count per event type
(`content_retrieved`, `content_grounded`, `turn_started`, `turn_completed`),
and a real run prints the same lines for what it delivered. The "newly
spooled" count that follows covers this run's projection only, so it is lower
than the delivered count whenever earlier batches are due
(`crates/commonmeasure-relay/tests/relay.rs`
`a_dry_run_forecasts_per_type_exactly_what_the_real_run_then_sends`).

A forecast is not the run. It makes no network call and writes nothing, so it
syncs neither the managed policy nor the reporting approvals, and it projects
against the policy and the approvals already on disk. A real run syncs both
before it projects, and the clearances and internal prefixes it then reads
can differ from the ones forecast, on a managed edge in particular. The
forecast ends with two lines saying what it was taken against — the applied
managed revision and whether it is stale, the local policy file, or the draft
passed with `--policy` — and that a real run refreshes both first. Over an
unchanged home the counts match.

`commonmeasure relay requeue` starts another schedule for all dead batches;
`commonmeasure relay requeue --batch 3` selects spool index 3. This resets the
schedule's attempt count and makes the batch due immediately, retaining its
last attempt and error until the next claim. Requeue sends nothing; run
`commonmeasure relay` to deliver due batches. Queued and delivered batches are
not requeued. CLI and service-mode invocations share the same persisted cadence
and an exclusive spool lock. Service mode checks on its existing interval;
the standalone command does not start a background worker. On a local edge
the session-end hook starts one relay run in the background (§Relay at session
end). A run takes the spool lock before it asks the hub for a standing,
refreshes the reporting approvals or writes to the relay directory, so a second run
started at the same moment exits before doing any of those. The command syncs
managed policy before the relay takes the lock, so on a managed edge the
losing run can still fetch the policy envelope and rewrite `policy.json`
before it exits.

Every process using an Edge home runs the same release (`ARCHITECTURE.md`
§What runs where).

## Receiver and key

The API key in `relay.json` belongs to the receiver in `relay.json`; on an
enrolled edge it is the hub's ingest key. The relay sends it as `X-API-Key`
only to that receiver's origin, compared as the instance issuer is compared
(§Instance reference). A delivery to another receiver named with `--receiver`
carries the key given with `--api-key`, or no `X-API-Key` header. The enrolled
key's standing is asked of the enrolled hub under the key whose receiver has
the hub's origin, so a key given with `--api-key` for another receiver is not
sent to the hub. The other requests an enrolled edge makes to its hub under
the stored key follow the same rule: the reporting approval refresh, the
directory proof refresh outside a relay run and `disconnect` send the
`relay.json` key only where the receiver in `relay.json` has the origin of the
hub in `enrolment.json` (`EnrolmentRecord::hub_ingest_key`), and otherwise
report that no ingest key is held for the hub and send nothing. A dry run
sends and prints no key.

## Relay at session end

A local edge relays when a session ends, as well as when `commonmeasure
relay` is run. The `session-end` hook, after recording `session_ended`,
starts `commonmeasure relay` as a separate process when `relay.json` names a
receiver and the marker file `relay/manual` is absent, and starts nothing
otherwise. The run is a whole relay run: every session log and every due
spooled batch, because the MCP server a host starts writes its crossings
under its own session identifier and the hook's session holds only part of
the work. Clearance, reporting approvals, backoff and the spool lock apply as to any run.

The hook does not wait for the run and its exit code does not depend on it.
The child takes no standard input or output from the hook and runs in its
own process group, so the host neither waits for its output nor ends it with
the hook. A run that finds the spool lock held by another relay exits at
once, before it has asked the hub for a standing or written to the relay
directory (on a managed edge the command's policy sync has already run); what
it would have projected stays in the session logs, and the batches already
spooled stay there, for the next run. The child's own output is discarded:
its outcome is in `relay/receipts.json` and the spool, which `status` and
`doctor` report, `doctor` with the last delivery time and its age.

On Windows the child is started with `DETACHED_PROCESS`,
`CREATE_NEW_PROCESS_GROUP` and `CREATE_BREAKAWAY_FROM_JOB`, the last so a
host that runs its hooks in a job object configured to kill its processes
does not take the relay with it. A job that forbids breakaway fails the
spawn, so the child is then started without that flag and shares the job's
fate. This path is `planned`: it has not been run on Windows, and no test
here exercises it.

Only a host that sends a session-end event to the hook relays this way.
`install claude` registers the `SessionEnd` hook and the plugin's
`hooks.json` registers the same events; the other hosts' installs register
none (`docs/contracts/host-integration.md` §2), so an edge used only through
them relays when `commonmeasure relay` is run. A session that ends without
the event, a crash for example, relays at the next session end. Removing
`relay.json` stops it with every other relay, and writing `relay/manual`
stops it alone, which also leaves a licence's reporting demand unmet
([session evidence §Source declarations](session-evidence.md#source-declarations)). The hosted service honours the same marker: its
interval relay does not run while the marker is present, and each skipped
interval is a journal line naming the marker
(`crates/commonmeasure-cli/tests/hosted_service.rs`
`the_service_skips_its_interval_relay_while_the_manual_marker_is_present`).
Tested in
`crates/commonmeasure-cli/tests/hook_e2e.rs`:
`a_session_end_relays_in_the_background_without_the_hook_waiting` against a
loopback receiver that records the bytes posted, and
`a_session_end_starts_nothing_without_a_receiver_or_under_the_manual_marker`
for the two cases that start nothing. Both establish the process behaviour
on this platform, not delivery to a hub.

## Grounding from host observations

The relay emits one `content_grounded` per eligible context-entry observation,
with its representation hash and generation as `turn_id`. It derives clearance,
source identity, licence and the private-address/internal exclusions from the
referenced acquisition. Event IDs use the observation's original log position;
the owning engagement remains that of the acquisition. Missing token counts
remain absent. Retrieval of unused content emits only `content_retrieved`.
Several representations of one acquisition in a generation produce separate
grounding events. Their hashes describe the representations; the event count
measures representation occurrences. Receivers measuring distinct sources per
generation count distinct (`turn_id`, `content_url`) pairs within the session,
not grounding events. Event IDs remain the deduplication key for delivery retries.

## Directory reporting consent

After directory enrolment, the relay requires current local root permission
and, on a managed edge, its current signed reporting approval, alongside source-policy
clearance and the privacy floor. The check applies to new projection and queued
batches immediately before delivery. Opt-in includes existing eligible witnessed
evidence; opt-out preserves the original record and does not recall deliveries.
No field is added to Content Telemetry. See
[directory enrolment](directory-enrolment.md).

## The agent id on the wire

The relay initially takes `agent_id` from the session's first `edge_identity`
record, or uses `commonmeasure` when there is none. It pins that value per
wire session in `relay/session-agents.json` before the first delivery attempt,
so every later batch and retry keeps the same id even when the session resumes
or compacts after enrolment. Retained spool batches supply the first id for a
session with no pin: one queued by an earlier run and not yet delivered, or
one whose pin file was lost; a relay run with nothing to deliver keeps all
pins. The pin is private relay state and adds no wire field.

## The refused count on the wire

The relay never projects a refused crossing: no URL, no reason, no hash of
the bytes it withheld. What it projects, on every batch of a session it
delivers, is `refused`: the number of `crossing_refused` records in that
session cleared for egress as a crossing would be (§Selected coverage), as
an integer at the top of the batch document and nothing else about them.
The count exists so an organisation's owner can see that policy was
enforced across its edges, and it is a count of enforcement, not of
sources: a refusal to a private address or a named internal prefix is
counted like any other, because nothing about the address leaves.

The count is the session's running total at the time the batch is
projected, never a per-batch difference. Every batch of one session
carries the same value, a later batch for the same session carries the
later total, and a receiver keeps the larger value it has seen rather than
summing, so a redelivered batch changes nothing. When a session's total has
moved since a receiver last accepted a batch for it and there is no new
event to carry it, the relay sends the last event of the session's
projection again, under its own id, one the receiver already holds or that is
already spooled for it, on a batch carrying the new total: the receiver
takes the larger count, so a refusal after the
session's last admitted crossing still crosses. The relay keeps, per
session, the highest count a receiver has accepted, whatever the receiver
(`relay/refused-delivered.json`), and treats a count in a batch still in the
spool as sent. Its summary states the totals it queued for the wire this run and nothing
more. A session that was refused and admitted nothing produces no batch and
its count does not cross; a batch without the field comes from an edge that
does not report it, or was sent to a receiver scoped to suppliers
(§Supplier scope), which is not the same as a count of zero.
`conformance/session-refused.json` is the vector.

## Verification

`crates/commonmeasure-relay/tests/relay.rs` exercises log reading, scope
resolution, projection, durable spooling and HTTP delivery to a loopback
receiver. It checks minimal turn disclosure and stable IDs across policy
changes, selected coverage exclusions, and known, zero and unknown ingestion
counts for sessions and runs. These are fixture-tested inputs through the real
relay transport; they do not establish Hub or external receiver acceptance.
The same tests check that the instance reference reaches only the receiver
whose origin is its issuer, on content events only, at each source record's
revision, that an unregistered session's batch carries none, and that a batch
spooled for the issuer and delivered to another receiver is posted without
the member; the receivers there record what the relay posts and are not hubs.
The supplier scope tests there post to loopback receivers and assert on the
JSON received: a scoped receiver gets its supplier's retrieval and grounding
and no `refused` member, also when the override spells the configured URL
with a trailing slash, when the batch was spooled unscoped, and when a
directory selection's recheck has rewritten the count; a list that does not
parse sends nothing, `null` is unscoped and `[]` sends nothing. The URL
comparison and the parser's table are unit-tested in
`crates/commonmeasure-harness/src/relay_config.rs`.
`crates/commonmeasure-relay/tests/conformance.rs` validates the corpus against
the pinned schemas, including rejection of missing or invalid turn privacy
levels. A receiver must re-pin the changed corpus and run its own replay gate.
`crates/commonmeasure-relay/tests/paced_retrieval.rs` fetches a page twice
inside a loopback origin's `Crawl-delay` through the real `context_fetch`,
pacing store and transport, and projects the waited crossing: its retrieval
carries the delay kept and the `timestamp` the crossing was recorded with,
which is after the page answered.
