---
title: Session evidence contract
---

# Session evidence contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This contract defines what a harness session records. The batch runner's
equivalent is [`docs/contracts/run-output.md`](run-output.md). The two are different shapes
because a run is one bounded comparison and a session is an open-ended
period of somebody's work. Terms like crossing,
engagement, scope and gap are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).

## Where

`$COMMONMEASURE_HOME/sessions/<session-id>.ndjson`, or
`~/.commonmeasure/sessions/<session-id>.ndjson`.

A record appended while a registration is retained for the session carries
one optional `instance` member: `issuer`, the registration service
identifier; `id`, its instance identifier; and `revision`, the immutable
authority binding held when the record was written (§Instance registration).
The reference resolves to the retained binding defined by the
[instance registration contract](instance-registration.md#session-evidence-reference).
It leaves `session_id`, `host` and the edge identity unchanged. A shared
session may carry records from different instances. Absence means no recorded
registration reference; it does not invalidate earlier records or establish
authority. The reference proves neither compliance nor fulfilment. It is
private to the operator and the registration service that issued it. The
relay sends it to one receiver only: the operator's own hub, where that hub is
the reference's issuer, as an event-level `instance` member on content events
(§Content Telemetry projection, §Instance reference). The hub removes the
member before onward delivery, so it reaches no publisher, and it adds no
Content Telemetry or SPUR field to what a publisher receives. Emission is
built; a reader that verifies the reference against the retained binding is
planned.

A hook takes the session identifier from the host's payload. The MCP server
takes the first of:

1. `--session`, which the host's registration passes;
2. `AGENT_SESSION_ID` in the server's environment, which Goose sets for
   every server it starts, so its records share one log per Goose session;
3. an identifier the server mints, `local-<milliseconds>-<process id>`.

The identifier names a file, so it must be a plain name: ASCII letters,
digits, `.`, `_` and `-`, one to 128 characters, not starting with `.`. Any
other identifier is refused, never cleaned: a hook given one records nothing
and exits zero, and the MCP server given one fails to start, naming where
the identifier came from (`crates/commonmeasure-harness/src/session.rs`
`safe_session`).

A **hosted session** ([`docs/contracts/host-integration.md`](host-integration.md)
§1, the hosted path) is one protocol session over HTTP. The edge mints its
identifier at `initialize`, `hosted-<milliseconds>-<128 random bits in hex>`,
and the same value is the `Mcp-Session-Id` header on every request and the
`session_id` on every record, so a request in a web server's log and a
record in the file are joined without a lookup table. The file is opened at
the session's first request that is not `initialize`,
`notifications/initialized`, `tools/list` or `ping`; a session that is asked
for nothing else leaves no file. On the enrolled edge a hosted session runs
on, the file's first records are `policy_sync` (trigger `server_start`,
where the edge is managed) and `edge_identity`, written at open, before any
crossing (`crates/commonmeasure-cli/src/mcp_session.rs`).

One file per session, opened for append. A session spans many short-lived hook
processes, so `seq` resumes from what is already in the file. Under concurrent
hooks two records can share a `seq`; a reader orders by position and treats
`seq` as a hint. Every append is fsynced before it returns, and a write failure
is materialised as an `evidence_gap` record by the next successful write **on
the same log instance** ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §2).

That scope matters for a session in a way it does not for a run. The owed gap
is held in memory by the log object that failed the write, so it can only be
written by a later append through that same object. A hook process is one
short-lived log per invocation, commonly one append, so a hook whose append
fails leaves no gap record and no later process knows it happened.
Session-log completeness is therefore per-process: the log records every
window that a process which survived to write again could describe, and
records nothing about a process that did not.

## Events

Each event is defined by what the host supplies. The Claude Code hook that
supplies it today is given as the example; what every host must provide is
in [`docs/contracts/host-integration.md`](host-integration.md) §2.

| Event | The host supplies | Meaning |
|---|---|---|
| `crossing_observed` | a hook after each tool call carrying the tool's input and response (Claude Code: `PostToolUse`) | the agent read this, and a hook saw it afterwards |
| `crossing_reconstructed` | a transcript that records tool results, read by `commonmeasure import` | read back from a host transcript, for work done before Common Measure was installed |
| `crossing_mediated` | the agent's fetch or search routed through the MCP server | Common Measure carried this, under policy |
| `crossing_refused` | the same | policy refused this before it could enter the agent's context |
| `processor_invoked` | the same | one processor invocation ([`docs/contracts/processor.md`](processor.md)), recorded beside the crossing it judged |
| `prompt_sources` | a hook at prompt submission carrying the prompt text (Claude Code: `UserPromptSubmit`) | which URLs the prompt named, as hashes, and the statements pasted material carried; no prompt text |
| `manifest_resolved` | nothing; the MCP server writes it after a mediated fetch | what Content Telemetry discovery established for the host: the URLs probed, the status, the cache decision, and the manifest's facts or the rejection; never a crossing |
| `turn_started` | a hook at prompt submission (Claude Code: `UserPromptSubmit`) | a turn boundary, carrying the host's turn identifier, a declared privacy level and the policy identity (§Turn boundaries) |
| `turn_completed` | a hook at the end of a turn (Claude Code: `Stop`) | a turn boundary, the same |
| `context_snapshot` | a transcript carrying the provider's usage counters and the host's own records of what it assembled, read at the end-of-turn hook (Claude Code: `Stop`) | a context-budget observation at the boundary, with its basis named, and the inventory of the window by category (§Context snapshots) |
| `nudge_issued` | a hook at session start whose stdout the host adds to context (Claude Code: `SessionStart`) | the standing mediation nudge was emitted for the host to add to the session's context |
| `edge_identity` | a hook at session start (Claude Code: `SessionStart`), on an enrolled edge | the identity this edge runs under: the hub, the key id the hub assigned at enrolment, and the key's standing (`enrolled`, or `revoked` with when and by which side), and whether the hub's key directory lists the key, as the edge last learnt it: `listed_until`, the time the hub stops serving the key's directory proof (its expiry less the 7,200-second margin), or `unlisted` with the reason. The edge keeps what it last learnt in `<home>/directory-listing.json`, bound to the key id, and never in `enrolment.json`, whose shape stays the one every released binary reads. A signed request under a key the directory does not list verifies nowhere. Absent on an edge that is not enrolled |
| `credentials_loaded` | nothing; the MCP server writes it before the first record a tool call or host observation leaves | the operator credentials file was loaded at server start: its path, digest and variable names, never a value |
| `policy_sync` | a hook at session start (Claude Code: `SessionStart`), or the MCP server's start for a session no hook refreshed | on a managed edge, what refreshing the policy from the hub did for this session: the outcome, the revision in force and whether its envelope has expired (§Policy synchronisation). Absent on a local edge |
| `host_process` | a hook at session start (Claude Code: `SessionStart`), and the MCP server's start | the host process this path runs under, so the hook log and the MCP log of one host session can be joined (§Host process) |
| `session_ended` | a hook at session end (Claude Code: `SessionEnd`) | the host reported the session ending, with the reason word the host gave (§Host process) |
| `client_identified` | the MCP client's `initialize` request; the MCP server writes it before the first record a tool call or host observation leaves | how the program on the other end of the stdio pipe named itself: `clientInfo.name` and `clientInfo.version`, with the protocol version it asked for and the one the server answered with (§Client identity) |
| `instance_refused` | nothing; the MCP server writes it | a mediated fetch or search was stopped before anything crossed because the session's registered instance could not be shown to hold authority: `refused` where the authority is known to have ended, `unavailable` where a required check could not be made (§Instance registration); never a crossing |
| `allowance_gap` | nothing; the MCP server writes it | the allowance ledger did not record a settlement or release for a mediated search: the reservation id, the observed charge where a receipt reported one, and the reason; the reservation stays held until the expiry sweep releases it |
| `evidence_gap` | nothing; the log writes it | a window this log could not record |

A host-policy refusal happens before any bytes move. A processor refusal,
which is what the PII detector and the injection screen produce, happens
after the fetch and before the text is returned: the bytes existed, their hash
is on the refused crossing, and they never entered context (`grounded` is
false). The two records differ because they are different facts, and both
are `crossing_refused` because in both cases the mediated path stopped the
text before it reached the model. The tool error the agent sees says which:
`refused before the crossing` for the first, `refused before the content
entered the context` for the second.

A crossing whose authority is the enrolled hub's is refused before any bytes
move, as a host-policy refusal is, and by the runtime rather than by the
operator's policy: the authority of the enrolment's `hub`, of its
`identity.origin`, or of a managed deployment's `policy_url`, compared by host
and by the port a request would connect to. The mediated path signs each hop
with the enrolled key, which the hub's own routes accept. `refusal` carries
one fixed sentence and `url` names the hop that was refused. No policy setting
lifts it. A `robots.txt` or licence probe to such an authority is not sent,
and the declarations record says why. A `robots.txt` that redirects to it is
unreachable (§Source declarations).

## Crossing

The mode decides which fields a record can carry, so there is one example per
mode. A single composite example would show a record no path writes: the
identity fields belong to the mediated path, `derived_from` to the
reconstructed path, and a reader checking a real log against the composite
would be checking it against a shape that cannot exist.

A **mediated** crossing carries the most, because Common Measure carried it and
could have refused it:

```json
{
  "session_id": "…", "timestamp": "…", "mode": "mediated",
  "host": "codex", "client": {"name": "codex-mcp-client", "version": "0.154.0", "title": "Codex"},
  "cwd": "/home/operator/code/project", "policy_scope": "code/project",
  "principal": "research-agent", "authentication_basis": "os_user",
  "url": "…", "host_name": "…",
  "identity": {"user_agent": "CommonMeasureBot/0.2.0 (+https://…/bot; mailto:…)",
               "key_id": "…", "signature_agent": "https://…"},
  "content_hash": "sha256:…", "retrieved_hash": "sha256:…",
  "estimated_tokens": 12, "token_basis": "characters/4",
  "grounded": true, "licence": {"state": "unknown"},
  "policy_identity": "sha256:…"
}
```

`retrieved_hash` is SHA-256 over the response body as the origin served it,
and appears on a mediated fetch that received a body and on no other record.
`content_hash` beside it covers the text delivered to the agent or withheld
from it, which for an HTML page is the page's readable text and not its
markup ([`docs/contracts/processor.md`](processor.md) §Status, `html-text-extractor`). An
origin that serves the body under the gzip content coding, which some do
whatever the request accepts, has `retrieved_hash` over the coded bytes it
served and `content_hash` over the text taken from what they decode to. The
two are equal only where the body was served with no content coding and
delivered as decoded. A body under any other content coding, or one that
does not decode, delivers nothing: the crossing carries the `failure` that
names the cause and neither hash. Where the two hashes differ,
the `processor_invoked` record of the extractor written before the crossing
carries `retrieved_hash` as its input hash and `content_hash` as its output
hash, and a reader re-derives the second from the bytes the first names by
applying the rules the record's configuration digest pins
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §12). `estimated_tokens` counts the delivered text.
The relay projects `content_hash` and never `retrieved_hash`: what a
receiver learns is the hash of what entered context.

`identity` is the network identity the request presented, and it appears on
every mediated crossing whose request left the machine and on no other: a
crossing refused before the request presented nothing, and a search result
was fetched by a supplier under its own contract rather than by this
runtime. An enrolled edge carries `key_id` and the `signature_agent`
directory a verifier resolves it in; an edge that is not enrolled, or whose
key the hub has revoked, carries `unsigned` instead with the reason in it.
The two are different facts, not a value and its absence: a publisher can
verify the first and can only take the second on trust.

`supplier` appears on a mediated search result and on no other record: the
provider name that served it (`exa`, `ozone`, `internal`, `skill:<name>`),
as policy and plans use it. A fetch has none, because this runtime made the
request; an observed or reconstructed crossing has none, because the host's
tool was not a supplier. The relay projects the name for suppliers and
withholds it for the operator's own corpus and for skills
(`crates/commonmeasure-relay/src/project.rs`).

`challenge` appears where the origin refused the fetcher rather than the
resource — a challenge marker such as `cf-mitigated`, or a status that
refuses the request itself. It is deliberately distinct from `refusal`,
which is this operator's policy, and from `failure`, which is the
transport. All three leave a crossing with no content hash, and a reader has
to be able to tell whose decision it was: the operator's, the origin's, or
nobody's.

An **observed** crossing names the host tool that made it and nothing about
identity or policy: a hook saw it after it happened.

```json
{
  "session_id": "…", "timestamp": "…", "mode": "observed",
  "host": "claude-code", "tool": "WebFetch",
  "agent_type": "Explore", "agent_id": "…", "turn_id": "…",
  "cwd": "/home/operator/code/project",
  "url": "…", "host_name": "…",
  "content_hash": "sha256:…", "estimated_tokens": 12, "token_basis": "characters/4",
  "grounded": true, "licence": {"state": "unknown"}
}
```

A **reconstructed** crossing names the transcript it was read from and no
working directory:

```json
{
  "session_id": "…", "timestamp": "…", "mode": "reconstructed",
  "host": "claude-code", "tool": "WebFetch",
  "url": "…", "host_name": "…",
  "content_hash": "sha256:…", "estimated_tokens": 12, "token_basis": "characters/4",
  "grounded": true, "licence": {"state": "unknown"},
  "derived_from": "~/.claude/projects/…/<session>.jsonl"
}
```

`refusal` and `breach` appear only when policy produced one, which only the
mediated path can. Otherwise they are absent rather than null, as every
optional field here is: a reader can tell "policy refused nothing" from "no
policy was watching" by which mode the record is in, not by a null.

`mode` is recorded, never inferred from the code path that wrote the row. The
three are not the same grade of evidence, and nothing merges them:

- **mediated** — governed before it happened, and could have been refused;
- **observed** — seen by a hook after it already had;
- **reconstructed** — read back from a host transcript afterwards, with nothing
  watching at the time. The weakest grade, and not called observed because a
  transcript is written by the host for its own purposes and can be compacted
  or truncated, so a hash taken from one is a claim about the transcript and
  not about what entered the model's context. `derived_from` names the file.

### Embedded credentials on acquisition

Before text transformation, `html-text-extractor` version 2 verifies supported
C2PA credentials against the unmodified body after removal of content coding.
It reads Annex A.7 inline manifests in UTF-8 HTML and Annex A.8 wrappers in
other UTF-8 `text/*` bodies. Verification runs before HTML markup or a
recovered text wrapper is removed. Invalid and untrusted recovered wrappers
are also removed; unsupported or incomplete wrappers stay in the text.

The `processor_invoked` record's `detail.embedded_credential` contains:

| Field | Meaning |
|---|---|
| `state` | `absent`, `invalid`, `untrusted` or `unavailable` |
| `credential_ref` | SHA-256 of the embedded manifest store; the declared link for an external manifest; otherwise null |
| `manifest_label` | The SDK's active manifest label, or null where no manifest could be read |
| `method` | `annex-a8`, `annex-a7-script`, `annex-a7-link` or `annex-a7`; null where the content type or encoding is unsupported |
| `verifier`, `verifier_version` | C2PA SDK identity and version |
| `validation_status` | SDK status codes, URLs and explanations; empty where no SDK reading completed |
| `trust_list`, `reason` | The trust configuration and reason for the result |

`untrusted` means the signature and content binding verify but no signer trust
list is configured. The current verifier cannot issue `trusted`. `invalid`
means the wrapper or manifest is malformed, or verification failed, including
when the signed content was altered. `unavailable` names the missing reader,
encoding support or external-manifest retrieval capability and adds a
`capability_unavailable` gap. Verification is offline; external manifest
links are recorded without being fetched. `absent` is limited to supported
text bodies in which no supported credential was found.

The same invocation carries `retrieved_hash` as its input, `content_hash` as
its output, the transformation's processor name, version and configuration
digest, and `detail.credential_wrapper_removed` for Annex A.8 removal.
`detail.extracted` describes HTML extraction. Both hashes and the verification
result remain recorded when a later admission screen withholds the text.
The crossing repeats the hashes; matching them to the preceding transform
record ties the observation to that acquisition without a sequence-number
join. The raw credential and source text are not copied into the source
record. Verification establishes no factual accuracy or licensing authority
and changes no admission rule. These private processor details are not
projected to Content Telemetry.

## Acquisition handles

The controlled Pi runner opts into explicit host observations before any mediated
crossing (including a refusal). Earlier observed hook crossings are allowed.
It opts in by sending the MCP JSON-RPC request `commonmeasure/observe` with
`{"event":"observations_started"}`. This is a host integration method, outside
`tools/list`; it is not a model tool. Its acknowledgement follows the durable
append. The method accepts only the records below and rejects unknown fields.
CM supplies the session, host, timestamp and working directory. The caller
cannot assert a different policy, source URL, observer or grade. The first
observation request records available credential provenance and supplied client
identity through the same session-start path used by tools.

Opening a log discovers opt-in from complete, parseable lines, tolerating partial
or damaged lines so hooks can continue appending and the MCP server can start.
It parses only a line whose bytes hold the event name `observations_started`,
which the writer never escapes, so opening a session that never opted in parses
nothing. It preserves existing bytes; it does not repair a damaged log.

Observation validation reads the durable file strictly and returns unavailable
on a malformed record or an incomplete last line. A concurrent incomplete
append may therefore make an observation unavailable; the controlled runner
stops on that failure. Each request reads the lines appended since the previous
one and checks the request against a validation index of the earlier ones: the
handles, generation, output and action identifiers, hashes and byte offsets the
checks need, and no URL or text. The index is derived from the log and is not
evidence. The writer holds it in memory and journals it to
`<home>/observation-index/<session-id>.ndjson`, so a restarted server resumes
from it. Each journal entry names the byte range of the log it covers and a
SHA-256 of the 4,096 log bytes before the end of that range. A journal that is
missing, malformed, discontinuous, of another version or about another log is
discarded and the index is rebuilt from the log; a journal that cannot be
written changes no answer. Only a turn identifier that is a canonical UUID is
indexed, so a host's own turn identifier never reaches the journal. A journal
loaded with more than 1,024 entries is replaced by one entry at the next
request. A malformed record stops the index at that record, so later requests
stay unavailable. The index does not reread bytes it has indexed: damage to
them, outside the digest's 4,096 bytes, is found by a strict read of the log
(`commonmeasure session`), not by a later observation request.

The journal is not authenticated: once its range and digest match the log its
facts are taken as given, and anyone who can read the log can compute the
digest. That is the log's own trust limit and no new boundary, because the log
is unauthenticated and sits beside the journal under the same user, so whoever
can write the journal can append the same claim to the log. Both are protected
by the operator home's file permissions, and the journal and its directory are
created owner-only. A forged log line stays in the evidence; a forged journal
fact leaves no record of itself.

Explicit observations cover `context_fetch` only. `context_search` deliveries
return no acquisition handle and retain their existing crossing semantics; they
are not stamped `host_required` and cannot receive a context-entry observation.
The controlled Pi runner exposes `context_fetch`, and `send_acquired_text` only
when the operator supplies action grants (§Host action decisions).

In this mode each admitted text fetch carries an `acquisition_id` (UUID) and a
separate `crossing_id` (UUID) on its `crossing_mediated` record. `observer` is
`cm`, `grade` is `mediated`, and `context_observation` is `host_required`.
The fetch response returns the acquisition handle only after its record has
been fsynced. Failure to record it makes the fetch unavailable. Refusals and
failed fetches yield no handle. The handle identifies the admitted bytes named
by `content_hash`; it is neither supplier-issued delivery identity nor a licence.
A repeated acquisition receives a new handle, even for identical bytes.

`grounded` is false on these crossings. Retrieval establishes no context entry.
Older sessions and integrations which have not opted in retain their existing
crossing semantics. The handle remains usable by later generations in the same
session. Cross-session references and unknown handles cannot establish use.

## Context-entry observations

`commonmeasure/observe` accepts:

```json
{"event":"context_entered","acquisition_id":"<UUID>",
 "generation_id":"<UUID>","representation_hash":"sha256:<64 hex digits>"}
```

CM appends `context_entered` with `observer: host`, `grade: observed`, and the
host identity of the MCP process. It checks that the handle belongs to an
admitted acquisition in this session. The representation hash identifies the
bytes the host observed in its serialised model request, including any host
transformation; the acquisition's hash remains unchanged. No text is accepted.
The Pi runner records this after its guarded HTTP transport receives a response.
It establishes the host's request boundary, not provider-internal context or
model execution. Transport failure can leave an attempted request without a
context-entry observation. History reuse records a new generation against the
same handle, without another retrieval. Duplicate observations for the same
handle, generation and representation are refused.

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

## Output-association observations

For each completed assistant message the host sends:

```json
{"event":"output_associated","generation_id":"<UUID>",
 "output_id":"<UUID>","output_hash":"sha256:<64 hex digits>",
 "acquisition_ids":["<UUID>"]}
```

The record names `observer: host` and `grade: observed`. It records an explicit
reference or quotation association under the host's convention, never semantic
support. The hash identifies the output; output text and quotation text are
not accepted. An empty list records that the observed output had no recognised
association. Unknown handles are retained as unresolved references and never
count as source use. Repeated markers count as repeated associations. A known
handle absent from that generation's observed context is also unresolved for
projection. At most 1024 association references are accepted per output.

The output observation and completion boundary are separate durable appends.
If the first succeeds and the second fails, the call returns unavailable.
An identical retry from the same host, with the same generation, output ID,
hash and ordered association list, appends only the missing boundary. A changed
retry or an output whose generation already has a completion boundary is refused.
A malformed durable log remains unavailable; retry does not repair it. The Pi
runner still stops on recording failure and does not implement this recovery.

The ingress also writes a minimal `turn_completed` boundary with the generation
as its host turn identifier and a reference to this output observation. This
boundary means the generation completed; it does not assert that the user's
whole task ended. This host-observation boundary carries `observer`, `grade`,
`generation_id`, `output_id`, minimal privacy fields, `detail.cwd` and the MCP
process's `policy_identity` when set. It does not carry the hook boundary's
policy-resolution fields or transcript path. Clearance uses its working
directory independently of the acquisition. A retry uses the completing
process's working directory and effective policy identity.
Its custom event field
`data.commonmeasure-output-associations.count` counts only associations whose
acquisitions and context entries pass selected coverage and privacy checks.
It is distinct from the number of `content_grounded` events. A receiver counts
this field once per boundary event ID. The field is absent on boundaries without
an output observation, so missing observation never becomes measured zero.
The hash, local handles and output ID stay in the private source record.

This extension remains Grounding-level Content Telemetry. It makes no standard
`content_cited` event or Citation-conformance claim. The separately appended
boundary lets a later output be projected after its grounding was already
spooled, without changing a previously delivered event's count or identity.
The boundary needs its own clearance and at least one eligible source event,
as §Selected coverage requires.

## Host action decisions

The host records its decision at the action boundary through
`commonmeasure/observe`, before executing an allowed action or returning a
refusal to the model:

```json
{"event":"action_decided","action_id":"<UUID>",
 "generation_id":"<UUID>","acquisition_id":"<UUID>",
 "action":"send_acquired_text","decision":"refused",
 "rule_id":"recipient_not_authorised",
 "target":{"url":"https://shared.example/inbox/other","recipient":"other"},
 "data_hash":"sha256:<64 hex digits>",
 "host_policy_hash":"sha256:<64 hex digits>"}
```

This first action kind sends exactly the acquired text. CM validates that the
acquisition was admitted in this session, its content hash equals `data_hash`,
and its handle has a context-entry observation for this generation. It rejects
duplicate action IDs; an action has no identical-retry path. `decision` is
`allowed` or `refused`; `rule_id` names the host rule applied. CM supplies
`observer: host`, `grade: observed`, host, session
and timestamp. The host cannot supply those fields or source-policy identity.
The acquisition retains CM's admission, effective source policy and applicable
licence/declaration conditions; the action record does not replace them or turn
them into permission to disclose. The observation is the host's assertion, not
CM's independent verification of its decision or its enforcement.

An action decision refers to the generation whose output proposed the action,
so it may follow that generation's `output_associated` and `turn_completed`
records. A context entry is refused once its generation has an output; an
action decision is not. In the controlled Pi loop every action record follows
its generation's completion boundary, because the tool call that proposes the
action ends the assistant message and the runner records that output before it
executes the tool.

`host_policy_hash` identifies the host's action policy, separately from CM's
source policy. `target` identifies both the destination resource URL and the
recipient. The URL must be absolute HTTP(S), at most 2048 bytes, without user
information, query or fragment. Its authority and path come from the host and
are bounded only by that length, as a crossing's URL is; CM cannot tell a
resource path from text placed in one, so hosts must not put credentials or
private text there. Recipient and rule identifiers are 1–128 ASCII letters, digits, dots,
underscores or hyphens. Both hashes use the SHA-256 form above. Unknown fields,
other action kinds and other decisions are rejected. The record has no field for
a prompt, acquired text, file contents or action arguments; the URL path is its
only free-form value.

An acknowledgement follows the fsynced append. Failure makes the action
unavailable: the controlled runner stops before sending. An `allowed` decision
records permission only; it does not establish transmission, recipient receipt,
subsequent access or fulfilment of licence conditions. A missing decision does
not establish refusal. Action records stay in the private source record; the
relay projects no event, boundary, source-use count or refused-acquisition count
from them, irrespective of source or session clearance.

The controlled Pi fixture uses an operator-supplied grant for an exact source
URL, destination URL and recipient, with redirects refused. The model may
propose an action but cannot extend that grant. Tests admit useful synthetic
content with an attribution condition and an embedded disclosure instruction,
observe its context entry, and exercise refusal of another origin and of an
unauthorised recipient or resource on an allowed shared host. An action of the
same shape to the authorised recipient and resource succeeds. These claims are
**fixture-tested**, with synthetic model responses and loopback HTTP captures.
They establish neither live model resistance nor process-wide egress control.
The host records `recipient_use` as unavailable because subsequent access,
retention and use at the recipient are outside its observation.

## Unavailable host observations

The ingress accepts `{"event":"observation_unavailable","observation":"…"}`
for the fixed categories `provider_context`, `provider_cache`,
`supplier_delivery_identity`, `semantic_support`, `process_egress` and
`recipient_use`.
CM appends the named gap with `observer: host`, `grade: unavailable` and a fixed
explanation. These are unavailable observations, never zero measurements and
never telemetry events. Absence of a later context or output record alone
establishes no negative claim: the host or process may have failed before it
could record one. Partial assistant outputs remain in the synthetic probe's
private journal and are not counted as completed output associations.

The controlled runner and projection are **fixture-tested**: the runner's own
probe tests (not published) and the projection tests in
`crates/commonmeasure-relay/src/project.rs`.
The content and model responses are synthetic. Supplier authorisation,
provider-internal events, independent receiver acceptance and delivery recovery
are separate evidence requirements.

## Importing history

```sh
commonmeasure import --dry-run          # what would be read, per host
commonmeasure import --since 2026-06-01
```

Only what a transcript evidences is imported. A `WebFetch` with a
recorded result becomes one grounded crossing; a `WebSearch` becomes one
ungrounded crossing per result link; a call the transcript never shows
completing becomes nothing.

A host whose history evidences no crossings is reported as **not importable**,
with the reason, rather than yielding an empty result. Codex reaches the
network by shelling out: its transcripts hold `exec` command text, and a URL
inside a command is not evidence that anything was retrieved. Importing those
would fabricate crossings, and the purpose of this store is that its
crossings are trustworthy. Pi has no web tools of its own; a mediated tool
result in its history was recorded when it happened, and a transcript copy
would be a weaker duplicate of an existing record.

`grounded` is true only when page text entered the model's context. `WebFetch`
grounds. `WebSearch` and third-party MCP results do not: the model saw titles
and snippets, and nothing ties a returned URL to bytes that entered context.
Asserting grounding there would put a citation in the record with nothing
behind it.

`content_hash` is SHA-256 over the ingested bytes as they stand rather than
over a JSON document, so the canonical form
([`docs/contracts/canonical-json.md`](canonical-json.md)) does not reach it. It covers the text as
ingested, which for a Claude Code `WebFetch` is the tool result's `result`
field and **not** the surrounding `{"bytes": …, "code": …}` wrapper. The model's context and the transcript carry
only `result`; a hash over the wrapper can never be matched against either, so
it would break any later citation check.

`estimated_tokens` always travels with `token_basis`. Four characters to a
token is an approximation, and a number labelled "tokens" that no tokeniser
produced claims a precision it does not have.

`agent_type` and `agent_id` appear only when the crossing happened inside a
subagent. They are the host's own discriminators, read from a hook payload or a
transcript; a main-agent call carries neither, and neither is inferred from
payload shape. A mediated crossing also carries neither: the MCP server is
called the same way from a subagent and from the main agent, and the host tells
it nothing that would distinguish them.

`turn_id` is the host's own identifier for the turn the crossing happened
in (Claude Code sends `prompt_id` on every hook payload; Codex sends
`turn_id`), carried so a reader can group crossings under the turn boundary
that shares it (§Turn boundaries). It appears only on observed crossings: the
MCP server is told nothing about turns, and a transcript's turn structure is
not read back on import.

`principal` and `authentication_basis` appear only on mediated crossings. They
name the identity resolved before admission and the basis that authenticated
it. Observed and reconstructed capture happened outside that enforcement path
and claims neither, even if the host payload contains an agent label.

`authentication_basis` is one of four values. `os_user` is where the runtime
read the process's effective operating-system user, and `unavailable` where
the platform offers no identity it can read (currently every non-unix
platform, including the Windows binary the release publishes). `unavailable`
is recorded rather than omitted, and it is not a downgrade in enforcement: a
policy file declaring principals fails closed there, refusing every mediated
crossing and saying so, and a policy file declaring none is governed by
directory scope alone (trusted principal resolution,
[`docs/GLOSSARY.md`](../GLOSSARY.md) §Principal). The other two are the
hosted edge's ([`docs/contracts/host-integration.md`](host-integration.md)
§1, the bearer token): `oauth_subject` where a hub-issued access token
verified, with `principal` the `sub` claim as `subject:<sub>` unless a
policy binding keyed `subject` names a label for it; and `edge_token` where
a token the edge issued verified, with `principal` `token:<label>` unless a
binding keyed `edge_token` names a label
([`docs/contracts/source-policy.md`](source-policy.md) §Scopes and
principals). A hosted session carries `cwd` only when the operator explicitly
declares its service directory (§Hosted scope).

`licence.state` is `unknown` on observed and reconstructed crossings: no host
tool reports rights, and reaching a page is not permission. A mediated fetch
records what the source declared (§Source declarations).

`cwd` is the directory the session was working in when the crossing happened.
It is recorded on the crossing itself, not only on turn boundaries, because
sessions cross worktrees. It is a fact only: which engagement that directory
is *reported* under is resolved at read time in the sink from
`$COMMONMEASURE_HOME/attribution.json` (ordered substring rules over the cwd,
first match wins, `unattributed` otherwise). Editing a
rule re-attributes history without rewriting any evidence. No engagement
label enters this log. Observed and mediated crossings carry a `cwd`;
reconstructed ones do not, because a directory read back from a transcript
would be a claim, not a witnessed fact. A hosted session records none by default. With an explicit service
`session_directory`, `cwd` instead names that operator-declared scope; the
`hosted_scope` record distinguishes its basis from a client working directory.
The process's own directory is never inferred. Older logs without the field
read as absent and gain no reporting permission from new configuration.

`policy_scope` names the scoped policy overlay (`scopes` in `policy.json`,
matched against the cwd when the server started, first match wins) that
governed a mediated crossing; a record enforced under a policy that cannot be
named cannot be audited. It is absent when the top-level policy applied, and
always absent on observed and reconstructed crossings, because nothing
governed those before they happened. Policy is resolved at capture time and
attribution at read time, the opposite of each other, because a strict rule
for one engagement and observe for another cannot be a projection: refusal
happens at the crossing.

The same matched scope may bind its cwd to a named `engagement` (the
**governing** engagement) and set `allow_telemetry_egress` explicitly. This is
read-time relay policy, not evidence added to a crossing: the relay resolves
each crossing's recorded `cwd` immediately before projection. Absence, no
match, no engagement name, and an omitted or false clearance all mean no
egress. A malformed policy makes the relay unavailable before projection; it
never becomes an unfiltered successful mode.

The governing engagement and the reported engagement are two identities: the
governing engagement is declared in `policy.json` and is the only one an
enforcing component can read (`Attribution` is in `commonmeasure-console`, above both the
relay and the harness); the reported engagement is the attribution projection
and is what every console figure counts under. Neither derives from the other
and neither enters this log. They are expected to agree; nothing enforces it,
and the console, the only reader of both, states where they differ and
prefers neither.

`refusal` is set only on `crossing_refused`, which only a mediated crossing can
be.

`policy_identity` is set on every mediated crossing and on every refusal,
and on no observed or reconstructed crossing, for the reason `principal`
is: only the mediated path met a policy. It is defined in §Policy identity.

`breach` is set on a mediated crossing that broke a constraint the operator's
mode chose to carry rather than refuse. The mode decides what happens to a
breach, never whether it is recorded ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md)): under `observe` or
`prefer` the crossing goes ahead with the broken constraint named here, where
`strict` would have refused it. A carried PII finding is a breach like any
other and is recorded in the same field. Under `strict` a PII finding
refuses a crossing only where the source is internal or private: a named
internal prefix, a loopback or private address reached under
`allow_private_hosts`, or the operator's own corpus. On a public source the
finding is recorded in `breach` and the crossing is admitted, because the
published contact details of a public page are not the personal data the
detector exists to keep out of a model, and the finding, with its
categories and offsets, is the evidence a reviewer needs. `refuse_on_pii:
true` in the policy makes `strict` refuse a finding on every source
([`docs/contracts/source-policy.md`](source-policy.md) §Recording). Observed and reconstructed crossings
never carry one, because nothing judged them before they happened; for the
same reason the PII detector scans only crossings the mediated path carries,
and its manifest names that blind spot.

## Source declarations

A mediated fetch reads what the source declares about AI use and records it
on the crossing, with three more fields:

- `http_status` — the status the final URL answered with. Absent for a search
  result, for a transport failure and for a refusal made before any request.
- `failure` — the client's account of a transport failure, when the request
  left the machine and nothing usable answered.
- `declarations` — what was read, from where, and what it adds up to.

The reader (`crates/commonmeasure-harness/src/declarations.rs`, the cache and
record in `crates/commonmeasure-harness/src/discovery.rs`) gathers
**statements**, each a source, a category (`train-ai`, `ai-input`,
`ai-index`, `search`), a preference (`allow` or `disallow`) and the
mechanism's own label. The sources are the page response's `Content-Usage`
header (`content-usage-header`), a `Content-Usage` rule in the selected
`robots.txt` group (`robots-content-usage`), a `Content-Signal` line in that
group or, where it has none, one outside any group (`robots-content-signal`),
and an RSL licence's usage terms (`rsl-licence`). The `CommonMeasureBot` group
is selected where the file names it and the `*` group otherwise; the record
names which. A `License:` or `Content-Signal` line belongs to the group whose
lines it follows without a blank line or a `Sitemap:` line between; after such
a break, or before any group, it is site-wide. Statements combine
most-restrictive-wins per category into `effective`; a category no statement
addresses is `unknown`, never `disallow`.

```json
"declarations": {
  "robots": {"requested_url": "https://publisher.example/articles/1",
             "url": "https://publisher.example/robots.txt",
             "final_url": "https://publisher.example/robots.txt", "cache": "fetched",
             "fetched_at": "…", "expires_at": "…", "status": 200,
             "reading": {"group": "CommonMeasureBot", "group_is_wildcard": false, "crawlable": true,
                         "access_rule": "Allow: /", "access_rule_wildcard": false,
                         "statements": [], "licences": ["https://publisher.example/license.xml"],
                         "crawl_delay": {"value": "2", "delay_ms": 2000, "honoured_ms": 2000, "capped": false}},
             "mode": "strict", "outcome": "allowed",
             "delay": {"host": "publisher.example", "delay_ms": 2000, "outcome": "waited",
                       "wait_ms": 1732, "budget_ms": 60000}},
  "redirects": [{"requested_url": "https://short.example/abc",
                 "url": "https://short.example/robots.txt", "cache": "fetched",
                 "fetched_at": "…", "expires_at": "…", "status": 200,
                 "reading": {"group": "*", "group_is_wildcard": true, "crawlable": true,
                             "access_rule": "Allow: /", "access_rule_wildcard": false,
                             "statements": [], "licences": []},
                 "mode": "strict", "outcome": "allowed"}],
  "licences": [{"url": "https://publisher.example/license.xml", "mechanism": "robots-license",
                "cache": "fetched", "status": 200, "content": "/",
                "terms": {"statements": [], "payment": {"kind": "use", "amount": {"currency": "USD", "decimal": "0.015"}},
                          "reporting": [{"kind": "telemetry", "profile": "https://contenttelemetry.org/profiles/spur",
                                         "endpoint": "…", "config": {"conformance_level": "grounding"}}]}}],
  "content_usage_header": "train-ai=n",
  "statements": [{"source": "content-usage-header", "category": "train-ai", "preference": "disallow", "detail": "Content-Usage: train-ai=n"}],
  "effective": {"train-ai": "disallow", "ai-input": "allow", "ai-index": "unknown", "search": "unknown"},
  "governing": "statements"
}
```

`robots.txt` and the licence it names are read before the request, so a
statement they carry is ruled on before any bytes move; the header and a
`Link: rel="license"` licence are read from the response, so a statement they
carry is ruled on after, and the bytes are withheld from context until it
has been read. The host's cache file keeps, per page URL, the licence the
page's `Link` header named when it was last fetched (`page_licences`), so a
later crossing to a host that states a delay reads it before the page. `robots.txt` is fetched once per host and cached for
24 hours, or for the response's `max-age` where shorter, under
`$COMMONMEASURE_HOME/declarations/<host>.json` beside the licence documents it
names (`cache` says `fetched`, `reused`, or `not_asked` where the host's
`Crawl-delay` put the probe beyond what the fetch may still spend waiting, or
the crossing was refused before the licence was reached); a probe that
failed, or that the host answered outside 2xx (and, for `robots.txt`, with
429 or outside 4xx), is cached for five minutes, with the failure in its
record's `unavailable` field and the answer in `status`; a licence that
answered 404 or 410 is cached for five minutes too, and asked again after
them. A probe that failed for a reason of this edge's own (`cut_short`,
below) is not cached as the host's failure: its record expires when it is
written, and the next crossing asks again. A licence probe that was not asked for is remembered only until the
next crossing. A probe
that waited out a turn is dated when it was sent, not when the wait began.
A licence is `unread` (true) where it exists and could not be read: it was
not asked for, the request failed or timed out, the host answered outside
2xx other than 404 and 410, the body was over the bound or did not parse as
RSL. A 401, 403, 451 or any other 4xx but 404 and 410 says the document
exists and is withheld from this fetcher, so it is `unread`. A 5xx is
`unread`. Redirects are followed, and the final status decides. A licence
the source named that answers 404 or 410 is `missing`: `status` holds the
code, `unavailable` says the declared licence is missing, and `missing` is
a gap (`reason: "evidence_missing"`) naming the URL and the status; the
licence has no terms and is not `unread`. The same `status` and `missing`
are on the licence in the tool result's `declarations.licences`. A licence
that was read and has no `<content>` entry for the page is not unread; it
names no terms for that page. For `robots.txt`, a 4xx other than 429 is not
a failure: the host publishes no rules for this fetcher (RFC 9309 §2.3.1.3),
and the answer is cached as a file is. A failed `robots.txt` probe never
overwrites the host's last answer: it is kept under `robots_held` in the
cache file while `robots` holds the failure, and dropped once the host
answers again. Cache files are written whole and renamed into place. Every probe goes
through the same host policy and address floor as the page, and no probe is
ever recorded as a crossing.

`robots` is the access-rule attribution for the URL the crossing names.
`requested_url` is the URL the rules were applied to; `url` is the
`robots.txt` at its origin; `final_url` is the URL that answered the
request for it, after redirects, and differs from `url` where the file
redirected; `reading.group` is the group selected, with
`group_is_wildcard` true where the `*` group was used because no group
names `CommonMeasureBot`; `reading.access_rule` is the Allow or Disallow
line that decided, as written, with `access_rule_wildcard` true where its
pattern uses `*` and so matched by wildcard rather than as a literal prefix;
`mode` is the policy mode the session was in and `outcome` what the rule
did: `allowed`, `refused` (a `Disallow`, before the request), `unreachable`
(the file could not be reached and no answer was held, so every path is
disallowed and the crossing stopped before the request), `cut_short` (this
edge's own request for the file failed and no answer was held, so the
crossing stopped before the request; below) or `unavailable`
(this edge did not request the file, because the operator's policy, the
address floor or the hub's origin stops requests to its origin; nothing was
held, no rule was applied, and the page, at the same origin, is refused by
the same check). A `Disallow` for the selected
group binds in every policy mode (owner decision, 14 September 2026), so `refused` is written under
`strict`, `observe` and `prefer` alike, and `mode` records the session's mode
without having decided anything. The refusal text and the agent's
`declarations.robots.explanation` carry the same attribution in words; the
refusal ends "A `Disallow` binds in every policy mode: refused before the
request." A refusal on the access rule ends with the source's file, "(the
source's robots.txt, <url>; …)", where other refusals name the operator's
policy file. Two refusals on the access rule name something else, because
the source's file is not where the reader can act. A file unreachable
because its redirect went to a host the operator's policy refuses, or to a
private address the policy could admit, ends "(operator policy in <file>,
which does not admit <target>, where the source's robots.txt redirected;
…)". A file this edge cut short ends "(this edge did not read <url>, and
the next crossing asks for it again; …)".

A `robots.txt` over 512 KiB is parsed up to its last complete line within
512 KiB, and the rest ignored (RFC 9309 §2.5; owner decision, 22 September
2026). `truncated` then gives `size`, the bytes the host sent after any
content coding was removed, and `read`, the bytes parsed, and the
explanation says how much was read. Where the reading came from a held copy,
`truncated` describes that copy. A first line longer than 512 KiB leaves
nothing to parse, so no rule applies. A body over the HTTP client's 32 MiB
ceiling is a failed request, and so unreachable.

```json
"truncated": {"size": 614437, "read": 524270}
```

A `robots.txt` that cannot be read is ruled as RFC 9309 §2.3.1 says for a
crawler, in every mode (owner decision, 22 September 2026). `status` is what the file answered, where it
answered. `unreachable` is true where the file answered 429 or a 5xx (or
any status that is neither 2xx nor 4xx), or where a timeout, name, TLS or
connection failure left no answer, or where the file redirected to a URL
this edge declines to follow; `unavailable` then names the failure, and a
probe timeout reads "did not answer within the 5s exchange budget".
A declined redirect is one to an address this edge does not mediate, to the
hub's origin, or to a host the operator's policy refuses. `unavailable`
reads "<file> redirected to <target>, which this edge does not follow:
<reason>", `declined_redirect` names the target, and the refusal cites RFC
9309 §2.3.1.2 beside §2.3.1.4. §2.3.1.2 expects a crawler to follow at
least five redirects; this edge follows five, and a sixth leaves the file
unreachable ("redirect limit exceeded").

Under `observe` and `prefer` the host policy records a breach rather than
refusing, so a `robots.txt` redirect to a host outside an allowlist is
followed there, and the file it reaches rules the page (§2.3.1.2).
`final_url` names that file, and the explanation and any refusal name it
too: "<url> (redirected to <final_url>) disallows …". The crossing that
sent the request carries the host policy's breach in `breach`, as a
crossing whose page redirected to such a host does: "<url> redirected to
<final_url>, which was requested for its rules: <reason>". A later crossing
that reuses the cached copy sends nothing to that host and carries no
breach for it.
Where the cache holds an earlier answer from the host, `reading` is read
from it however old it is, `held_copy` names it (`fetched_at`,
`expires_at`, `status`, and `final_url`, the URL that gave it), and
`outcome` is what its rules say. Where none is
held, `reading` is absent, `outcome` is `unreachable`, and the refusal names
the file, the host and the failure, and ends "An unreachable robots.txt is
a complete disallow in every policy mode: refused before the request."
Before 22 September 2026 a declined redirect and a body over 512 KiB were
`unavailable` and the crossing proceeded with no rule; a cached probe of
either kind is asked for again rather than reused.

A probe can fail for a reason of this edge's own rather than the host's:
the whole-call time limit (below) left no time for the request, or the
request was given only what was left of that limit, less than the 5 s
exchange budget, and had not answered when the limit was used up; or the
request could not be signed. The host is not held to have failed. The file
is not `unreachable`: `cut_short` is true, `unavailable` names the reason,
and the cache record expires when it is written, so the next crossing asks
again. A held answer rules as it does for an unreachable file. With none
held, `outcome` is `cut_short` and the crossing is refused before the
request in every mode, because a page is not fetched under a `robots.txt`
this edge has not read. The refusal ends ": refused before the request, in
every policy mode." A licence probe cut short is `unread`, and so refuses
the crossing, and is asked again at the next crossing; a manifest probe
cut short is `unavailable` and is not kept. Before 22 September 2026 these
cases were recorded as the host being unreachable and cached for five
minutes.

```json
"robots": {"requested_url": "https://publisher.example/private/report",
           "url": "https://publisher.example/robots.txt",
           "final_url": "https://publisher.example/robots.txt", "cache": "fetched",
           "fetched_at": "…", "expires_at": "…", "status": 503,
           "unavailable": "https://publisher.example/robots.txt answered 503",
           "unreachable": true,
           "held_copy": {"fetched_at": "…", "expires_at": "…", "status": 200,
                         "final_url": "https://publisher.example/robots.txt"},
           "reading": {"group": "*", "group_is_wildcard": true, "crawlable": false,
                       "access_rule": "Disallow: /private/", "access_rule_wildcard": false,
                       "statements": [], "licences": []},
           "mode": "observe", "outcome": "refused"}
```

`carried` is no longer written. Under 0.3.5 and earlier, `observe`
and `prefer` made a disallowed request and recorded the breach with outcome
`carried`; those records still read, and a reader treats `carried` as a
`Disallow` that was not obeyed. Preferences read from `robots.txt`
(`Content-Usage`, `Content-Signal`) are not the access rule and still follow
the session's mode (below).

`reading.crawl_delay` is the selected group's `Crawl-delay`, and `delay` is
what it did to this request. The delay binds in every policy mode (owner
decision, 14 September 2026),
signed or unsigned. `value` is the line as written and `delay_ms` that
value in milliseconds; a value that is not a non-negative decimal number
of seconds is listed in `unreadable` and ignored, and a group that states
none has no `crawl_delay`. `honoured_ms` is the delay this edge keeps:
`delay_ms`, or 60000 where the file states more, with `capped` true. Every
group naming `CommonMeasureBot` supplies the delay where the file has such a
group, and every `*` group otherwise, as for the access rules: RFC 9309
section 2.2.1 merges the records of all the groups that name the fetcher, and
the merged values keep the longest delay.

The time of the last request to each host is kept under
`$COMMONMEASURE_HOME/crawl-delay/`, one file per host with a lock file beside
it, so the delay holds across sessions, across restarts and across two MCP
servers on one edge: each request takes its turn under the lock, writing the
time it will be sent before it waits. The file is named for the host, or, for
a host too long for a file name, its first 40 characters and a SHA-256 digest
of the whole name. `delay.outcome` is `clear` (the last request was longer ago
than the delay), `waited` (`wait_ms` slept through before the request),
`refused` (the wait did not fit `budget_ms`; nothing was requested and
`next_at` says when the host may next be asked) or `unavailable` (the turn
itself could not be kept, and nothing was requested rather than a delay being
dropped). A refusal names the file, the delay as written, the host and
`next_at`, in every mode. `licence_first` names the licence whose turn the
page waits behind (below); on a refusal decided before either turn was
taken, `wait_ms` is the licence's wait and the page's delay together,
`next_at` is when the host may next be asked, and where the licence was
not sent `unavailable` says why.

`budget_ms` is what the `context_fetch` may still spend asleep when the turn
is taken. On an edge served over stdio one call may spend 60 seconds asleep
in total, the same bound as the longest delay this edge keeps, so a first
crossing to a host is never refused by the delay it has just read. On a
hosted edge, served over streamable HTTP, one call may spend 20 seconds, and
a wait beyond that is refused with the transport named (below). Only sleeping
is charged to the budget: name lookups, connections, signing and responses
are bounded by the request timeouts and by the whole-call ceiling instead.

The requests of one crossing take their turns in this order. `robots.txt`
takes none, because the delay cannot be read without it. The access rule is
ruled from it before anything else is asked of the host, so a crossing
refused on a `Disallow` or an unreachable file, in any mode, takes no turn
and asks for nothing further: not the page, not the licence `robots.txt` names or the one the
page last named (a licence `robots.txt` names is recorded `not_asked`), and
not the Content Telemetry manifest.

A page is never admitted under a licence this edge has not read (owner
decision, 22 September 2026: a reporting demand binds in every policy mode,
and content whose reporting and licence requirements are not respected is
not had). So a licence with no current reading in the cache is read before
the page: the one `robots.txt` names, and, on a host that states a delay,
the one the page's `Link` header named when it was last fetched. Each takes
the host's next turn, and the page waits for its own turn after them, so a
first crossing to a host that states a delay and names a licence spends two
delays; that is the cost of reading the terms. Whether licence-then-page
fits what the call may still spend asleep is decided before either turn is
taken, from the host's pending wait plus one delay per licence: where it
does not fit, the crossing is refused before any request is sent to the
host, naming the licence and the delay, with the licence recorded
`not_asked` and `delay.licence_first` set. The licence is never read only
for the page to be refused after it. A licence already read and still
current takes no turn, and the page's turn is the only one. A licence
reached only through the page's response, on a first fetch, is read after
the page with whatever the call has left; where that does not fit it is
`unread`, the bytes are withheld, and the next crossing reads it first.

The page's own turn is taken last, once the licences have been read, any
allowance consulted and what the terms say ruled on, so a crossing its
terms refuse waits for nothing and leaves the host's turn to the next
request. Where the host states a delay, no licence is to be read first and
its manifest is not in the cache, the manifest probe is asked before the
page, but only if the host is clear: it takes a turn that costs nothing or
is not sent. Where a licence is to be read first, the licence has the
host's next turn and the manifest waits for a later crossing: it carries no
demand. Every redirect hop is ruled
and takes its own host's turn in the same order. A publisher's log cannot
tell a probe from the page fetch, which is why the probes are paced; a
manifest probe whose wait does not fit is not sent and is recorded as
`not_asked` rather than refusing the crossing. A licence another server's
turn crowds out between the check and the probe is not sent either, and
the crossing is refused with `unavailable` saying why.

A licence `robots.txt` or the page names that is `unread` refuses the
crossing in every policy mode: its terms exist and are unknown, and they
may carry a reporting demand. The failure is recorded as `unavailable`,
remembered for five minutes and asked again after it. A licence that is
`missing` (404 or 410) does not refuse: the crossing proceeds in every mode
as though no licence terms were declared, and the record and the tool
result carry the gap naming it (owner decision, 22 September 2026:
adoption of licensing standards is uneven, and a publisher's broken link
should not make its site unreachable). Both rules hold where the source's
statements govern; where an applicable operator assessment governs, the
assessment stands in for the licence's terms, as it does for a licence
that was read.

On a paced host the manifest is asked before the page rather than after it,
which is an exception to discovery never delaying a crossing. After the page,
the probe would need a whole delay from what the page left, so a host stating
more than half the budget would never have its manifest resolved and would
lose its owner identity and telemetry endpoint on every crossing. A manifest
that was not asked is recorded with `cache: "not_asked"` and held until the
host may next be asked (`expires_at` is the delay after the attempt), so the
next crossing after the delay asks it at its first free turn, and the record
is not rewritten on every crossing in between.

The host timeouts these bounds sit under were measured in Claude Code 2.1.278
on 22 September 2026:

- **stdio:** a tool call is ended at `MCP_TOOL_TIMEOUT`, 300000 ms by default,
  a hard wall-clock limit per call that progress notifications do not extend,
  with a per-server `timeout` in the host's configuration overriding it.
- **streamable HTTP and SSE:** a request is aborted at 60000 ms by default. A
  configured timeout is raised to at least 60000 ms, and the floor is 30000 ms
  (`MCP_TIMEOUT`).

Without a whole-call bound the worst case was about 310 seconds: six requests
along a five-hop redirect chain at the 30-second client timeout (180 s), two
5-second probes a hop (60 s), the two manifest probes (10 s) and 60 seconds of
sleeping. That exceeds the stdio default. One `context_fetch` therefore keeps
a whole-call time limit: 240 seconds over stdio, and 50 seconds on a hosted
edge, under the 60-second streamable-HTTP default (lead's decision, 22
September 2026). It is checked before every turn, hop and probe: past it no
further request is sent, a probe is recorded `not_asked` and a hop is
refused, each saying the call reached its time limit, and what may still be
slept is never more than what is left of it. No request, page, hop or
probe, is given longer than what is left of the limit, so a request in
flight does not carry the call past it; name resolution is the one step no
timeout here bounds. The hosted budget of 20 seconds of sleeping sits inside
the 50. The hosted limit caps how far a hosted call follows a slow redirect
chain: a chain that has not answered inside 50 seconds is refused naming
the limit, where an edge served over stdio would have followed it, and the
caller reads a refusal rather than a timeout. The wait parks the thread
that serves the call and cannot be cancelled; a host configured with a
shorter timeout than these sees the call time out rather than the
refusal.

A turn that cannot be kept is fail-closed: where the store's directory cannot
be created, the lock cannot be taken or the record cannot be written, the
outcome is `unavailable`, nothing is requested, and the reason names the
remedy, with the directory on an edge served over stdio. A record that cannot be read, or that is dated
beyond the longest turn plus the longest wait because the clock moved
backwards, is discarded: the request owes a full delay, waits it out, rewrites
the record, and `recovered` says which record was discarded and why. Nothing
prunes the directory: it holds one small file and one lock file per host ever
fetched, an operator clears one host by removing its file and all of them by
removing the directory, and each host's interrupted writes are cleared when it
next takes a turn.

An edge serving several tenants from one home paces them all as one fetcher,
because one identity is what the publisher sees. A refusal served over that
transport carries no `next_at` and no `wait_ms`, and says the session is
served over HTTP; its `unavailable` and `recovered` text names the store's
file by name alone and no recorded instant, because the tenant can act on
neither the operator's file system nor another tenant's fetch. A `waited`
ruling still carries `wait_ms` there, and a tenant can derive from it when
another tenant last asked the host; the call's own elapsed time gives the
same figure, so withholding it would hide nothing.

Each redirect hop is evaluated against the `robots.txt` at its own origin
before the hop is requested: a hop its own file disallows is refused, in
every mode, before that hop is asked for, and being a redirect exempts
nothing.
`redirects` holds the evaluation of every earlier hop in request order, and
`robots` is the last hop evaluated: the URL that answered, or the hop that
was refused. A shortener's `Disallow` is therefore recorded against the
shortener's URL and never against the page it pointed at, and a destination
URL the agent names directly is judged by its own file with `redirects`
empty. `statements` and `licences` are those of the origin that answered.
Where a hop is refused by host policy before its file is read, `robots`
stays the last hop that was evaluated and its `requested_url` says which.

What the operator's mode does with a disallowed `ai-input` statement, the
use a mediated fetch makes of a page: `strict` refuses, before the request
when the statement was known then and otherwise after it, in which case the
bytes were fetched and are withheld from context, their hash on the refused
crossing and `grounded` false; `observe` and `prefer` carry the crossing with
the statement named in `breach`. A `robots.txt` `Disallow` for the selected
group is not ruled on this way: it refuses in every mode (above). A licence whose AI-input permission is
conditional on a payment, or on a token from a licence server, is a term this
edge cannot meet without a settlement rail, and is ruled on the same way with
the term named.

A licence's reporting demands are ruled on where the source's own statements
govern, whatever the combined AI-input preference. A `Content-Signal` in
`robots.txt` or a `Content-Usage` header that disallows AI input beside a
licence that permits it does not set the demand aside: `observe` and `prefer`
carry the Disallow as a breach, and the demand is ruled on as well. Where an
applicable operator assessment governs instead, the licence is not read for
demands, and the operator's own declared reporting duty is what applies; that
duty follows the session's mode like any other operator ruling.

Which demands apply follows RSL 1.0 §3.12, under which a demand binds activity
the enclosing licence authorises. A licence authorises AI input where its
`<permits type="usage">` covers it, or where it has no usage `<permits>` at
all, because §3.5 restricts usage only where a `<permits>` of that type
exists (§3.4 and the grammar make every child of `<license>` optional); in
both cases no usage `<prohibits>` may cover it. The demands of the licence the
crossing is taken under are ruled on: one that permits AI input by name
first, else one silent on usage. Where no licence in the governing entry
authorises AI input and the crossing goes on outside `strict`, every demand in
the entry is ruled on: taking the page outside the licence does not excuse
the fetcher from the report its owner asks for (owner decision,
22 September 2026).

A licence's telemetry reporting demand is met when its profile is the Content
Telemetry binding this runtime speaks, its conformance level is one this
runtime emits (`retrieval` or `grounding`), and the session can deliver:
the policy scope clears telemetry egress, `$COMMONMEASURE_HOME/relay.json`
names a receiver, and automatic delivery is in force. Automatic delivery
means the events leave without anyone typing a command — the session-end
relay (§Relay at session end) or the hosted service's interval. The marker
file `$COMMONMEASURE_HOME/relay/manual` switches it off, and a demand is
then unmet however the rest is configured; the reason names the marker, so
the operator reads what to remove. A profile this runtime does not recognise
is an unmet demand.

Which sessions have automatic delivery is read from the session itself and
the home's own state, never from host files under `$HOME`:

- A running hosted service on the home: automatic for every session in it,
  whatever its host word or client, stdio sessions included, because the
  service's interval relay reads every session log in the home. "Running"
  means a process holds the home's lock `hosted-service.lock` now
  (`delivery::service_running`, the probe `doctor` uses); a service that is
  configured but stopped holds nothing and does not count. The interval
  relay skips its run while `relay/manual` is present and says so in the
  journal.
- A hosted session under `hosted service`: automatic for the same reason.
  `hosted serve` runs no interval relay and takes no lock, so its sessions
  are not automatic.
- `--host claude-code` with the client `claude-code`: automatic. Claude Code
  is the one host whose hook table registers a session-end event
  (`CLAUDE_HOOKS` in `crates/commonmeasure-harness/src/registration.rs`); the
  Cursor and Copilot CLI tables register none, and Codex, Pi, Claude Desktop
  and VS Code have no hook that runs at a session end. `SESSION_END_HOSTS` in
  `crates/commonmeasure-harness/src/delivery.rs` is held to those tables by a
  test.
- Every other `--host` word: unmet, with the reason "no automatic delivery;
  this host (`<host>`) sends no session-end event".
- Under `--host claude-code`, any client name but `claude-code`: unmet, and
  the reason names the client. `--host` defaults to `claude-code`, so the
  host word alone does not identify Claude Code: Goose has no `install` and
  is registered by a hand-written extension with no `--host`
  ([`host-integration.md`](host-integration.md) §6), and any client not yet
  captured arrives the same way. The check is an allowlist because the two
  failure modes are not symmetric: a Claude Code build that announced another
  name would be refused visibly and fixed by one string, where a client
  missing from a blocklist would take content whose report nothing relays.
  The name is the one every Claude Code session recorded on the owner's
  machine announced, `{"name": "claude-code", "title": "Claude Code"}`: 10 of
  10 `client_identified` records, Claude Code 2.1.270 to 2.1.278, September
  2026. Names known to belong to other hosts (`codex-mcp-client`,
  `claude-ai`, `local-agent-mode-*`, `cursor-vscode`) only make the reason
  more precise ("is not Claude Code"). A session that sent no `clientInfo`
  trusts the host word; MCP clients must send it, so that arises only in
  tests.

Whether Claude Code's `SessionEnd` hook is actually installed is `doctor`'s
check (§Relay at session end), not the ruling's: reading it at each licence
ruling would make the outcome depend on the machine's host files. An
operator on Codex, Cursor or another host without a session-end event can
connect through a hosted endpoint where the host supports one (the service
serves `claude-connector`, `chatgpt`, `m365-copilot` and
`copilot-cloud-agent`, behind OAuth), or run `commonmeasure hosted service`
on the same home the stdio server uses, which needs that home enrolled and
under managed policy; while the service holds the home's lock, that home's
stdio sessions count. Otherwise a source whose licence demands usage
reporting is refused on that host. Tested in
`crates/commonmeasure-cli/tests/mediated_e2e.rs`
`reporting_demand::a_host_that_sends_no_session_end_event_leaves_a_telemetry_demand_unmet`,
`a_known_other_client_under_the_default_host_word_leaves_the_demand_unmet`,
`an_unknown_client_under_the_default_host_word_leaves_the_demand_unmet` and
`a_stdio_session_on_a_home_a_running_service_holds_meets_the_demand`,
and in `crates/commonmeasure-harness/src/mcp.rs`
`a_hosted_session_meets_a_reporting_demand_only_under_the_interval_relay`.

A `<reporting>` element of another type (`provenance` or `audit`) is a demand
this runtime cannot meet, since it reports telemetry only; it is unmet
whatever the scope clears, and the reason names the licence, the type and the
profile. An element that states no type is read the same way: RSL 1.0 §3.12
lists the types a client must satisfy and states no default for an element
that names none, so this runtime does not read a typeless element as the
telemetry one it can meet. The record shows the type as `unstated`.

An unmet reporting demand refuses the crossing in every policy mode, whether
the demand was known before the request or only after the bytes arrived, in
which case they are withheld from context with their hash on the refused
crossing (owner decision, 22 September 2026). The mode the
operator set is not the source's consent, and RSL 1.0 §3.12 says an activity
whose applicable demands are not satisfied is unlicensed. The operator's
choice is whether to report, which the marker expresses, not whether to
decline the duty and still read. Every other declaration ruling keeps the
mode discipline above. Tested in `crates/commonmeasure-cli/tests/mediated_e2e.rs`
`reporting_demand::a_disallow_in_robots_does_not_excuse_an_unmet_reporting_demand`,
`a_content_usage_disallow_does_not_excuse_an_unmet_reporting_demand`,
`a_licence_without_permits_still_binds_its_reporting_demand` and
`a_met_demand_beside_a_disallow_is_carried_with_the_breach_in_observe`.

The telemetry ruling is on the record as `declarations.reporting`: the
profile and level demanded, the receiver named or its absence, whether the
scope clears egress, `met`, and the first reason it is not, the marker
included. A demand of another type has no entry there; its refusal is the
record of it.

`content_telemetry_id` is the UUID a mediated fetch sent as its
`Content-Telemetry-ID` request header (Content Telemetry section 7.2), so
an owner that logged the header can match the retrieval event the relay
projects to its own line; the relay carries it on the retrieval event and on
nothing else. It is sent only to a host where a manifest was verified or a
licence was read before the request, so it is absent on the first fetch of
a new host, on a host that has declared nothing, and on every search
result. It is re-attached on a same-domain redirect and left off a
cross-domain one. A mediated fetch the origin answered outside 2xx, or that
no origin answered, is on this record with its `http_status` or `failure`
and is never projected as a retrieval.

`allowance` is the allowance gate's account of a fetch whose licence, read
before the request, quoted a price for AI input and whose principal declares
an allowance: the consultation, the quoted price reserved before the request
(`decision`: `reserved`, `proceeded_with_breach` or `declined`), and its
settlement against the receipt. Strict refuses an exhausted or incomparable
allowance before the request; observe and prefer carry the fetch with the
breach named. No settlement rail that pays a quoted price is built, so the
receipt
reports no charge, the reservation is released on it, and `paid` says
nothing was; a rail that pays reconciles the same reservation against what
it charged. Absent where no licence quoted a price, the operator's terms
govern, or the principal declares no allowance.

`named_by` says who named the source of a mediated fetch: `user` when a
prompt of this session carried the URL the fetch was asked for, `agent` when
prompts were recorded for the session and none carried it, `unknown` when the
log holds no `prompt_sources` record, which is every host without the prompt
hook. It is decided from the recorded hashes (§Prompt sources) against the
URL as asked, with and without a trailing slash. "Provided by the user"
means the user supplied the content bytes; a URL the user pastes is a
reference, and the fetch is acquisition by the system, so `named_by` is a
fact for the operator's policy and never a carve-out from a preference.

The operator's terms reference and assessment are recorded separately from
source statements under `declarations.terms`. Their fields and scope checks
are defined by [source policy](source-policy.md#terms). A reference alone,
including a supplier API subscription, leaves applicability unresolved.
Only an applicable assessment covering the requested URL and `ai-input`
governs: `governing` is `operator_terms`, the source statements remain in
`statements` and `effective`, and the assessment's reporting duties apply.
Otherwise `governing` is `statements` and the source declaration and policy
mode decide whether content is admitted.

Where a terms entry names the host, `declarations.assessment_decision`
records the rule, actual URL and use, scope result and explanation, policy
mode and declaration-check outcome. Successful tool results carry the same decision; refusal results carry an
error and leave the structured decision in the source record.
The assessment's own reason remains in `terms.assessment.reason`.

```json
"assessment_decision": {
  "rule": "terms (publisher.example) assessment",
  "url": "https://publisher.example/articles/1",
  "intended_use": "ai-input",
  "applicability": "applied",
  "reason": "The operator assessed this basis as applicable to the requested content and AI input.",
  "mode": "strict",
  "outcome": "allowed"
}
```

`applicability` is `applied`, `out_of_scope` or `unresolved`. The latter
includes a legacy reference with no assessment. `outcome` is `allowed`,
`allowed_with_breach` or `refused` for the declaration checks. `mode` and
`outcome` are `null` if an earlier allowance refusal prevents those checks
from running. A separate host, screening or transport failure can still
prevent delivery. The
crossing's refusal or breach preserves the resulting reason. An unresolved
assessment remains unresolved in observe and prefer modes even when content
is carried. Each redirect destination is evaluated against its own content
scope before being requested; the final decision names the final or refused
hop, as the robots evaluation does.

Applicable terms may name the institution identifiers the basis attributes
usage to (`access_context`). They require the standard's session-level
`access_context` container on any report. The event batch the relay delivers
has no session data, so a session under such terms is withheld by the relay
and counted as withheld for that reason, until session-document delivery is
built. The relay conservatively retains this withholding for a configured
host's institution identifiers even when its assessment does not apply.
`licence.state` is `declared` with the terms reference only where its
assessment governs, with the licence URL where RSL terms were read, and
`unknown` otherwise. Reaching a page establishes no permission.

Issuer and authority references are operator assertions, retained without
legal verification. [Embedded credentials on acquisition](#embedded-credentials-on-acquisition)
describes verification before transformation; the pasted-content reader below
is a separate path.

## Manifest discovery

After a mediated fetch succeeds, the server resolves the host's Content
Telemetry discovery manifest (`/.well-known/content-telemetry.json`,
standard section 8) and writes a `manifest_resolved` record; the crossing
carries the record's `seq` as `manifest_record`. Concurrent writers can reuse
sequence numbers. The console displays manifest details only for a unique
preceding reference with the crossing's host; missing, foreign or ambiguous
references are labelled unavailable or ambiguous
(`crates/commonmeasure-harness/src/manifest.rs`,
`crates/commonmeasure-harness/src/discovery.rs`). The manifest identifies an
owner and its telemetry endpoint and carries no demand, so discovery never
delays the crossing's ruling and never refuses anything. The one exception is
a host that states a `Crawl-delay`: its manifest is asked before the page, at
the crossing's first free turn, and the page waits behind it; where the host
is not clear the probe is not sent, the record says `cache: "not_asked"`, and
it is held until the host may next be asked (§Source declarations).

```json
{
  "session_id": "…", "host": "news.publisher.example", "timestamp": "…",
  "cache": "fetched", "fetched_at": "…", "expires_at": "…", "max_age": 3600,
  "probes": [{"url": "https://news.publisher.example/.well-known/content-telemetry.json", "status": 404},
             {"url": "https://publisher.example/.well-known/content-telemetry.json", "status": 200}],
  "outcome": "verified",
  "facts": {"schema_version": "1.0", "id": "https://publisher.example/.well-known/content-telemetry.json",
            "roles": ["content_owner"], "operator": "Publisher Example",
            "telemetry_endpoint": "https://telemetry.example.com/v1/events",
            "domains": ["publisher.example", "*.publisher.example"], "key_ids": ["key-1"]}
}
```

The consumer rules are the standard's (section 8.7): a 404 or a network
error leaves the participant unverified and rejects nothing (`not_published`
and `unavailable`); invalid JSON, a schema failure, a duplicate key id or a
`domains` entry that is not the manifest's own host or a subdomain of it
rejects the manifest with the reason (`rejected`); any `1.x` version is
accepted. When a subdomain answers 404 the apex, one label up, is asked once,
because an apex manifest may claim its subdomains; `probes` lists both. The
outcome is cached per host under `$COMMONMEASURE_HOME/manifests/<host>.json`
for the response's `max-age`, an hour when it names none, a day for a 404 and
five minutes for a failure, written whole and renamed into place; a crossing
under a live cache entry records `cache: reused` with the same facts. The
manifest's `id` must name the host it was fetched from, after redirects. Every probe goes through the same host policy and address
floor as the page. The relay projects nothing from this record
(`crates/commonmeasure-relay/tests/relay.rs`).

## Prompt sources

The prompt-submission hook records, per prompt, the URLs the prompt named and
the statements pasted material carried (`crates/commonmeasure-harness/src/prompt.rs`):

```json
{
  "session_id": "…", "host": "claude-code", "timestamp": "…",
  "url_hashes": ["sha256:…"],
  "url_hash_basis": "sha256 over the URL text as written in the prompt, trailing punctuation removed",
  "statements": [{"source": "pasted-c2pa", "category": "ai-input", "preference": "disallow",
                  "detail": "cawg.training-mining cawg.ai_inference: notAllowed"}],
  "references": [{"kind": "constraint-info", "href": "https://publisher.example/license.xml"}],
  "c2pa": {"method": "annex-a8", "validation_state": "valid", "claim_generator": "…",
           "training_mining": {"cawg.ai_inference": "notAllowed"}},
  "embedded": "statements read",
  "not_scanned": ["images: the prompt hook receives text only, so a manifest embedded in a pasted image is not read"]
}
```

No prompt text and no URL text enters the record: a URL is kept as its hash,
so a later mediated crossing can be matched to it. Pasted material is read
for an RSL licence in a `<script type="application/rsl+xml">`
(`pasted-rsl`), a `<link rel="license" type="application/rsl+xml">` (recorded
as a reference, not fetched), a `Content-Usage:` line in a pasted HTTP
response (`pasted-content-usage`), and a C2PA manifest embedded in text under
Annex A.8 or in HTML under A.7, read with the C2PA SDK (`pasted-c2pa`). From
the manifest the training-and-data-mining entries are taken: `allowed` is
allow, `notAllowed` and `constrained` are disallow, and a `constraint_info`
URL is recorded as a reference for the mediated path to follow. The SDK's
validation state is recorded as reported; without a trust list a well-signed
store is `valid`, not `trusted`. Plain text with nothing embedded is recorded
as `no embedded statement`, which is unknown. A pasted image is not read,
because the hook receives text only, and the record says so. The hook makes
no network request.

## The nudge issuance

The plugin's `SessionStart` hook emits the standing mediation nudge
(`plugin/README.md` §The standing nudge) and records that it did:

```json
{
  "session_id": "…", "host": "claude-code",
  "timestamp": "…", "nudge": "mediation-nudge/3", "source": "startup",
  "basis": "emitted on the SessionStart hook's stdout for the host to add to the session's context; injection is the host's act and is not witnessed"
}
```

The record also carries `policy_identity`, or `policy_unavailable` where
the hook could not read the policy (§Policy identity).

`nudge` names the versioned wording in force
(`crates/commonmeasure-harness/src/nudge.rs`), so a review counting mediated against
observed crossings can split sessions that were asked from sessions that were
not. `source` is the host's own account of how the session began (`startup`,
`resume`, `clear`, `compact`); each of those starts re-delivers the nudge,
because each rebuilds the context the nudge is in. The basis is in the
record because only emission is witnessed: adding hook stdout to the model's
context is the host's act. Nothing records whether the agent read or obeyed
it; the session's own crossing counts measure obedience.

## Instance registration

A working instance is registered with the hub the edge is enrolled with
under the [instance registration contract](instance-registration.md). The
edge half is built against the hub's first increment and names the
reporting duties of its second; the client names no entitlement or shared
limit. Integration state: `fixture-tested`
(`crates/commonmeasure-relay/tests/instance_registration.rs`, against a test
double of the hub that verifies the signature by the hub's pinned rule). The
ignored test in `crates/commonmeasure-cli/tests/instance_e2e.rs` drives the
binary against a running hub; it was run against the real hub server and
database on loopback on 19 September 2026 and again on 21 September 2026. The
second test there carries
a reporting duty through `relay` to the hub's read of it, against a hub
started with its development onward setting (`DEV_ONWARD_LOOPBACK`, which a
hub accepts only in development mode on a loopback listener) so that it
delivers to the test's loopback receiver. That receiver is a test double
which refuses every delivery until the test tells it to accept. The hub
reads the duty `outstanding` while the instance is active; `outstanding`
with no acceptance after closure, once the double has refused a delivery;
and `delivered`, with the receiver's acceptance, after the double accepts
the hub's retry. The run establishes this for the edge and the hub on
loopback and for no receiver.
`crates/commonmeasure-cli/tests/instance_recovery_e2e.rs` is the third such
test, run the same way on 18 and 21 September 2026: an instance's
process and a relay are each killed with `SIGKILL` and a later process
recovers delivery (**A killed instance**, below). Its hub is started with
the same development onward setting and stands behind a pass-through
forwarder in the test, and its onward receiver is the same kind of test
double.
No hosted deployment has been exercised.

**The signature.** Each lifecycle request is signed with the key minted at
enrolment (`crates/commonmeasure-harness/src/identity.rs`
`sign_registration`). The signature covers `@method`, the absolute
`@target-uri`, `content-digest` (SHA-256 of the exact body bytes; of no bytes
for a read) and `idempotency-key`, with `created`, `expires` (300 seconds
later), `keyid`, `alg="ed25519"`, a fresh `nonce` and
`tag="commonmeasure-registration"`. The hub's interoperability vector is
copied to `crates/commonmeasure-relay/tests/fixtures/registration-request.json`
and reproduced byte for byte: the base, the headers and, Ed25519 being
deterministic, the signature. The two files change together. The target is
the origin the hub returned at enrolment plus the route path, because the hub
rebuilds the target from that origin; the request is sent to the hub URL
`connect` was given.

The hub refuses a `created` later than its own clock and allows no grace, so
the edge dates `created` 30 seconds before its own clock
(`REGISTRATION_SKEW_ALLOWANCE_SECS`) and `expires` 300 seconds after that: a
clock up to 30 seconds ahead of the hub's is accepted, and a signature is
good for 270 seconds from sending. RFC 9421 does not require `created` to be
the sending time. A clock further ahead, or more than 270 seconds behind, is
answered `401 invalid_registration_signature`. That answer and `401
registration_signature_required` are the hub turning the request away before
it looks at the key's standing or the instance; `401 nonce_reused` is raised
once the key has verified, when the nonce is consumed. None of the three
rules on the instance, so all three are recorded `unavailable`, never
`refused`, and the operation keeps the hub's `server_time` as `hub_time`
beside the edge's own `at`.

**The commands.** `commonmeasure instance register` sends the typed work
references (the session id as a `session` reference in the declared host's
namespace, a job id as a `job` reference in `context-job`), the host
declaration, the owner's delegation and custody acceptance by their hub
identifiers, the requested expiry, an optional parent and predecessor, and
the applied source policy: the envelope retained in
`managed/last-known-good.json` with the digest `managed/state.json` records
as applied. `renew` restates the retained registration with the policy then
applied and the expected revision; `close` sends the outcome and final
evidence digests; `status` reads the standing. `register --duty <reference>`
(repeatable) names a reporting duty in the hub's form,
`owner:<content owner id>`; a renewal restates it, and `close` reports every
duty the registration named as outstanding, because the hub reads a duty
`delivered` only after the instance has ended and the edge has no later
knowledge to state. The hub binds an idempotency
key to the exact body bytes, so each mutation's body is kept at
`<home>/instances/attempts/<idempotency-key>.json` before it is sent. A
command run again with the same `--idempotency-key` resends those bytes under
a fresh nonce and ignores the arguments of the second run, because a
recomputed expiry would otherwise turn the retry into
`409 idempotency_conflict`. An edge that is not enrolled,
not managed or has activated no policy revision is refused before any
request, naming what is missing. So is:

- an idempotency key outside the hub's rule, 1 to 128 of `A-Z a-z 0-9 - _`,
  which the hub would answer `401 invalid_registration_signature`;
- a key whose kept attempt was made for another hub: the kept body names that
  hub's delegation and carries its policy envelope, so the error names both
  hubs and asks for another key;
- `register` for a session that already points at an instance whose retained
  standing is `active` and whose window has not passed, unless
  `--replace-session` is given. With the flag the new record keeps the earlier
  instance as `session_taken_from`; the earlier instance is not ended at the
  hub and still closes. A retry under the key that registered the instance the
  session points at is not refused.

**What is kept.** `<home>/instances/<instance>.json`, mode 0600 in a 0700
directory (`crates/commonmeasure-harness/src/instance.rs` `Retained`): the
hub, the issuer, the instance, the revision, the standing the hub last
answered, the work references and session, `valid_from`, `expires_at`,
`next_check`, `online_validation_required`, the registration as sent without
the policy envelope, the signed accepted binding exactly as returned, the
closure receipt, each reporting duty as the hub last answered it (`duties`:
reference, state, event counts and receiver acceptances; the hub's statement,
following its latest answer: absent until an answer carried a duty, removed
when an answer's `duties` names none, and left as it is by an answer with no
`duties` array), and one entry per operation with its idempotency key, HTTP
status, outcome (`accepted`, `replayed`, `refused`, `unavailable`,
`unreachable`, `binding_rejected`, `closure_unacknowledged`) and the hub's
stable reason. The attempt file also holds what the last send of it did, so
a registration that produced no instance still leaves its refusal, and the
instance a registration produced.
The summary printed by `instance status`, `register`, `renew` and `close`
carries `closure`: the outcome, time and custody receipt as this edge retained
them, null where it holds none, including a closure the hub did not
acknowledge; `status` does not fetch it.
`<home>/instances/by-session/<session-id>` names the instance a session's
records are attributed to; closing the instance removes it, and `disconnect`
removes every pointer and keeps the records, because the instances belong to
the key given up and the hub answers `404` for them to a key enrolled later.
After `disconnect` those sessions are unregistered: nothing is checked and no
member is added.

The mediating server and the CLI both append to one record. Each reads the
record again and writes it while holding an exclusive lock on
`<home>/instances/.lock`, after the hub has answered and never across a
request, so neither drops the other's entry; a standing older than the
revision held is ignored. Files are staged under the whole file name plus a
value unique to the write, because session identifiers may contain dots.

An accepted binding is retained only if its signature verifies under the key
the answer names, the signed bytes are the binding shown, the purpose is
`instance-registration`, the binding names this edge key, the organisation in
the enrolment record and, as issuer, the origin the hub returned at
enrolment, and it agrees with the request it answers: `policy.revision` and
`policy.digest` with the envelope sent, `policy.applied_digest` with the
digest sent, `registration.work` with the work sent, every duty the request
named present in both `registration.duties` and the `terms.reference` of the
binding's `duties` (the binding may carry more, from a parent or
predecessor), and `expires_at` no later than the expiry asked for. The edge pins no registration signer yet, so this
does not establish that the signer is one the operator trusts; the policy
signer pin is not reused for it.

A binding that fails is `binding_rejected`. The hub has made the revision
whatever the edge thinks of it, so the instance is recorded with the hub's
instance and revision and the standing `binding_rejected`, which `status`
reads and `close` ends. A rejected registration names no session: nothing is
attributed to it. A rejected renewal keeps the last binding that verified,
takes the hub's revision so that closure states the right one, and leaves the
session pointed at the record: its crossings are refused
`instance_binding_rejected` until the operator closes the instance or
registers again. The hub answering `active` does not clear the standing.
A closure reports as outstanding the duties the registration named. Under the
standing `binding_rejected` it reports only those the hub's latest binding
also restates in `registration.duties`, because the hub holds a closure to the
binding it signed last and refuses a duty that binding lacks
(`unknown_duty_reference`), which would leave the instance active until
expiry. For a rejected renewal the record keeps that latest binding's accepted
document, unverified, beside the binding that verified (`rejected`).

**The binding applied.** Every record appended to the session's log while
its pointer exists carries `instance` (§Where), read from the retained
record at each append, so a renewal changes the revision on the records that
follow it. Before a `context_fetch` or `context_search` crosses, and after
the source policy's host ruling, the MCP server checks the retained binding:

| Condition | Outcome |
|---|---|
| the retained standing is not `active` | `refused`, `instance_<standing>` |
| `expires_at` is not after now | `refused`, `instance_expired`; the hub is not asked |
| `online_validation_required`, or `next_check` has come: the signed read of the instance answers `active` | the work proceeds; the revision, expiry and next check are taken from the answer |
| that read answers another standing, or `401 edge_inactive`, `401 unknown_edge`, any other `401` not named in the next row, `403`, `404` or `409` | `refused`, with the standing or the hub's reason |
| that read answers `401 invalid_registration_signature`, `401 registration_signature_required` or `401 nonce_reused`: the hub did not accept the request's signature and ruled on nothing | `unavailable`, `instance_check_unavailable`, with the hub's reason and both clocks |
| the hub is not reached within five seconds, answers anything else, or the retained record cannot be read | `unavailable`, `instance_check_unavailable` or `instance_record_unreadable`; there is no grace |

The hub's first increment sets `online_validation_required` and a
`next_check` equal to the time of issue, so every crossing is checked. A
stopped crossing appends `instance_refused` with `tool`, `outcome`, `reason`,
the `url` for a fetch, and the `instance` reference, and the tool error says
`refused before the crossing` or `unavailable before the crossing`. It is not
a `crossing_refused`: the source policy did not refuse the source, and the
record adds nothing to the wire's refused count. The source policy is ruled
on as before, whatever the registration says: a registration admits nothing
the policy refuses, and a stale cached policy stays enforced. Only the
validity window's end is judged by the edge's clock; whether the window has
begun is left to the hub's answer.

**A killed instance.** The edge keeps no state in the instance's process that
recovery needs. Each record is synced before its append returns and carries
the `instance` reference it was written under, the retained binding is a file,
and delivery state is the spool's (§Delivery state). After the process is
killed, `commonmeasure relay` from the same home, run by any later process,
projects the session's records and delivers them to the issuing hub with their
reference. A relay killed after it claimed a batch leaves the claim with no
outcome; the batch goes again at the claim's persisted deadline with the same
event identifiers and `instance` members, and the receiver's deduplication by
event identifier is what keeps a batch it had already accepted from counting
twice.

Nothing closes a killed instance for it. The hub reads it `active` and its
duties `outstanding` until an operator runs `instance close` from the same
home, or its window passes and the hub reads it `expired`; the hub accepts a
closure while standing is `active` or `expired` (closing an expired
instance is not exercised from this edge). Either ends the
instance, and the hub reads a duty
`delivered` only after that and after the onward receiver's acceptance. The
edge has no command that finds the instances whose process has gone, and
chooses no closure outcome on the operator's behalf.

A final record cut short by a kill inside an append is not repaired. The log
stays unavailable to a strict reader: `commonmeasure inspect` and
`commonmeasure session` fail naming the file. A relay run that reads the log
skips that session: nothing of the log is projected, spooled or sent, the
whole records before the cut-short one included. The run goes on for every
other session it reads and delivers every due batch in the spool. A batch
spooled earlier from the damaged session is delivered with them in a home
without a directory selection. Where a directory selection applies to the
batch, the recheck before delivery reads the batch's session log again; a
log that does not read cannot establish consent, so the batch stays queued
and unsent under a hold that consumes no attempt, the session is named with
the skipped ones whether or not the run was scoped to it, and the batches
behind it are delivered. The batch leaves on the first run after the log
reads. The run then fails, naming each skipped session, its file and the
number of its batches left queued, after the report of what it relayed; a
delivery failure in the same run names them as well. The hosted service
journals the report and the skipped sessions at each interval when no
delivery failed; a delivery failure is journalled under "relay did not run",
with the skipped sessions named after it; the relay did run (NET-08). The
relay keeps the sessions it last skipped in `relay/skipped-sessions.json`,
and `commonmeasure status`,
`commonmeasure doctor` and the egress block of the console's `/api/status`
(`skipped_sessions`, and in `detail`) name them, because a run that delivered
everything else leaves no delivery error. A run of the whole home replaces
the list; a run scoped with `--session` replaces only the entries of the
sessions it opened or skipped. Records already delivered
stay delivered. A session record carries no seal and no chain; those
belong to a published run. What can be checked after exit is that every line
is a whole record, that `inspect` joins each output to the acquisitions it
cites, and that an output's bytes hash to its recorded `output_hash`.

Integration state: `fixture-tested` by
`a_batch_accepted_before_the_relay_was_killed_is_sent_again_with_the_same_ids_and_reference`,
`a_cut_short_final_record_is_skipped_by_name_and_the_other_session_is_delivered`,
`a_damaged_log_does_not_hold_back_another_sessions_duty_bearing_batches`,
`a_batch_whose_log_is_damaged_later_stays_queued_under_a_directory_selection`
and `a_batch_whose_log_is_damaged_later_is_delivered_without_a_directory_selection`
(`crates/commonmeasure-relay/tests/relay.rs`),
`a_damaged_session_log_is_named_after_the_report_of_what_was_relayed`
(`crates/commonmeasure-cli/tests/relay_e2e.rs`),
`an_expired_instance_stops_its_session_while_a_stale_policy_still_rules`
(`crates/commonmeasure-relay/tests/instance_registration.rs`) and
`a_killed_writers_output_still_joins_its_acquisition_and_a_lost_final_append_is_explicit`
(`crates/commonmeasure-harness/src/observations.rs`), each against a stated
double. The run named above exercised the same properties against a real hub
server and database on loopback. The hub was started with its development
onward setting, behind a pass-through forwarder in the test that withholds
one answer so the relay can be killed at a known point, with a test double as
the onward receiver. In that run the output is a string the test supplies as
host and `content_hash` is compared with the killed process's own answer, so
its hash checks show that the record survived the kill and joins after exit;
they are not an independent reading of the page or of an output artefact.
No hosted deployment and no real receiver has been exercised.

A session with no pointer has no registration: nothing is checked, no member
is added and no request is made. Hook and import records are stamped like any
other record of the session; they are observed after the fact, so the check
cannot stop what they describe, and that coverage gap is unchanged.

## Policy identity

The digest of the effective policy a session resolved
([`docs/GLOSSARY.md`](../GLOSSARY.md) §Policy identity), recorded so a reader can tie each
governed record to the policy that governed it without the policy document,
and so two edges can be compared for drift by digest alone. It is SHA-256
over the canonical JSON of the pre-image
([`docs/contracts/canonical-json.md`](canonical-json.md)). The pre-image itself, the versions it
is taken under and the rest of the recipe are
[`docs/contracts/fleet-status.md`](fleet-status.md) §Policy identity; this section says where
the digest appears in a session and what its absence means.

Three record kinds carry it:

- `crossing_mediated` and `crossing_refused`: `policy_identity` is the
  digest of the policy the mediating server resolved at start and ruled
  under. The server resolves once, so every crossing in one server process
  names one identity, and a policy edited while the server runs reaches the
  record only when a server next starts.
- `turn_started`, `turn_completed`: `policy_identity` is the digest of the
  policy a mediated crossing in the boundary's `cwd` would meet, resolved
  from the same document by the same resolver. A hook enforces nothing, so
  this is the identity of the policy in force at that boundary and not a
  claim that anything was enforced by it.
- `nudge_issued`: `policy_identity`, resolved the same way against the
  `cwd` the host reported at session start.

On every record kind the field sits at the top level of the payload, so
one rule reads them all.

A hook that cannot read the policy records `policy_unavailable` with the
loader's reason in place of `policy_identity`. It does not record the
identity of the permissive default, because the operator did not write
that policy, and a mediating server refuses to start under the same file.

A record carrying neither field predates the field. `commonmeasure
session` counts those records and says they predate it; no reader treats
their absence as "no policy governed this", and no older record is
rewritten to carry one.

The digest is drift evidence. An edge computes it about the policy it
loaded and reports it about itself, so a matching digest on two edges says
they resolved the same effective policy and nothing about whether either
edge was tampered with.

## Client identity

`host` on every record is the word the registration passed to
`commonmeasure mcp --host`: `claude-code`, `codex`, `pi`, `claude-desktop`,
`cursor`, `copilot-cli` or `vscode`. The server refuses any other value at
start, but `--host` defaults to `claude-code`, so a registration that passes
no `--host` (Goose's hand-written extension, for one) is recorded as
`claude-code` whatever the client is; the `client` field below is what tells
them apart, and the reporting ruling reads it (§Source declarations).
Observed crossings also carry the hook command's host words, which include
the browser surfaces `chatgpt-web`, `google-ai-overview` and
`bing-copilot-search` ([`docs/contracts/host-integration.md`](host-integration.md) §2). The
same word covers several programs: one `[mcp_servers]` table serves the
Codex CLI, the ChatGPT desktop app and the Codex IDE extension, and Claude
Desktop connects its chat and its local agent mode as two clients. What
tells them apart is the client's own word for itself, the `clientInfo` it
sends in the protocol's `initialize` request, which the server holds from
the handshake and records once, before the first record a tool call leaves:

```json
{
  "session_id": "…", "host": "codex", "timestamp": "…",
  "client": {"name": "codex-mcp-client", "version": "0.154.0", "title": "Codex"},
  "protocol_version": "2025-06-18", "negotiated_protocol_version": "2025-06-18"
}
```

`protocol_version` is the revision the client asked for and
`negotiated_protocol_version` the one the server answered with, which
decides what the client could send. The server serves `2025-03-26`,
`2025-06-18` and `2025-11-25`; it answers a client that asks for one of those
with it, and any other client with `2025-11-25`, the latest, as the
protocol's version negotiation states
(<https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle>).
A client that asked for `2024-11-05` is recorded with the two values
different.

A stdio server that is started and never asked for anything leaves a
session file holding its start records only: `host_process` (§Host
process) on every edge, and `edge_identity` and `policy_sync` on an edge
that is enrolled with a hub or runs managed policy. It leaves no client or
credentials record, with or without an operator credentials file, because
`credentials_loaded` and `client_identified` both wait for the first record
a tool call leaves and `context_status` leaves none. A hosted session
on the same edge opens its file at its first request beyond the lifecycle
(§Where), so one asked for nothing leaves none.

The same `client` object is stamped on every `crossing_mediated` and
`crossing_refused` record the server writes after it, so one crossing
answers which program made it without a search back to the session's start.
`title` and `protocol_version` are present only where the client sent
them; `negotiated_protocol_version` is present on every such record. A
client that sends no
`clientInfo` leaves no `client_identified` record and no `client` field on
its crossings; the absence is the fact, and no default name is put in its
place. Observed and reconstructed crossings never carry the field: no MCP
client made them.

Names recorded from the hosts probed: `claude-code` with the title
`Claude Code` (every Claude Code session recorded on the owner's machine,
10 of 10, Claude Code 2.1.270 to 2.1.278), `codex-mcp-client` (the Codex
CLI at 0.154.x and the ChatGPT desktop app at 0.153.x, told apart by
`version`), `claude-ai` and `local-agent-mode-commonmeasure` (Claude
Desktop's two clients), `cursor-vscode` (Cursor). The relay projects
neither the record nor the field; what a receiver learns about the host is
`contextops-host-tool`.

## Credential provenance

The MCP server loads `$COMMONMEASURE_HOME/credentials.env` at start
(`crates/commonmeasure-supply/src/credentials.rs`) and, when a file was actually
there, records what it contributed before the first record a tool call
leaves:

```json
{
  "session_id": "…", "host": "claude-code", "timestamp": "…",
  "credentials": {
    "path": "/home/op/.commonmeasure/credentials.env",
    "present": true,
    "sha256": "sha256:…",
    "applied": ["EXA_API_KEY"],
    "shadowed": ["TAVILY_API_KEY"]
  }
}
```

`applied` names the variables the file supplied; `shadowed` names the ones
the launching environment already held, because the environment always wins.
The record carries only the file's path, digest and variable names; a
credential value never enters any record. A session whose log holds
crossings but no `credentials_loaded` event ran on the launching environment
alone: the absence of the event also answers the provenance question, and a
session with no tool call or host observation does not open a log for this. Because the
inference rests on the record always being written, a server that cannot
append it fails that tool call before fetching anything, naming the log,
and tries again at the next call.

A hosted service configured for supplier credential custody
(`docs/contracts/supplier-credentials.md` §Edge side) also holds credentials
its hub released. Where it holds any, the record is written whether or not a
file was present, and gains a `hub` member naming each released credential in
use:

```json
"credentials": {
  "path": "/home/op/.commonmeasure/credentials.env",
  "present": false,
  "hub": [
    {"connection_id": "…", "provider": "exa", "variable": "EXA_API_KEY",
     "fetched_at": "2026-09-19T10:00:00.000Z"}
  ],
  "hub_shadowed": [
    {"connection_id": "…", "provider": "tavily", "variable": "TAVILY_API_KEY"}
  ]
}
```

`fetched_at` is when the fetch that last served the credential started, so
a credential kept through an unreachable hub shows the age of its release. A
released credential whose variable the environment or the file already holds
is not used; it is listed under `hub_shadowed` with its connection id.
`shadowed` stays what the file section above defines, a list of variable
names, and is present only where a file was.

A released set is used for at most three fetch intervals after the last
fetch the hub answered with a release (900 s at the default interval). The
age is checked where a value is read, so an unreachable hub, a fetch that
cannot be tried and a fetch that has stopped running all end alike. Past
that age `hub` is empty, nothing is listed under `hub_shadowed`, and each
credential is listed under `hub_expired` with the bound it passed:

```json
"hub": [],
"hub_expired": [
  {"connection_id": "…", "provider": "exa", "variable": "EXA_API_KEY",
   "fetched_at": "2026-09-19T10:00:00.000Z", "max_age_seconds": 900}
]
```

A search for that provider is then refused as any unconfigured provider is,
by its variable's name.

The released set can change while a session runs. Before a crossing made
under another set of connections in use, shadowed or expired than the last
record named, the record is written again, so a session may hold more than
one `credentials_loaded` and the latest before a search names what that
search could use. No record, `context_status` answer or error carries a
released value. Fixture-tested; no hub has served a release to this code.

## Turn boundaries

The hooks at prompt submission and at the end of a turn each append a
boundary:

```json
{
  "session_id": "…", "host": "claude-code", "timestamp": "…",
  "turn_id": "<the host's prompt or turn identifier>",
  "privacy_level": "minimal",
  "privacy_basis": "the boundary records the host's turn identifier, the working directory and the transcript path; it holds no query text, response text, intent or summary, so minimal is the only level it can declare",
  "policy_identity": "sha256:…",
  "detail": {"cwd": "/home/operator/code/project", "transcript": "…"}
}
```

The record also carries `policy_identity`, or `policy_unavailable` where
the hook could not read the policy (§Policy identity).

`turn_id` is the host's identifier, absent when the host sends none and
never invented. An observed crossing in the same turn carries the same
value, which is what makes a crossing without its turn's question useful: a
reader groups the crossings under the boundary, sees when the turn began
and ended, which directory it ran in and which host transcript holds the
question, and reads the question there under the host's own access rules if
authorised. The question itself never enters this log, and no record here
would be more useful for carrying it: the operator's questions (which
sources, at what cost, under which policy, for which principal) are answered
by the crossings, and the compliance reader who needs the prompt has the
transcript path.

`privacy_level` is declared in the Content Telemetry vocabulary
(`schema/telemetry-session.v1.json` `PrivacyLevel`: `full`, `summary`,
`intent`, `minimal`) and is always `minimal`, because that is the only level
whose claim the record can honour: the higher levels carry the query text, a
summary, or a classified intent, and this record carries none of them. It is
a constant of the edge rather than a policy field, since a declared level the
record could not honour would be a false claim; a projection that carries a
turn event copies the level and may only lower what it carries. The basis is
on the record so a reader needs no other authority.

## Host process

On the hosts that pass no session identifier to the MCP server (Claude Code
and Codex among them), a hook and the server write to two logs: the hook
takes the host's identifier and the server mints `local-<milliseconds>-<process id>`
(§Where). The two were started by one host process, and that process is the
join. At session start the hook records `host_process`; at server start the
MCP server records the same. Each walks up from its own parent to the first
ancestor that is not a shell or a launcher wrapper (`sh`, `bash`, `zsh`,
`dash`, `fish`, `ksh`, `commonmeasure-launch`, the `disclaimer` helper
Claude Desktop wraps a server in; `crates/commonmeasure-harness/src/host_process.rs`
`WRAPPERS`) and records that process:

```json
{
  "session_id": "…", "host": "claude-code", "timestamp": "…",
  "path": "hook",
  "writer_pid": 99796,
  "host_process": {"pid": 94522, "started_at": "2026-09-17T22:55:48Z", "command": "claude"},
  "basis": "the first ancestor of the writing process that is not a shell or launcher wrapper, read from the operating system's process table when the record was written"
}
```

`path` is `hook` or `mcp`. `writer_pid` is the process that wrote the
record. `started_at` is the ancestor's start time as the operating system
reports it, to the second; a `pid` alone is not an identity, because the
system reuses numbers, and the pair is. `command` is the ancestor's
executable name only, never its arguments, which can carry a prompt. Where
the process table cannot be read, or the walk reaches the root of the tree
without leaving shells, `host_process` is `null` and `unavailable` names
why; nothing is guessed ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §7).
`basis` is on the record either way. The table is read on macOS and Linux;
on any other platform the record is written with `unavailable` naming the
platform.

The hook writes the record after the other session-start records and, like
every hook record, best-effort. The stdio MCP server writes it when it
starts, beside `policy_sync` and `edge_identity` and also on an edge that
has neither, and a failed append is a failed start. A server that is
started and asked for nothing therefore leaves a log holding its start
records only, which is what the fourth state below reads. A hosted session
writes no `host_process`: its client runs in a vendor's cloud, and the
service's own parent is a supervisor and not a host.

Two logs on one edge whose `host_process` records agree on `pid` and
`started_at` were written under one host process, and a reader treats them
as one **host session**: the hook log holds the turns, prompt sources,
snapshots and observed crossings; the MCP log holds the mediated and
refused crossings and the client identity. A log without the record joins
nothing; a reader shows it on its own and says the join is unavailable,
never groups it by working directory or timing. The join says nothing about
which turn a mediated crossing belongs to; that remains timestamp order
between the boundaries. On a host that runs several agents in one process
(Codex's app server starts one MCP server per thread; Claude Code's
subagents share the parent's process) the join names the host process, not
the thread or the subagent, and a reader must not present it as one. The same
holds over time: a host that begins a new session identifier inside one
process (Claude Code after `/clear` or a resume) leaves several hook logs
naming one process, and all of them join with that process's MCP log.
Instance identity is the [instance registration contract](instance-registration.md)'s,
which never reuses a process id; `host_process` is local liveness evidence
and nothing more.

`commonmeasure session`, given either log's identifier, reads the joined
logs, so the witnessed crossings it compares against a context snapshot
(§Context snapshots) include the mediated crossings the other log holds.
It orders the records of the two logs by their timestamps, keeps each
log's own order where timestamps are equal, and prints each log it joined
and the process it joined on, or that it joined nothing and why.

The liveness probe is `crates/commonmeasure-harness/src/host_process.rs`
`is_present`: whether a process with this `pid` and this `started_at` is in
the table now, or no answer where the platform's table is not read.

A host process present in the process table is **running**. Present, with a
turn boundary or a crossing recorded, is **seen working**. Absent from the
table with no `session_ended` is **gone without a session end**, reported
as that and never as completed. A log whose only records are the ones
written at start (`policy_sync`, `edge_identity`, `host_process`,
`nudge_issued`, `hosted_scope`) is **configured, never seen working**.

A reader joins on a log's first `host_process` record. A session the host
resumes under a new process writes a second record with the new pair; it is
recorded, and a reader that joins on the first alone says so. Reading the
later records is not yet implemented.

The end of a session is recorded where the host reports it:

```json
{"session_id": "…", "host": "claude-code", "timestamp": "…", "reason": "clear"}
```

`reason` is the host's word, recorded as given (Claude Code's `SessionEnd`
supplies one such as `clear`, `logout`, `prompt_input_exit` or `other`) and
absent when the host sends none. A crash sends no hook, so the absence of
`session_ended` is not evidence that a session continues; the process table
is.

## Content Telemetry projection

The relay emits Grounding-level events only: `content_retrieved`,
`content_grounded`, `turn_started` and `turn_completed`. It emits no citation,
presentation, reproduction or engagement claim. The pinned schemas are listed
in `schema/SOURCE.md`; `crates/commonmeasure-relay/src/project.rs` implements
this selection, and `conformance/` freezes the emitted batches.

### Selected coverage

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
configured receiver. A witnessed session crossing requires a matched policy
scope, a named governing engagement and explicit telemetry clearance. Current
directory consent and managed grants also apply where directory reporting is
configured. A run explicitly named to the relay contributes its admitted
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

### Instance reference

A `content_retrieved` or `content_grounded` event whose source record carries
the `instance` reference (§Where) carries it as an event-level member, beside
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

### Ingestion measure

A grounding event carries `tokens_ingested` only when its source record has a
non-negative integer count and a non-empty recorded `token_basis`. The relay
copies both, preserving `token_basis` as a custom event `data` field retained
from the existing wire format. It is not a standard field. Session counts come
from `payload.estimated_tokens` and `payload.token_basis`; published run counts
come from each admitted source's `tokens` and `run.token_basis` in `summary.json`.
The projection never substitutes a current estimator for a missing recorded
basis. An absent count or basis leaves both fields absent, and a recorded zero
with a basis remains zero. `characters/4` and `whitespace-words` are estimates,
not model-tokeniser counts or measurements comparable between emitters.

The relay does not emit `chars_ingested`: it reads hashes and counts from the
record, not the captured text. Multiplying a rounded token estimate would not
recover the Unicode code points placed in context. Existing `content_hash`
continues to identify the captured content; page text never enters telemetry.

### Delivery state

The relay retains each projected batch in `relay/spool/outbound.ndjson`.
`queued_at` records when it entered the spool; older batches have an unknown
queue age. Each line carries its spool `index`; a line written before indices
were stored takes its line number. Private metadata records `queued`,
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
written, and the next writer removes it. One case is kept: an unterminated
final queue line that parses as a whole entry is given its newline, since a
restored file may end that way; if it was an interrupted enqueue, its event
ids count as spooled and nothing is projected twice. Every queue line is
checked to be a whole entry, and every journal line parsed, whenever the spool
is read, so damage before the last line of either file is an explicit error
from the relay, `relay requeue`, a dry run and status, and no line is skipped
or removed. One damaged queue line therefore stops the delivery of every
queued batch until it is repaired. Removing the line does not repair it: the
metadata still names its index. The number in the report is the batch's spool
index: the line's `index` member or, for a line without one, its line number
counted from zero, so the first line is 0. The batch's metadata is the snapshot
member and the journal records that carry that index. For a damaged line of a
delivered batch, replace the line with a placeholder that keeps its index, for
example
`{"index":7,"origin":"damaged line replaced","document":{"events":[]}}` for
spool index 7; the spool then opens, delivery resumes and the placeholder is
pruned under the retention rule below. For a damaged line of an undelivered
batch the payload is lost. While no relay runs, empty the line and keep its
newline, and remove the batch's entries from the snapshot and the journal. Do
not delete the line: a line with no `index` member takes its index from its
line number, so deleting a line above it would give it, and every such line
after it, the metadata of the batch before, and a queued batch could read as
delivered with no error. Every release to v0.3.3 wrote lines without the
member, and they stay so until a prune rewrites the file. Then run the relay,
which projects the events again where the session log still reads and the
clearance in force admits them. A damaged line that has lost the start of its
`index` member can be reported as out of order, by its line number counted
from zero; the remedies are the same, and where the snapshot or the journal
has an entry for the batch, its index is the one no other line carries.
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
attempt withheld (§Instance reference); the field is absent on deliveries
recorded before delivery withheld the member. A batch with a count above zero
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
journal exists. Batches below an existing `outbound.ack` offset are imported
as accepted only where `delivered.idx` records every event id in a non-empty
batch; otherwise they remain queued. Subsequent acceptances are recorded per
batch.

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

Only the receiver's documented acceptance (HTTP 200 or 201 with JSON
`status: "ok"` and an unsigned `events_created` count) marks a batch delivered.
A zero count is valid for a redelivery. Acceptance of a later batch does not
acknowledge earlier queued or dead batches. Later batches may arrive before
earlier ones, including within the same session; receivers deduplicate event
ids and retain the highest refused count. A crash after acceptance but before
local acknowledgement may send the same event identifiers again. Undelivered
payloads, dead batches included, remain on disk. Current directory consent and
source policy are checked when a batch is due. The relay sends the cleared subset and records
acceptance for that delivery, leaving withheld event ids out of `delivered.idx`.
Private batch metadata records `delivered_subset: true` when the accepted
attempt omitted events from the retained batch, and `false` when it included
all of them; the field is absent on older deliveries whose subset is unknown.
Those ids can be projected again if clearance returns. If no events remain
cleared, the batch stays queued with a separate hold reason and consumes no
HTTP attempt; the last delivery error is preserved. Repeated unchanged holds
do not rewrite metadata. A held batch is rechecked on later due invocations.

Queued, held and dead batches are undelivered. A held reporting obligation has
no automatic grace period. `commonmeasure status`, `commonmeasure doctor` and
the console show queued, dead and delivered **batch** counts, oldest queued age,
next attempt and last error. The existing delivered and pending **event** counts
remain separate; pending includes dead batches. Unreadable delivery state is
reported as unavailable, with unknown counts. Legacy queue ages remain unknown.
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
syncs neither the managed policy nor the directory grants, and it projects
against the policy and the grants already on disk. A real run syncs both
before it projects, and the clearances and internal prefixes it then reads
can differ from the ones forecast, on a managed edge in particular. The
forecast ends with two lines saying what it was taken against — the applied
managed revision and whether it is stale, the local policy file, or the draft
passed with `--policy` — and that a real run refreshes both first. Over an
unchanged home the counts match (open: no field evidence
establishes which of the two caused the gap of 22 September 2026).

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
refreshes a directory grant or writes to the relay directory, so a second run
started at the same moment exits before doing any of those. The command syncs
managed policy before the relay takes the lock, so on a managed edge the
losing run can still fetch the policy envelope and rewrite `policy.json`
before it exits. Do not run an older
relay against a spool after this metadata has been written: older versions read
only the prefix acknowledgement and cannot honour per-batch state, and a
version from before the journal ignores it and numbers batches by line. Such a
version sends again every batch whose acceptance is only in the journal. After
a prune it refuses the spool where the snapshot names a line number the
shorter queue lacks, and otherwise applies each retained state to the batch
now on that line and gives a new batch a line number an earlier batch held.

The API key in `relay.json` belongs to the receiver in `relay.json`; on an
enrolled edge it is the hub's ingest key. The relay sends it as `X-API-Key`
only to that receiver's origin, compared as the instance issuer is compared
(§Instance reference). A delivery to another receiver named with `--receiver`
carries the key given with `--api-key`, or no `X-API-Key` header. The enrolled
key's standing is asked of the enrolled hub under the key whose receiver has
the hub's origin, so a key given with `--api-key` for another receiver is not
sent to the hub. The other requests an enrolled edge makes to its hub under
the stored key follow the same rule: the directory grant refresh, the
directory proof refresh outside a relay run and `disconnect` send the
`relay.json` key only where the receiver in `relay.json` has the origin of the
hub in `enrolment.json` (`EnrolmentRecord::hub_ingest_key`), and otherwise
report that no ingest key is held for the hub and send nothing. A dry run
sends and prints no key.

### Relay at session end

A local edge relays when a session ends, as well as when `commonmeasure
relay` is run. The `session-end` hook, after recording `session_ended`,
starts `commonmeasure relay` as a separate process when `relay.json` names a
receiver and the marker file `relay/manual` is absent, and starts nothing
otherwise. The run is a whole relay run: every session log and every due
spooled batch, because the MCP server a host starts writes its crossings
under its own session identifier and the hook's session holds only part of
the work. Clearance, grants, backoff and the spool lock apply as to any run.

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
(§Source declarations). The hosted service honours the same marker: its
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

### Verification

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
`crates/commonmeasure-relay/tests/conformance.rs` validates the corpus against
the pinned schemas, including rejection of missing or invalid turn privacy
levels. A receiver must re-pin the changed corpus and run its own replay gate.

## Context snapshots

The host's context report is an observation with a named basis, not ground
truth inferred from transcript size. At each `Stop` boundary on Claude Code,
the hook reads the host transcript's most recent model call (the provider's
own usage counters, which the host records beside each call) and appends:

```json
{
  "session_id": "…", "host": "claude-code", "transcript": "…",
  "basis": "host transcript usage counters: API-reported token usage of the session's most recent model call, read at the Stop boundary",
  "model": "<the model the host reported>", "observed_at": "…",
  "context_tokens": 12000, "input_tokens": 20,
  "cache_read_input_tokens": 11800, "cache_creation_input_tokens": 180,
  "output_tokens": 40,
  "inventory": {
    "basis": "host transcript records up to the same boundary: each attachment record the host injected into context, measured as the character count of the host's serialised record; each message text block, tool input and tool result, measured as the character count of its text; both divided by four; an estimate, never a tokeniser's count, cumulative since the last compaction",
    "token_basis": "characters/4",
    "categories": {
      "acquired_content": {"records": 13, "estimated_chars": 22884, "estimated_tokens": 5721},
      "agents": {"records": 1, "estimated_chars": 2780, "estimated_tokens": 695},
      "conversation": {"records": 171, "estimated_chars": 353636, "estimated_tokens": 88409},
      "instructions": {"records": 170, "estimated_chars": 58104, "estimated_tokens": 14526},
      "local_files": {"records": 2, "estimated_chars": 832, "estimated_tokens": 208},
      "other_host_records": {"records": 0, "estimated_chars": 0, "estimated_tokens": 0},
      "skills": {"records": 1, "estimated_chars": 8712, "estimated_tokens": 2178},
      "system_prompt": {"records": 1, "estimated_chars": 12860, "estimated_tokens": 3215},
      "tool_definitions_loaded": {"records": 2, "estimated_chars": 9704, "estimated_tokens": 2426},
      "tool_listing": {"records": 1, "estimated_chars": 5028, "estimated_tokens": 1257}
    },
    "available_on_demand": {"tools": ["WebFetch", "WebSearch", "…"], "skills": ["…"], "agents": ["Explore", "…"]},
    "definitions_loaded": ["WebFetch", "WebSearch"],
    "invoked": {"Bash": 54, "WebFetch": 10, "WebSearch": 5, "…": 0},
    "compactions": 0
  },
  "unavailable": ["context_limit", "free_capacity", "resident_tool_definitions (the built-in tool schemas travel in the request, not the transcript)", "memory (auto-memory is part of the system prompt and not separable from it)", "cache state per category"]
}
```

`context_tokens` is arithmetic over reported counters (the uncached tokens
plus both cache legs), not an estimate; it is what the provider reported
reading for that call. The basis is in the record, so the number cannot be
read as a tokeniser's measurement of the window. What this basis cannot see
is listed in `unavailable` and not estimated
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §7): the host exposes no context limit and no free
capacity at hook boundaries, the resident tool schemas travel in the request
rather than the transcript, and auto-memory is part of the system prompt.
Only counters, the model name, a timestamp, record kinds, tool and skill
names and text lengths are read from the transcript; no conversation text is
copied. A transcript
that cannot be read, or a host without this transcript shape, produces no
snapshot, not a snapshot of zeros.

The `inventory` is a second observation on a second basis, kept apart from
the counters. The host writes one `attachment` record into its transcript
for each piece of scaffolding it injects into the window, and the reader
files each record, and each message block and tool result, under a named
category: `system_prompt` (the host's own prompt snapshot, where the host
version writes one; otherwise listed unavailable), `instructions`
(instruction files, environment, model and session notices, reminders),
`skills` (the skill listing), `agents` (the subagent listing),
`tool_listing` (the names of the tools loadable on demand),
`tool_definitions_loaded` (definitions that entered context on demand),
`local_files` (files and directories the host read into context),
`other_host_records` (attachment kinds this reader does not know, counted
under their own name rather than filed as instructions), `conversation`
(prompts, answers, commands the person queued, tool inputs and the results
of tools that acquire nothing) and `acquired_content` (the results of the
host's fetch and search tools and of MCP tools, which is the text the
crossings hash). An attachment's footprint is the character count of the
host's serialised record, divided by four; a message block's, tool input's
or tool result's is the character count of its text, divided by four. Both
are explicit estimates with the basis on the record, never a tokeniser's
count, and never added to the API-reported counters. Thinking blocks are
not counted, because the host does not report whether it sends earlier
turns' thinking again.

The three states of a capability are recorded apart, because a discoverable
tool is not one whose definition entered context or whose capability was
invoked: `available_on_demand` holds the names the host listed (tools,
skills, subagent types), `definitions_loaded` the tools whose definition the
host recorded entering context on demand, and `invoked` every call the
transcript records, by tool name. A name in the first list and neither of the
others was offered and never used.

A compaction rebuilds the window, so every footprint and listing starts
again at one and `compactions` counts how many times that happened; the
host writes a boundary line and then the summary message, and the two are
one compaction. Invocation counts survive, because a call that happened
still happened.

`commonmeasure session` prints the last boundary's inventory and, between
each pair of consecutive snapshots, the API-reported change in
`context_tokens` beside the estimated growth of host scaffolding (the seven
categories that are not conversation or acquired content), of conversation
and of acquired content, the witnessed crossings recorded between the two
boundaries, and the remainder the estimates do not account for. The
remainder is printed rather than absorbed into a category: the estimates
are on their own basis and do not explain the provider's count. A boundary
with no counters, or whose inventory carries different categories from its
neighbour's, is printed as not comparable; nothing is compared against a
zero that was never measured.

`commonmeasure session` also prints the snapshots beside the session's witnessed
acquisition (the sum of `estimated_tokens` over observed and mediated
crossings, on its own `characters/4` basis) as a lower bound on acquired
input; it does not claim that sum explains the host's totals, which include
everything the host assembled.

Two lags are inherent to the basis. The host flushes each assistant message
to its transcript after the `Stop` hook fires, so each boundary observes the
turn before it, and a one-turn session records no snapshot. The host also
does not announce every boundary: `/clear` starts a new conversation and
session, so the old log ends; its final model call may never be observed,
because no later `Stop` occurs in that session. Compaction keeps the session
and shrinks the context in place, so it appears as an otherwise-unexplained
drop between consecutive snapshots and not as its own record. The snapshots
are boundary observations; this basis cannot claim what happened between two
of them.

## Policy synchronisation

On a managed edge ([`docs/contracts/policy-envelope.md`](policy-envelope.md)
§Deployment mode) the policy is refreshed from the hub at session start,
and the session log records what the refresh did:

```json
{
  "session_id": "…", "host": "claude-code", "timestamp": "…",
  "trigger": "session_start",
  "policy_url": "https://hub.example/api/v1/policy/desired",
  "outcome": "already_applied", "revision": 2, "digest": "sha256:…", "reason": null,
  "applied": {"revision": 2, "digest": "sha256:…", "expires_at": "2026-09-21T01:40:58Z"},
  "stale_since": null
}
```

`trigger` is `session_start` when the session-start hook ran it and
`server_start` when the MCP server did, which it does for any session whose
log carries no refresh when it starts, whatever the host; a session is
refreshed once. The refresh precedes every other record of the session, so
the `nudge_issued` record and every crossing name the policy the refresh
left in force. The
outcomes are the ones the envelope contract enumerates, plus `unavailable`
when the deployment or state file did not load and nothing was asked of the
hub. `applied` is the revision in force after the refresh, whatever the
outcome, and `stale_since` is its envelope's expiry once that has passed:
the policy stays in force and the record says since when it has been stale
([`docs/contracts/policy-envelope.md`](policy-envelope.md) §Cadence and
staleness). A local edge writes no such record, because it makes no
management request. The refresh the relay runs before delivery writes to
the managed state file, not to any session.

## The agent id on the wire

The relay initially takes `agent_id` from the session's first `edge_identity`
record, or uses `commonmeasure` when there is none. It pins that value per
wire session in `relay/session-agents.json` before the first delivery attempt,
so every later batch and retry keeps the same id even when the session resumes
or compacts after enrolment. Retained spool batches supply the first id for
sessions delivered before a pin exists; a relay run with nothing to deliver
keeps all pins. The pin is private relay state and adds no wire field.

## The refused count on the wire

The relay never projects a refused crossing: no URL, no reason, no hash of
the bytes it withheld. What it projects, on every batch of a session it
delivers, is `refused`: the number of `crossing_refused` records in that
session whose working directory resolves to a scope cleared for egress, as
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
event to carry it, the relay sends the session's last delivered event
again, under its own id, on a batch carrying the new total: the receiver
already holds the event and takes the larger count, so a refusal after the
session's last admitted crossing still crosses. The relay keeps the highest
count each receiver has accepted per session (`relay/refused-delivered.json`)
and its summary states the totals it put on the wire this run and nothing
more. A session that was refused and admitted nothing produces no batch and
its count does not cross; a batch without the field comes from an edge that
does not report it, which is not the same as a count of zero.
`conformance/session-refused.json` is the vector.

## What is deliberately absent

- **Prompts, responses and conversation text.** Records carry identifiers and
  hashes. The transcript path is noted on a turn boundary; its contents are not
  copied.
- **Local and private addresses** from observed capture: loopback, RFC 1918,
  `.local`, `.internal` and `file://`. Passive capture sees everything and
  never asks, so it holds a floor the operator cannot lower **except by named
  prefix, and never by omission**: `record_internal_prefixes` in `policy.json`
  lists internal prefixes (`"https://rag.example.internal/"`,
  `"file:///corp/kb/"`) whose crossings are recorded with the same grades,
  hashes and token estimates as public traffic. Each prefix must end with
  `/`, because the slash is the consent boundary: a prefix without it would
  also match hosts and paths that start with the same characters, so the
  loader refuses it. The operator owns that corpus, and a named prefix is
  written consent. Anything the list does not match stays out (naming one
  corpus is not consent for localhost as a whole), and an absent or empty
  list means the floor holds everywhere. The named prefixes appear in
  `context_status`, so the console can say what is being recorded and on
  whose authority. The mediated tools honour the same prefixes, and
  `allow_private_hosts` is the broader grant, because there the agent named
  one URL deliberately. Reconstructed import keeps the unconditional floor: a
  prefix names consent from the moment it is written, and it does not
  retroactively cover transcripts nothing was watching at the time. Internal
  crossings are operator-record only; any egress
  projection must treat internal URLs as private by default. The
  classification is stamped on the record at capture (`"internal": true`) and
  egress honours the stamp, so a prefix later edited or removed cannot make a
  crossing recorded as internal leave the machine. The content view
  (`/api/content`) serves an internal-stamped crossing with an explicit
  `"internal": true` marker, because it is part of the record; its context
  footprint is listed in the internal-use view (`/api/internal`) alone.
- **This plugin's own MCP tools** (`mcp__commonmeasure__*`) from observed capture.
  The server that carried them records them, and observing them again would
  log each crossing twice and extract every URL from the page body that was
  returned.
- **Engagement labels**, governing or reported. The log records the cwd, a
  fact, and each identity is resolved from it by the reader that needs it:
  the relay from `policy.json`, the sink from `attribution.json`. A label
  written into evidence would have to be rewritten when a rule changes, and
  evidence is never rewritten. The
  `cwd` is a path on the operator's own machine and stays in the
  private record: the Content Telemetry projection selects fields by allowlist
  and does not carry it.
- **Any evaluation.** No evaluator runs against a session. Batch-run evaluation
  is [`docs/contracts/run-output.md`](run-output.md) §Plans.

## Reading it

```sh
commonmeasure session            # the most recent session
commonmeasure session <id>       # a named one
commonmeasure serve              # the session view of the console, on loopback
```

## Conformance

`crates/commonmeasure-cli/tests/embedded_credentials.rs` provides fixture-tested
acceptance through the real MCP binary and HTTP transport over loopback:
signed text and HTML verify before removal, tampering remains invalid, gzip
retains the coded hash, refused content retains verification, and malformed,
unsupported and externally linked credentials never acquire trust. The
fixtures use ephemeral signing identities; they establish no publisher trust.

`crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the payload
shapes a host sends. `crates/commonmeasure-cli/tests/mediated_e2e.rs` drives the MCP
server over stdio against a real loopback origin. Between them they assert the
hash covers the ingested text, search results do not ground, subagent identity
survives, private addresses stay out, own tools are not double-recorded, the
client's `initialize` name and version are recorded once before the first
crossing and stamped on each mediated crossing and refusal, a client that
sends none leaves neither, each requested protocol revision is answered as
version negotiation states, an initialised server that is asked for nothing
records its host process and nothing else with or without a credentials
file, the server's session
identifier comes from `--session`, then `AGENT_SESSION_ID`, then itself, an
identifier that is not a plain name records nothing from any reader, a
page served under gzip carries `retrieved_hash` over the coded bytes, a
refusal reaches the agent and the log, no payload makes a hook fail, a turn
boundary declares `minimal` and carries the host's turn identifier without
the prompt, a Stop boundary records the inventory with the three capability
states apart and the session report attributes the change between two
boundaries, and the standing nudge is delivered and its issuance recorded at
session start, with delivery surviving an unreadable payload and an
unwritable home. `crates/commonmeasure-cli/tests/recorded_sessions.rs` reads
real recorded host sessions, which are not published, and pins the report
over them, and the snapshot reader is tested over that Claude Code session's
transcript with its text replaced by same-length filler
(`crates/commonmeasure-harness/tests/recorded/claude-code-transcript.jsonl`).


## Hosted scope

A service configured with `session_directory` writes one `hosted_scope` record
when it opens a session's file, before any crossing. Its payload carries
`session_id`, `host`, `timestamp`, `basis: "service_configuration"` and the
canonical `directory`. It attests the operator's declaration only. The selected
root must already be locally enrolled under the service account; neither
configuration nor OAuth consent approves reporting. The existing directory
consent, signed grant, source-policy veto and privacy filter still apply.
This record and its path never enter Content Telemetry.

## Directory reporting consent

After directory enrolment, the relay requires current local root permission
and, on a managed edge, its current signed grant, alongside source-policy
clearance and the privacy floor. The check applies to new projection and queued
batches immediately before delivery. Opt-in includes existing eligible witnessed
evidence; opt-out preserves the original record and does not recall deliveries.
No field is added to Content Telemetry. See
[directory enrolment](directory-enrolment.md).

## Local comparison records

Console retrieval comparisons use `contextops-comparison/1` in
`<home>/comparisons/compare-<uuid>.ndjson`. They use the same durable NDJSON
writer and mediated crossing/processor shapes as sessions, but the relay scans
only `sessions/`: comparison queries and results have no implicit Hub egress.

`comparison_started` binds query bytes and SHA-256, ordered selected providers,
requested/effective limit (five), Edge home/directory, effective canonical
policy and identity, and process principal/authentication basis.
`comparison_provider_started` precedes each supplier attempt;
`comparison_provider_finished` retains its terminal outcome. A successful
receipt records received/admitted/refused counts, recordable result metadata
and refusal reasons, charge, adapter request latency, total elapsed time,
endpoint, HTTP status where present, adapter version and response hash.
Snippet text and raw response bodies are not retained in this summary.
Unknown charges and missing request latency remain null/unknown. The existing
private-address recording floor also applies to result metadata.

`comparison_finished` is written only after every selected provider has a
recorded outcome. Completion means the attempt is accounted for; individual
providers can be refused or unavailable. Without this event the reader shows
an incomplete comparison. A started request without a recorded receipt may
have incurred supplier charges. Evidence failure stops later providers and
returns an unavailable result, never an unqualified success. These local
records are neither sealed batch runs nor a Hub summary upload format.

The local JSON download contains private evidence. The separate
[comparison export](comparison-export.md) projects permitted measurements into
an offline report bundle, with explicit query inclusion and no supplier rerun.

## Provider policy ruling

`provider_policy_ruled` records a provider or principal policy breach before
MCP search dispatch. It names the session, host, tool, provider, outcome
(`refused` or `allowed_with_breach`) and reason, without the query or a source
URL. A strict refusal sends no request. Observe/prefer breaches are also
returned by the tool and carried on delivered source records.
