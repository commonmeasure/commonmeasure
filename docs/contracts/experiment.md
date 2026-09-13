---
title: Experiment contract
---

# Experiment contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

A provider comparison is valid only when its varying and fixed inputs are
explicit. Terms like suite, run, plan and manifest are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

## Fixed per experiment

- context-job suite version;
- answer model and inference parameters;
- system and task prompts;
- provider credential class and account tier;
- geography and execution window;
- maximum result count and admitted context budget;
- evaluator set, versions and rubrics — sealed in the run manifest as
  `evaluators` (today: grounding, coverage and freshness,
  [`docs/contracts/run-output.md`](run-output.md) §Plans), with the suite's declared
  coverage rubric, as-of reference and governance block sealed beside them
  as `coverage_rubric`, `as_of` and `governance`;
- retry, timeout and cache policy;
- aggregation and ranking method.

When the experiment varies anything downstream of acquisition — context
policy, optimisation or model configuration — the supply itself is held
fixed by running against recorded responses (`--replay`,
[`docs/contracts/run-output.md`](run-output.md)), with every served byte pinned to a manifest
hash.

## Varied deliberately

- supply plan and provider;
- provider-native retrieval configuration where the plan declares it;
- fallback or ensemble policy in experiments designed to test it.

## Required records

- immutable run identifier and manifest hash;
- start/end timestamps and component latencies;
- raw response reference and normalised envelope hash;
- canonical sources and transformation history;
- quoted and observed charges;
- context tokens admitted to inference;
- answer and citation artefact hashes;
- evaluator results with evaluator identity;
- every failure, retry, refusal and missing field.

## Fairness rules

- Do not call a snippet-only response equivalent to full-page extraction.
- Do not hide provider retries inside aggregate latency.
- Do not score absent licence data as an unlicensed claim; score it unknown.
- Do not reuse one provider's discovered URLs in another provider's independent
  search trial unless the experiment is explicitly a fetch comparison.
- Randomise execution order where time and freshness could bias a provider.
- Separate provider acquisition cost from downstream model-token cost.
- Preserve native provider strengths; compare plans for jobs, not vendors in the
  abstract.

## Output

The console may show Pareto frontiers and job-specific winners. It must not
publish a universal provider ranking from heterogeneous tests.
