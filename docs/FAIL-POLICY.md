---
title: Evidence-integrity contract
---

# Evidence-integrity contract

When the runtime could not observe, record or enforce something, it writes a
gap record saying so, in the same log as everything else. A reader can always
tell "nothing happened" from "this could not say what happened".

Binding on the implementation, for both batch runs and harness sessions. Every
clause names what enforces it — the test, where the behaviour can be witnessed
from outside; the implementation, where it cannot, and the clause says which. A
behaviour change and a contract change are made in the same commit. Terms like
crossing, run, evidence log and gap are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

## 1. Durable before acknowledged

Every evidence record is fsynced before the append returns, and the directory
entry is synced too on the append that creates the log. "Recorded" means on
disk, not in a buffer.

The two halves are witnessed differently. Records are appended in order and read
back in order under test; durability itself is not — a test in one process
cannot tell a synced write from a buffered one, and this repository has no way
to cut power to a disk. The fsync is held by the implementation and by review of
it, not by a test.

- `records_are_durable_and_readable_in_order`
  (`crates/commonmeasure-runtime/tests/evidence_integrity.rs`) — order and readability
- `append_line` (`crates/commonmeasure-runtime/src/evidence.rs`) — the `sync_all` on the
  file and, for a newly created log, on its parent directory

## 2. Write-failure windows are materialised

When an append fails, the window is held and written as an `evidence_gap`
record by the next successful append. If the gap record itself cannot be
written, the gap stays pending — nothing is lost and nothing is silently
dropped. `run.evidence_complete` is false while a gap is owed.

A pending gap is held by the log instance that owes it, so it reaches the file
only through a later append on that same instance. A run is one process and one
log, so the limit costs nothing there; a session is not one process, and
[`docs/contracts/session-evidence.md`](contracts/session-evidence.md) §Where states what the limit costs there.

A run does not abort because its log could not take a line: the work still
happened. It must not finish claiming a complete record.

That obligation fixes an ordering. The log is closed — terminal record written,
any owed gap given a last chance to be written — before completeness is read
and the summary is built. The other order publishes a claim that the next write
can falsify, and since a failed append is deliberately not fatal, nothing
downstream would correct it. The summary therefore attests to the log
(record count and digest) rather than the log attesting to the summary: a
finished log cannot carry the hash of a document that does not exist yet.

- `a_write_failure_window_is_materialised_as_a_gap`,
  `a_failed_terminal_append_is_reported_as_an_incomplete_record`,
  `a_closed_log_reports_its_own_size_and_digest`
  (`crates/commonmeasure-runtime/tests/evidence_integrity.rs`)
- `the_summary_attests_to_a_finished_evidence_log`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)

## 3. One log, one run — and one log, one session

A run's log is created exclusively: a second run cannot append to a first run's
file, because a log holding two runs describes neither.

A session's log is the opposite and for the same reason. A session spans many
short-lived hook processes, so its log is opened for append and `seq` resumes
from what is there. Concurrent hooks can share a `seq`; a reader orders by
position.

- `a_second_run_cannot_append_to_a_first_runs_log`
  (`crates/commonmeasure-runtime/tests/evidence_integrity.rs`)
- `separate_hook_processes_append_to_one_session`
  (`crates/commonmeasure-cli/tests/hook_e2e.rs`)

## 4. A run is published only when it is complete

Artefacts are written into a staging directory beside the published one,
named for the run that is writing it, and
moved into place atomically. An interruption leaves the previously published run
exactly as it was; a partial run never appears under the published name.

- `a_completed_run_replaces_the_previous_one`,
  `an_abandoned_run_leaves_the_published_one_intact`
  (`crates/commonmeasure-runtime/tests/evidence_integrity.rs`)

## 5. A missing dependency produces an explicit unavailable result

No credential, no gateway, or an unreachable gateway each produce a plan with
status `unavailable`, no answer, and a gap naming what is missing. There is no
weaker successful mode and nothing to fall back to.

- `without_a_gateway_every_plan_is_explicitly_unavailable`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)
- `a_run_with_no_dependencies_claims_nothing_and_explains_every_absence`
  (`crates/commonmeasure-cli/tests/run_contract.rs`)
- `an_unreachable_gateway_is_unavailable` (`crates/commonmeasure-inference/tests/gateway.rs`)

## 6. Per-mode enforcement, identical recording

`PolicyMode` decides what happens to a breach, never whether it is recorded.
`Strict` refuses; `Prefer` and `Observe` carry the breach and record it
identically.

Two refusals are unconditional in every mode, because neither is a matter of
operator preference: a source that carries no text cannot ground an answer,
and a required licence is not satisfied by an unknown one.

A processor's verdict is under the same discipline, with one stated
exception. The injection screen's match refuses the crossing in `strict` and
is carried as a recorded breach in `observe` and `prefer`, recorded
identically, because it reaches the decision through the same `Ruling` every
other constraint uses. The PII detector's finding does the same on an
internal or private source; on a public source `strict` records the finding
and admits the crossing, because a public page's published contact details
are not the personal data the detector exists to keep out of a model, unless
the policy sets `refuse_on_pii`. The finding is recorded the same way
whichever way it is ruled.

- `strict_source_policy_refuses_a_disallowed_host_before_inference`,
  `observe_mode_carries_the_same_breach_it_does_not_lose_it`,
  `a_strict_pii_finding_on_a_public_source_is_recorded_and_the_source_carried`,
  `a_strict_pii_finding_on_the_internal_corpus_refuses_the_source_before_inference`,
  `a_strict_pii_finding_on_a_private_address_from_a_supplier_refuses_the_source`,
  `observe_mode_records_the_pii_finding_and_carries_the_source`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)
- `a_public_source_carries_a_pii_finding_in_strict_and_an_internal_one_is_refused`,
  `refuse_on_pii_refuses_a_public_source_in_strict`
  (`crates/commonmeasure-runtime/src/processor/pii.rs`)
- `observe_mode_records_the_breach_it_does_not_enforce`,
  `a_source_without_text_is_refused_in_every_mode`,
  `a_required_licence_is_not_satisfied_by_an_unknown_one`
  (`crates/commonmeasure-runtime/tests/policy_and_selection.rs`)
- `observe_mode_carries_the_crossing_and_still_records_it`,
  `a_compliant_crossing_records_no_breach`,
  `a_fetch_carrying_pii_from_a_private_address_is_refused_in_strict_mode_and_recorded`,
  `a_fetch_carrying_pii_from_a_named_internal_prefix_is_refused_in_strict_mode`,
  `refuse_on_pii_is_loaded_reported_and_refuses_with_the_processor_wording`,
  `observe_mode_carries_a_pii_finding_and_records_it`
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`);
  `a_public_source_is_admitted_with_the_finding_recorded_and_the_other_classes_are_refused`
  (`crates/commonmeasure-harness/src/mcp.rs`) for the public case, which no
  loopback origin can stand in for

## 7. Unknown is never zero

An unreported charge makes a cap unenforceable and leaves a gap saying so; it
never satisfies the cap. A gateway that reports no usage yields `null` tokens,
not zero. A gateway that does not name the model it ran does not thereby confirm
the requested one.

A provider that declares no price is bounded by the principal's allowance only
at the dispatch after the one that exhausts it: the charge counts from the
receipt, so one unpriced dispatch can exceed the allowance by any amount
([`docs/contracts/run-output.md`](contracts/run-output.md), `acquisition.allowance`). A settlement the
allowance ledger does not record after one retry is a gap naming the
reservation, the observed charge and the reason; the ledger's total then omits
money that moved, and the record says so rather than the ledger counting zero.

A latency total is the sum of the legs that ran. A plan whose inference never
happened has no inference leg to add, which is not the same as adding a
zero-length one — and written as a defaulted zero, an absent measurement becomes
arithmetically indistinguishable from an instantaneous one.

An evaluation that measured nothing is `unevaluated` or `unmeasured` with
the reason stated, never a zero: a suite with no coverage rubric or as-of
date, a plan with no assembled window, and a window part with no declared
date each leave the fraction absent rather than fabricated — and one
unmeasured candidate blocks a fraction-weighted selection rather than
silently losing it.

- `an_unreported_charge_makes_the_cap_unenforceable_not_satisfied`,
  `cost_selection_abstains_when_a_candidate_disclosed_no_price`,
  `one_unmeasured_candidate_blocks_a_fraction_ranking_and_is_named`
  (`crates/commonmeasure-runtime/tests/policy_and_selection.rs`)
- `an_unpriced_provider_is_refused_only_once_the_allowance_is_exhausted`,
  `a_settlement_the_ledger_cannot_write_is_a_gap_naming_the_money_that_moved`
  (`crates/commonmeasure-runtime/tests/allowance_dispatch.rs`),
  `a_settlement_the_ledger_cannot_write_leaves_a_gap_naming_the_money_that_moved`
  (`crates/commonmeasure-harness/src/mcp.rs`)
- `no_rubric_is_unmeasured_never_zero` (`crates/commonmeasure-runtime/src/coverage.rs`),
  `an_undated_part_makes_the_plan_unmeasured_naming_the_part`
  (`crates/commonmeasure-runtime/src/freshness.rs`)
- `a_silent_gateway_yields_unknowns_never_zeroes_or_the_requested_route`
  (`crates/commonmeasure-inference/tests/gateway.rs`)
- `a_plan_without_inference_reports_the_acquisition_leg_alone`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)

## 8. Capture never breaks the agent

A hook exits zero whatever happens, and a payload it cannot read produces no
records rather than an error. Interrupting the operator's work over a
recording problem is worse than missing a row, and a row the hook cannot
understand must not be invented.

- `no_payload_makes_the_hook_fail_the_tool_call`
  (`crates/commonmeasure-cli/tests/hook_e2e.rs`)
- `malformed_input_gets_an_error_and_the_session_continues`
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`)

## 9. Observed and mediated are never merged

Which path recorded a crossing is a recorded field. A mediated crossing was
governed before it happened and could have been refused; an observed one was
seen after it had happened. Recording them alike would claim enforcement that
did not occur.

This plugin's own MCP tools are therefore excluded from observed capture: they
are already recorded by the server that carried them.

- `a_mediated_fetch_returns_the_bytes_and_records_the_crossing`,
  `strict_policy_refuses_the_crossing_and_records_the_refusal`
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`)
- `our_own_mediated_tools_are_not_observed_a_second_time`
  (`crates/commonmeasure-harness/src/hook.rs`)

## 10. Grounding is claimed only where bytes entered context

`WebFetch` grounds. Search and third-party MCP results do not: nothing ties a
returned URL to the bytes the model saw. The hash covers the ingested text, not
the transport wrapper around it, or it can never be matched against a
transcript.

- `a_websearch_records_each_result_and_grounds_none`,
  `a_webfetch_is_recorded_with_the_hash_of_what_entered_context`
  (`crates/commonmeasure-cli/tests/hook_e2e.rs`)
- `a_citation_outside_the_window_is_uncovered`
  (`crates/commonmeasure-runtime/tests/evaluation.rs`): the grounding
  evaluator gives a cited URL that never entered the window the `uncovered`
  verdict, not a supported one

## 11. Context is admitted whole or not at all

A source that will not fit the job's budget is left out whole, with a gap. A
truncated source has a content hash that matches nothing, and a citation into
text the model never saw is worse than a missing source.

- `the_context_budget_drops_whole_sources_rather_than_truncating_them`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)
- The grounding evaluator is what checks a citation against the admitted
  text: `a_faithful_citation_is_supported_and_its_span_excises_to_the_quote`,
  `a_quote_no_window_source_contains_is_contradicted`,
  `a_citation_outside_the_window_is_uncovered`
  (`crates/commonmeasure-runtime/tests/evaluation.rs`)

## 12. A transformation is evidence, not a rewrite

A transform-stage processor that changes context leaves a record a reviewer
can re-derive without trusting the runtime: excising the recorded spans from
the sealed source text must reproduce the recorded output hash, and the
original envelope — its own content hash, its provider metadata — stays
addressable beside the transformed text that entered the window. An
extraction record is re-derived by applying the rules the configuration
digest pins to the bytes `retrieved_hash` names, which reproduces
`content_hash`; the crossing carries both hashes, and the record that ties
them is written before it. A processor
finding never copies the content it found: the PII detector records categories
and byte offsets, checkable against the sealed bytes, because an evidence
trail that repeats an identifier has disclosed the data it exists to detect.

- `the_optimiser_reduces_admitted_tokens_without_dropping_a_source`
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`)
- `the_output_hash_is_rederivable_by_excising_the_recorded_spans`
  (`crates/commonmeasure-runtime/src/processor/optimise.rs`)
- `the_output_is_a_function_of_the_input_under_the_pinned_rules`
  (`crates/commonmeasure-runtime/src/processor/extract.rs`),
  `an_html_page_is_delivered_as_extracted_text_with_both_hashes_recorded`,
  `a_gzip_page_is_carried_with_the_retrieved_hash_over_the_coded_bytes`
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`)
- `strict_refuses_and_observe_carries_the_same_finding`
  (`crates/commonmeasure-runtime/src/processor/pii.rs`),
  `a_fetch_carrying_pii_from_a_private_address_is_refused_in_strict_mode_and_recorded`
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`) — the identifier itself appears in
  no record.

## 13. A replay run refuses to start rather than degrade

A missing or altered recording aborts a replay run before any run directory
exists. This is deliberately not clause 5: a plan marked `unavailable` would
make a weaker run that still calls itself a replay, and a reader diffing two
"replay" runs must never discover afterwards that one replayed less than the
other without saying so. The refusal names the provider or the hash
disagreement; nothing is published.

- `a_provider_without_a_recording_refuses_to_start`,
  `an_altered_capture_refuses_to_serve`
  (`crates/commonmeasure-runtime/tests/replay_mode.rs`)

## Execution modes

Execution modes must stay visibly distinct in code, output and UI:

| Mode | Context supply | Inference | Evidence claim |
|---|---|---|---|
| no external acquisition | internal corpus query, if configured; no external call | configured gateway, if any | internal and baseline plans may complete; external plans are `unavailable` |
| recorded replay | committed redacted response data through the real adapters | real local or configured model route | replay-tested |
| live experiment | authenticated provider calls, response bytes sealed | real pinned model route | `live-verified` per plan |
| harness session | observed or mediated harness activity | model used by the host | real local evidence |

No mode may silently fall back to a weaker one: a missing dependency produces an
explicit unavailable result or gap (§5).
