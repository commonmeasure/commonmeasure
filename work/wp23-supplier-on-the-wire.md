# The supplier that served an acquisition, on the wire

## Package

WP-23, the box "The supplier that served an acquisition on the wire".

## Goal

A content owner, or the network reporting for it, can tell from the Content
Telemetry a Common Measure edge delivers whether a retrieval was served
through a licensed supplier or fetched from the open web. Each mediated
search result records the provider that served it; the relay projects that
name as the namespaced custom field `commonmeasure-supplier` on the retrieval
and grounding events of supplied sources, for sessions and for runs, and
withholds it for the operator's own corpus and for skills. The corpus
carries a supplied session and a supplied run, and the hub's pinned copy is
re-pinned in the same change set.

## Files

- `crates/commonmeasure-harness/src/session.rs`
- `crates/commonmeasure-harness/src/mcp.rs`
- `crates/commonmeasure-harness/src/hook.rs`
- `crates/commonmeasure-harness/src/import.rs`
- `crates/commonmeasure-relay/src/project.rs`
- `crates/commonmeasure-relay/Cargo.toml`
- `crates/commonmeasure-relay/tests/conformance.rs`
- `crates/commonmeasure-relay/tests/relay.rs`
- `crates/commonmeasure-cli/tests/mediated_e2e.rs`
- `conformance/session-supplied.json`
- `conformance/run-supplied.json`
- `conformance/README.md`
- `docs/contracts/session-evidence.md`
- `DECISIONS.md`
- `ARCHITECTURE.md`
- `ROADMAP.md`

## Done when

- `cargo test -p commonmeasure-relay` passes, including
  `a_supplied_crossing_names_its_supplier_and_a_fetch_names_none`,
  `the_internal_corpus_and_a_skill_are_never_named_as_suppliers` and
  `a_supplied_result_names_its_supplier_at_the_receiver_and_a_fetch_does_not`.
- `cargo test -p commonmeasure-relay --test conformance` passes without
  `UPDATE_GOLDENS`, with `conformance/session-supplied.json` and
  `conformance/run-supplied.json` present and every document valid against
  `schema/telemetry-event-batch.v1.json`.
- `cargo test -p commonmeasure-cli --test mediated_e2e` shows the internal
  corpus result crossing recording `supplier`.
- The hub repository's pinned corpus is re-pinned to this change and its
  replay test passes there.

## Out of scope

- Sending a `Content-Telemetry-ID` on supply requests: waits on a supplier
  documenting that it accepts the header, at which point its adapter declares
  `corroborate`.
- Anything about what an acquisition cost on the wire.
- The hub's network registration, parent-host resolution and network report.
