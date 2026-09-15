# Trust-boundary test quality

## Package

WP-18, WP-19, WP-27 and WP-29: protect policy identity, managed policy,
session evidence and fetcher identity through the existing real seams.

## Goal

Tests fail when a trust-boundary check is removed or made incorrect.
Assertions on explanatory prose are replaced with behaviour assertions;
operator-facing interfaces and contract values remain asserted.

## Files

- `crates/commonmeasure-harness/src/policy.rs`
- `crates/commonmeasure-harness/src/managed.rs`
- `crates/commonmeasure-harness/src/identity.rs`
- `crates/commonmeasure-harness/src/session.rs`
- `crates/commonmeasure-cli/tests/inspect_dossier.rs`
- `crates/commonmeasure-cli/tests/mediated_e2e.rs`
- `crates/commonmeasure-cli/tests/install_e2e.rs`
- `crates/commonmeasure-cli/tests/serve_e2e.rs`
- `crates/commonmeasure-cli/tests/hook_e2e.rs`
- `crates/commonmeasure-cli/tests/managed_policy.rs`
- `crates/commonmeasure-cli/tests/connect_e2e.rs`

## Done when

Run cargo-mutants on each of the four source files separately, with one
job, a per-mutant timeout, and tests that can reach the file. Use an
isolated worktree and target directory; check available disk before each
run. Record the tool version and counts of caught, missed, unviable and
timed-out mutants in the change's validation description.

Triage every surviving mutant: a real gap needs a test shown failing with
the mutant and passing without it; an equivalent mutant needs a reason;
unreachable code may be removed only with caller-search evidence. Never
bulk-triage a refusal, hash, signature, time bound, scope match, principal,
clearance or path. Re-run each file after corrections.

Review string assertions in the named integration files. Keep values
operators or programs act on, accessible names and contract refusal
identifiers. Replace prose assertions with observable behaviour; delete
redundant assertions already covered by documentation path checks.

Formatting, workspace Clippy, harness tests, touched integration files
and the contract/documentation tests pass. The integrated tree passes the
repository gate. Findings that remain go in `docs/qa/OPEN.md`; routine
mutation output stays in temporary storage. Delete this brief on landing.

## Out of scope

No runtime-crate test sweep, workspace-wide mutation run, new mock seam or
production behaviour change except removing proven unreachable code.
