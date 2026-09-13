---
title: The QA gate
draft: true
---

# The QA gate

The gate runs after each work package and before a release checkpoint
(`ROADMAP.md` §Definition of done). Open findings live in `OPEN.md`.

## Mission

Independently decide whether the repository is a small, truthful and
technically sound base for the product `PRODUCT.md` describes.

Reject mocks, generated ceremony, duplicated systems, unsupported claims and
abstractions without current value.

## Independence

Your first pass is read-only. Do not edit, stage or commit. Do not begin with the
landing's handoff; inspect the repository and evidence
first, then use the handoff only to check omissions.

Every finding must identify exact evidence, observable consequence, the simpler
correction and the test or run that would close it. Remediation happens only
after findings are reviewed and accepted.

## Read and inspect

Read completely:

1. `AGENTS.md`
2. `ROADMAP.md`
3. `PRODUCT.md`
4. `ARCHITECTURE.md`
5. `README.md`
6. `DECISIONS.md`
7. [`docs/FAIL-POLICY.md`](../FAIL-POLICY.md)
8. provider, experiment, processor, run-output and session-evidence contracts
10. provider verification and recon notes

Inspect all Rust, manifests, tests, runnable commands, retained artefacts,
console behaviour and the complete diff from the previous accepted baseline.

## Review lenses

### Execution truth

- Does any mock, fake backend, embedded response, constant score or test-only
  implementation stand in for a claimed integration?
- Can a “live”, “replay” or “run” command silently downgrade?
- Are requested routes presented as executed routes?
- Are unknown measurements represented as zero or fabricated values?
- Does every UI action do what its label says?

### Integration

- Is there one coherent dependency path rather than parallel `commonmeasure-*` and copied
  subsystems?
- Does every retained crate and abstraction have a real current consumer or an
  external protocol boundary?
- Do recorded bytes pass through the same real parsing and policy path intended
  for live data?
- Do tests drive real binaries/processes and actual storage/transport boundaries
  where integration is claimed?

### Simplification

- Find duplicate types, conversion layers, needless traits, speculative
  generality, dead dependencies, copied CLIs and unused provider implementations.
- Prefer deletion or direct code when an abstraction has one internal consumer.
- Distinguish genuine domain variation from abstraction added for appearance.

### Rust quality

- Check ownership and lifetimes, error types and context, panic/`unwrap` paths,
  resource cleanup, atomic/durable writes, concurrency assumptions and command
  execution.
- Check idiomatic naming, module boundaries and dependency choices.
- Public APIs and non-obvious invariants must be documented; comments explain
  why, not mechanics.

### Human inspectability

A sceptical human must be able to answer from local artefacts:

- What command ran and with which sealed inputs?
- Which exact bytes were retrieved and admitted?
- Which content or skill route was requested and actually executed?
- What entered inference, and which model/provider actually ran?
- What did it cost in money, latency and tokens?
- Why was a route selected, refused or unavailable?
- Which evidence supports each output claim?

### Documentation consistency

Check that product, architecture, roadmap, contracts, Cargo graph, CLI help,
console language and verification badges describe the same system.

## Severity and gate

- **P0:** false success/evidence claim, hidden mock or downgrade, lost evidence,
  secret exposure, or no real path where one is claimed.
- **P1:** disconnected/duplicated architecture, incorrect unknown handling,
  misleading UI/docs, unsafe durability or error behaviour.
- **P2:** maintainability, documentation, idiom or test-coverage weakness that
  does not falsify the current claim.

Verdict:

- **PASS:** no open P0/P1 findings and acceptance evidence is reproducible.
- **BLOCK:** any open P0/P1 finding, or the principal path cannot be inspected.

Absence of a feature is not a defect when it is labelled unavailable or future.

## Commands

Run at minimum:

```sh
git status --short
git diff --check
git diff --stat
cargo metadata --no-deps --format-version 1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
rg -n 'fixture|mock|fake|stub|dummy' crates demo console docs
rg -n 'unwrap\(|expect\(|panic!|todo!|unimplemented!' crates --glob '*.rs'
rg -n 'stale|legacy|deprecated' Cargo.toml crates README.md ARCHITECTURE.md docs
cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output /tmp/qa-run
cargo run -p commonmeasure-cli -- inspect /tmp/qa-run
```

Run the documented CLI and manual walkthrough. If the environment prevents a
real socket/process check, report that limitation separately; do not call it a
code failure and do not substitute a mock.

## Report

Return proposed Markdown. After the findings are reviewed and accepted, add
each open one as a self-contained row in [`docs/qa/OPEN.md`](OPEN.md); the report
itself is not kept in the repository.

For each finding record:

```text
ID and severity:
Claim:
Evidence (file:line or command output):
Consequence:
Simpler correction:
Evidence required to close:
Status: open | accepted | resolved | rejected with reason
```

End with:

- PASS or BLOCK;
- commands and results;
- principal path actually demonstrated;
- claims deliberately unavailable;
- ordered remediation list;
- areas not inspected.

## Stop conditions

Stop and ask if review would require a paid/live call, destructive remediation,
or a product decision not already in the canonical documents. Otherwise finish
the read-only gate even when the verdict is BLOCK.
