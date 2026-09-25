---
title: SimpleQA benchmark
domain: edge
audience: integrator
section: reference
---

# SimpleQA benchmark

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

The local `commonmeasure benchmark` command runs a deterministic subset of
SimpleQA through the existing batch runtime: governed search, context
admission, answer generation and a reference-answer correctness processor.
It is available to the local operator. There is no hosted execution or new
Hub role; organisation access to benchmarks is proposed and not built.

## Inputs and controls

The dataset importer pins Tavily's `tavily-search-evals` revision
`99feb7decc9be67edb49a63d3985cf6094c873f2` and checks the SHA-256 of the exact
4,326-row CSV before normalising it. Case IDs are one-based upstream row
numbers. The bundled smoke set contains the first five rows; it is for
workflow verification and does not represent the full benchmark. The runner
seals the exact normalised dataset bytes, including locally supplied changes;
the source metadata alone is not an independent authenticity check.

Selection sorts cases by SHA-256 of
`simpleqa-sample/v1\n<decimal seed>\n<case id>`, then takes the requested
count. Manifest fields retain the selected IDs, available population, seed,
dataset digest, upstream revision, suite, resolved principal policy, answer
and judge model plans, grader configuration digest and inference request shape.
The default is five cases, seed zero, five results, Exa and Tavily, and
`gpt-4.1` for answer and judge. These are requested controls; per-call evidence
records what the adapter and gateway actually report.

The CLI resolves the process principal and working-directory source policy
before constructing each job. Mode and constraints are copied without
reordering access rules, and the existing principal allowance is consulted by
the runtime. Policies with terms overlays, unresolved principal policy,
`refuse_on_pii` or named internal-source prefixes are refused before dispatch
because the batch path cannot represent those settings. This preserves the
session policy's stronger PII and internal-source screening requirements.
Internal providers and skill invocation are excluded. The runtime's existing
batch admission, privacy and PII behaviour applies; this command does not
claim parity with every mediated-session feature.

Without `--live`, neither supplier nor model calls occur. `--live` enables
acquisition, answer generation and grading; it requires a configured inference
gateway. The question goes to selected search suppliers; the question and
admitted context go to the answer gateway; the question, reference answer and
prediction go to the same gateway for grading. The gold answer never enters
search or answer generation. Unknown supplier charges need no additional
approval; existing source policy and allowance rulings still apply.

Direct OpenRouter is supported at the exact endpoint
`https://openrouter.ai/api/v1/chat/completions`, using exactly one of
`OPENROUTER_API_KEY` and `OPENROUTER_API_KEY_FILE`. Other endpoints never load
or receive that credential. Missing, ambiguous, malformed or unreadable keys
leave inference unavailable; the live benchmark preflight stops before any
calls. The fixed HTTPS destination, no-redirect transport and bounded key-file
read prevent forwarding the key to a caller-selected origin. Authentication is
excluded from request-body digests and evidence. Authenticated HTTP error
bodies are omitted; a response echoing the exact key is rejected. Local gateway
configuration and request-body construction are unchanged.

Use OpenRouter model IDs such as `openai/gpt-4.1` for both `--model` and
`--judge-model`; names are not translated. The recorded gateway is `openrouter`,
and executed model/provider fields remain whatever the response reports.
OpenRouter account routing and fallback defaults apply; fixed upstream-provider
routing and monetary inference-charge parsing are not implemented. Setup is in
`demo/benchmarks/simpleqa/README.md`. Direct environment-key authentication is
**live-verified** by a five-case smoke run; key-file
loading remains fixture-tested. All successful answer and grader calls reported
the requested `openai/gpt-4.1` model and provider `OpenAI`. Three HTTP 429
responses left two supplier grades and one baseline answer unmeasured; no
request was retried. This establishes the live execution path, not complete
grading, a fixed upstream route or full-dataset acceptance.

## Correctness and accounting

`simpleqa-correctness/1` is a verify-stage processor using the pinned Tavily
`OPENAI_GRADER_TEMPLATE`, with trailing whitespace removed. Its plain-text
response protocol accepts exactly
`A`, `B` or `C` after trimming whitespace. Other responses and missing/failed
inference are unmeasured. A valid `C` is the model's judgement that an answer
did not attempt the question; a transport failure is not an abstention.
The invocation preserves the exact grader request and reply, requested and
reported executed models, token counts, latency, and unknown monetary charge.
The rubric and its licence are included in the repository; attribution and
walkthrough are in `demo/benchmarks/simpleqa/README.md`.

Each provider, including the separately named `none` no-context baseline,
retains the full selected-case denominator. Summary counts distinguish
completed, refused, unavailable, errored, correct, incorrect, not attempted,
measured and unmeasured outcomes. `accuracy_measured` is correct divided by
graded cases, null when none were graded. `accuracy_all_selected` is only
available when every selected case has a grade. Missing measurement never
becomes zero. Acquisition charges and inference usage remain in individual
source records; aggregate acquisition, answer and judge costs remain null
until a unit-aware, completeness-aware cost aggregation is implemented.

The existing runtime also generates a no-context answer per case, which can
incur a model charge. Each completed answer has one grader call. No automatic
retry or resume is implemented. The output directory must not already exist.
An interrupted attempt retains its manifest, started events and any published
case runs; a started request may have incurred a charge. Benchmark-log write
failures and run I/O errors stop subsequent dispatch. A case whose runtime
reports `evidence_complete=false` stops later cases and is not graded.
Within a case, ordinary run-log failures retain the runtime's existing gap
semantics: dispatch may continue, and recording a recovered gap can restore
`evidence_complete=true`. That flag does not assert that no evidence was lost;
the private run record retains the gap. A recovered gap does not itself stop
grading or subsequent cases.

## Artefacts and sharing

`contextops-simpleqa-benchmark/v1` writes `manifest.json`, `dataset.json`,
`evidence.ndjson`, one ordinary run directory per case, and
`results.private.json` with grader evidence. These are private source records,
outside Content Telemetry and Hub relay input.

`summary.json` and `summary.csv` are the first benchmark sharing files. They
contain aggregate provider results; the JSON also carries dataset and
experiment hashes, requested model names, seed and result limit. They exclude
questions, gold answers, predictions, source content, principal identity,
gateway endpoints and raw diagnostics. An organisation may still consider its
model names and aggregate performance confidential. They are unsigned local
files, with no hosted sharing link. Compare ZIP export remains a separate
[single-comparison format](comparison-export.md).

This is a **Common Measure evaluation on SimpleQA**, not a reproduction of
Tavily's published scores. Answer instructions, policy and context processing,
provider configuration and judge transport differ. A small sample is not a
leaderboard. Document relevance (including QuotientAI), full-dataset live
validation, cost aggregation, batch UI and batch ZIP export remain open.
