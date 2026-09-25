---
domain: edge
audience: integrator
---

# Conformance corpus

Golden Content Telemetry v1.0 `event_batch` documents: the relay's **actual
output**, frozen as files, so the wire between edge and receiver has an
executable definition instead of a prose one. The pinned schemas in `schema/`
say what a valid document looks like; this corpus says what this edge sends,
and what any receiver must accept.

Two gates hold the contract, one per side of the wire:

- **This repository** (`crates/commonmeasure-relay/tests/conformance.rs`) proves the
  corpus is byte-identical to what `commonmeasure_relay::project` emits from fixed
  ledger inputs, and that every document validates against the pinned
  schemas. The relay cannot drift from the corpus without CI saying so.
- **The receiver** (Common Measure Hub, in its own repository and its own
  conformance test) replays a pinned copy of these files into its real
  router and database, and checks that the hub answers each batch `201`
  with `events_created` equal to its event count on first delivery and `0`
  on full redelivery, and keeps the larger `refused` count. The receiver
  cannot drift from the corpus without its CI saying so.

Either repository's CI fails when the wire changes.

The relay treats any `2xx` answer as acceptance and needs no response body;
a JSON body with an unsigned `events_created` supplies the count of newly
recorded events (`docs/contracts/telemetry-projection.md` §Delivery state).
A receiver conforms by accepting these documents; the Hub's response
format is not required.

## Changing the wire

A deliberate wire change is made edge-first:

1. Change the projection or wire types; keep
   `crates/commonmeasure-relay/tests/conformance.rs` green against the pinned schemas.
2. Regenerate the corpus: `UPDATE_GOLDENS=1 cargo test -p commonmeasure-relay --test
   conformance`, and review the diff, which is the wire change.
3. Re-pin the hub's copy of this corpus and make its replay test green in
   the same hub change.

Each document is stored as the canonical JSON of the batch
(`docs/contracts/canonical-json.md`), indented for a reader: the member order
is the canonical one, so the frozen bytes cannot move because a dependency
changed how a JSON map is ordered. The session and event identifiers are
UUID version 5 over the canonical text of a pre-image naming the record each
id stands for, so a receiver recomputes one with a canonical serialiser and
nothing else.

The corpus is small on purpose: each document exercises wire surface the
others do not.

| Document | What it exercises |
|---|---|
| `session-grounded.json` | retrieval and grounding events on a session, with the grounding `data` fields and a declared licence; a derived session id; an enrolled edge's key id as `agent_id`, with the host tool in its namespaced `data` field; a retrieval carrying the `content_telemetry_id` of the fetch's `Content-Telemetry-ID` header |
| `session-turns.json` | turn boundaries: recorded turn IDs and `turn.privacy_level: minimal`, with no source URL on a turn event |
| `session-refused.json` | the batch-level `refused` count, for a session refused twice and admitted once |
| `session-supplied.json` | the supplier that served a mediated search result, in its namespaced `data` field |
| `session-registered.json` | a session registered part-way through and renewed once, projected for the hub that issued the instance: content events carry the event-level `instance` member at each record's revision, and the crossing before the registration carries none (`docs/contracts/telemetry-projection.md` §Instance reference) |
| `session-paced.json` | the `Crawl-delay` a retrieval was sent under, in its namespaced `data` field, on the retrieval of a page that waited its turn and not on its grounding; a host stating `Crawl-delay: 0` carries none (`docs/contracts/telemetry-projection.md` §Custom fields) |
| `run-licensed.json` | a run projection with the emitter id `commonmeasure` as `agent_id`, a declared licence, and ingestion counts with their recorded `token_basis` beside `tokens_ingested` |
| `run-supplied.json` | a run whose plan acquired through a supplier, named in its namespaced `data` field |

Every session batch carries `refused`: the session's running total of
crossings policy refused at the time the batch is projected, an integer and
nothing else about them. A receiver keeps the larger value on redelivery
rather than summing. Every event declares selected coverage and Grounding
level in `data.commonmeasure-projection`; the selection rule is
`docs/contracts/telemetry-projection.md` §Selected coverage.

Behavioural edge cases, such as batch splitting at the 500-event cap and the
privacy floor, stay in each side's own test suite; the corpus defines the
shape of what crosses, not everything about how each side behaves.
