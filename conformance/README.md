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
- **The receiver** (Common Measure Hub, maintained separately; its
  `crates/server/tests/conformance.rs`)
  replays a pinned copy of these files into its real router and database and
  requires the frozen acceptance contract: HTTP 201 with
  `{"status": "ok", "events_created": n}` where `n` counts the batch's
  events on first delivery and `0` on full redelivery. The receiver cannot
  drift from the corpus without its CI saying so.

Either repository's CI fails when the wire changes.

## Changing the wire

A deliberate wire change is made edge-first:

1. Change the projection or wire types; keep
   `crates/commonmeasure-relay/tests/conformance.rs` green against the pinned schemas.
2. Regenerate the corpus: `UPDATE_GOLDENS=1 cargo test -p commonmeasure-relay --test
   conformance`, and review the diff, which is the wire change.
3. Re-pin the hub's copy (`crates/server/tests/fixtures/conformance/`, see
   its `SOURCE.md`) and make its replay test green in the same hub change.

Each document is stored as the canonical JSON of the batch
(`docs/contracts/canonical-json.md`), indented for a reader: the member order
is the canonical one, so the frozen bytes cannot move because a dependency
changed how a JSON map is ordered. The session and event identifiers are
UUID version 5 over the canonical text of a pre-image naming the record each
id stands for, so a receiver recomputes one with a canonical serialiser and
nothing else.

The corpus is small on purpose: each document exercises wire surface the
others do not (both event kinds, a derived session id, an enrolled edge's
key id as `agent_id` with the host tool in its namespaced `data` field, the
emitter id `commonmeasure` on a run, a declared licence, the grounding
`data` fields, a retrieval carrying the `content_telemetry_id` of the
fetch's `Content-Telemetry-ID` header, a run projection, and the supplier
that served a mediated search result or a run's sources in its namespaced
`data` field). Behavioural
edge cases — batch splitting at the 500-event cap, refusals, the privacy
floor — stay in each side's own test suite; the corpus defines the shape of
what crosses, not everything about how each side behaves.
