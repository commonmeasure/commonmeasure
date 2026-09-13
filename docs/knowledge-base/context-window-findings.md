---
title: Context-window findings and the product's status against them
draft: true
---

# Context-window findings and the product's status against them

Constraints from [`docs/guide/context-window-optimisation.md`](../guide/context-window-optimisation.md), each with the
product's status (honoured, partly, not yet) and the code or package that owns it.

## Admission and routing

- Admission is a quality decision: the accuracy-optimal amount of context is
  often below the budget-optimal amount, so a refusal is evaluated as a
  quality mechanism, not only a cost control. Partly: coverage and freshness
  are measured per plan (`crates/commonmeasure-runtime/src/evaluate.rs`); answer
  quality needs the judge in `ROADMAP.md` §Fidelity verification.
- Routing has a ceiling where supply descriptions overlap. Honoured in the
  routing experiment's reading ([`docs/READ-A-HOLDOUT.md`](../READ-A-HOLDOUT.md) §5); no code enforces it.
- Compression sits below deduplication, ranking and budget-aware selection.
  Honoured: the context optimiser runs at the transform stage, after
  admission and selection (`crates/commonmeasure-runtime/src/processor.rs`).
- A saved token is worth the price of the model it was headed for: rerank or
  compress before an expensive uncached model, not before a cheap model on a
  warm prefix. Partly: the model route is recorded per run
  ([`docs/contracts/run-output.md`](../contracts/run-output.md)); cache state is not recorded.
- Not yet, and no package owns them: degradation curves are model- and
  task-specific, so a router fitted on one model is not evidence about
  another (no second suite with a different model exists); content injected
  before a host's cache breakpoint invalidates the host's cache (host-owned).

## What the record carries

- Inventory figures are reconstructions and are labelled as such, because no
  API reports a per-component token breakdown. Honoured: the per-category
  inventory is estimated from the host's own transcript records, carries its
  basis on the record, and names the categories no basis can see
  (`crates/commonmeasure-harness/src/snapshot.rs`,
  [`docs/contracts/session-evidence.md`](../contracts/session-evidence.md) §Context snapshots).
- Not yet, and no package owns them: cache read and write tokens per plan
  (so a plan cheaper because its prefix cached is not mistaken for one that
  admitted better sources); ordering recorded as a decision; source overlap
  per run.
- Extraction quality is a named difference between providers. Not yet: the
  envelope records text and its basis ([`docs/contracts/provider.md`](../contracts/provider.md)), no more.
- Citation support has three states: the link resolves, the page is
  relevant, the page supports the claim. Partly: the grounding evaluator
  records supported, contradicted, uncovered and unparseable
  (`crates/commonmeasure-runtime/src/evaluate.rs`); relevance is not a separate state.
- A gap names which failure mode it is. Partly: gaps carry a reason
  (`crates/commonmeasure-runtime/src/evidence.rs`); retrieval failure modes are not a taxonomy.
- Every optimiser emits input and output hashes, token counts, removed spans
  and configuration identity. Partly: the processor contract requires a
  configuration digest and per-invocation evidence
  ([`docs/contracts/processor.md`](../contracts/processor.md)); removed spans are not recorded.

## Evaluation

- Every comparison includes a no-acquisition plan and a single-fixed-provider
  plan. Partly: the committed comparisons run each source class alone and
  mixed (`demo/commerce/README.md`); the job contract does not require the
  two baselines.
- Not yet, and no package owns them: variance and the minimum detectable
  effect in the run record (one run per plan today); the correlation between
  the evaluators and the operator's real objective; a one-line self-check
  (does the retrieved evidence suffice) in the baseline set; "succeeded on
  all k runs" reported beside the mean.
