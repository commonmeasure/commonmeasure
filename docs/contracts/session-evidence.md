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
`~/.commonmeasure/sessions/<session-id>.ndjson`. The host supplies the session
identifier; the MCP server generates one when it does not.

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
| `edge_identity` | a hook at session start (Claude Code: `SessionStart`), on an enrolled edge | the identity this edge runs under: the hub, the key id the hub assigned at enrolment, and the key's standing (`enrolled`, or `revoked` with when and by which side). Absent on an edge that is not enrolled |
| `credentials_loaded` | nothing; the MCP server writes it at start | the operator credentials file was loaded at server start: its path, digest and variable names, never a value |
| `allowance_gap` | nothing; the MCP server writes it | the allowance ledger did not record a settlement or release for a mediated search: the reservation id, the observed charge where a receipt reported one, and the reason; the reservation stays held until the expiry sweep releases it |
| `evidence_gap` | nothing; the log writes it | a window this log could not record |

A host-policy refusal happens before any bytes move. A PII refusal happens
after the fetch and before the text is returned: the bytes existed, their hash
is on the refused crossing, and they never entered context (`grounded` is
false). The two records differ because they are different facts, and both
are `crossing_refused` because in both cases the mediated path stopped the
text before it reached the model.

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
  "host": "claude-code",
  "cwd": "/home/operator/code/project", "policy_scope": "code/project",
  "principal": "research-agent", "authentication_basis": "os_user",
  "url": "…", "host_name": "…",
  "identity": {"user_agent": "CommonMeasureBot/0.2.0 (+https://…/bot; mailto:…)",
               "key_id": "…", "signature_agent": "https://…/.well-known/http-message-signatures-directory"},
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
markup ([`docs/contracts/processor.md`](processor.md) §Status, `html-text-extractor`). The
two are equal where the body was delivered as decoded. Where they differ,
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
crossings are trustworthy. Pi has no web tools of its own; the few
`ledger_fetch` results in its history were mediated and recorded when they
happened, and a transcript copy would be a weaker duplicate of an existing
record.

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

`authentication_basis` is `os_user` where the runtime read the process's
effective operating-system user, and `unavailable` where the platform offers no
identity it can read (currently every non-unix platform, including the Windows
binary the release publishes). `unavailable` is recorded rather than omitted, and
it is not a downgrade in enforcement: a policy file declaring principals fails
closed there, refusing every mediated crossing and saying so, and a policy file
declaring none is governed by directory scope alone (trusted principal
resolution, [`docs/GLOSSARY.md`](../GLOSSARY.md) §Principal).

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
would be a claim, not a witnessed fact. Older logs without the field read as
absent.

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
prefers neither (`DECISIONS.md` §Session policy and egress).

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
other and is recorded in the same field. Observed and reconstructed crossings
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
  "robots": {"url": "https://publisher.example/robots.txt", "cache": "fetched",
             "fetched_at": "…", "expires_at": "…", "status": 200,
             "reading": {"group": "CommonMeasureBot", "crawlable": true,
                         "statements": [], "licences": ["https://publisher.example/license.xml"]}},
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
carry is ruled on after. `robots.txt` is fetched once per host and cached for
24 hours, or for the response's `max-age` where shorter, under
`$COMMONMEASURE_HOME/declarations/<host>.json` beside the licence documents it
names (`cache` says `fetched` or `reused`); a probe that failed, or that the
host answered outside 2xx and 404, is cached for five minutes and recorded as
`unavailable` with the status. A 404 is not a failure: the host publishes no
rules. Cache files are written whole and renamed into place. Every probe goes
through the same host policy and address floor as the page, and no probe is
ever recorded as a crossing.

What the operator's mode does with a disallowed `ai-input` statement, the
use a mediated fetch makes of a page: `strict` refuses, before the request
when the statement was known then and otherwise after it, in which case the
bytes were fetched and are withheld from context, their hash on the refused
crossing and `grounded` false; `observe` and `prefer` carry the crossing with
the statement named in `breach`. A `robots.txt` `Disallow` for the selected
group is ruled on the same way. A licence whose AI-input permission is
conditional on a payment, or on a token from a licence server, is a term this
edge cannot meet without a settlement rail, and is ruled on the same way with
the term named. A licence's telemetry reporting demand is met when its profile
is the Content Telemetry binding this runtime speaks, its conformance level
is one this runtime emits (`retrieval` or `grounding`), the session's policy
scope clears telemetry egress and `$COMMONMEASURE_HOME/relay.json` names a
receiver; an unmet demand is ruled on the same way, and a profile this
runtime does not recognise is an unmet demand, as RSL requires. The ruling is
on the record as `declarations.reporting`: the profile and level demanded,
the receiver named or its absence, whether the scope clears egress, `met`,
and the first reason it is not.

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
breach named. No settlement rail pays a quoted price yet, so the receipt
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
fact for the operator's policy and never a carve-out from a preference
(`DECISIONS.md` §Source declarations).

Terms the operator holds for the host (`terms` in `policy.json`, an
agreement referenced by an identifier the operator chooses) govern over any
statement the source publishes: `governing` is `operator_terms`, the
statements are still recorded and none is enforced, and the only duty ruled
on is the reporting the terms themselves require. Terms may name the
institution identifiers the agreement attributes usage to
(`access_context`); they are recorded on the crossing under
`declarations.terms` and require the standard's session-level
`access_context` container on any report. The event batch the relay delivers
has no session data, so a session under such terms is withheld by the relay
and counted as withheld for that reason, until delivery as a session document
is built. `licence.state` is
`declared` with the terms' reference where terms govern, with the licence
URL where an RSL licence's terms were read, and `unknown` otherwise. Reaching
a page is not permission.

## Manifest discovery

After a mediated fetch succeeds, the server resolves the host's Content
Telemetry discovery manifest (`/.well-known/content-telemetry.json`,
standard section 8) and writes a `manifest_resolved` record; the crossing
carries the record's `seq` as `manifest_record`
(`crates/commonmeasure-harness/src/manifest.rs`,
`crates/commonmeasure-harness/src/discovery.rs`). The manifest identifies an
owner and its telemetry endpoint and carries no demand, so discovery never
delays the crossing's ruling and never refuses anything.

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

## Credential provenance

The MCP server loads `$COMMONMEASURE_HOME/credentials.env` at start
(`crates/commonmeasure-supply/src/credentials.rs`) and, when a file was actually
there, records what it contributed:

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
session that never crosses does not open a log for this.

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

`crates/commonmeasure-cli/tests/hook_e2e.rs` drives the real binary with the payload
shapes a host sends. `crates/commonmeasure-cli/tests/mediated_e2e.rs` drives the MCP
server over stdio against a real loopback origin. Between them they assert the
hash covers the ingested text, search results do not ground, subagent identity
survives, private addresses stay out, own tools are not double-recorded, a
refusal reaches the agent and the log, no payload makes a hook fail, a turn
boundary declares `minimal` and carries the host's turn identifier without
the prompt, a Stop boundary records the inventory with the three capability
states apart and the session report attributes the change between two
boundaries, and the standing nudge is delivered and its issuance recorded at
session start, with delivery surviving an unreadable payload and an
unwritable home. `crates/commonmeasure-cli/tests/recorded_sessions.rs` reads
the real sessions committed under `demo/host-sessions/` and pins the report
over them, and the snapshot reader is tested over that Claude Code session's
transcript with its text replaced by same-length filler
(`crates/commonmeasure-harness/tests/recorded/claude-code-transcript.jsonl`).
