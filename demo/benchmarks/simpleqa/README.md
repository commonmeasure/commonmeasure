---
domain: edge
audience: contributor
---

# SimpleQA starter

Five cases from the pinned
[Tavily search evaluation dataset](https://github.com/tavily-ai/tavily-search-evals/tree/99feb7decc9be67edb49a63d3985cf6094c873f2),
with the same upstream correctness rubric. The source code and redistributed
material retain the [upstream MIT notice](UPSTREAM-LICENSE.txt).
The rubric is `crates/commonmeasure-runtime/src/processor/simpleqa-grader.txt`.
The answer-generation path and plain-text grading protocol differ from
Tavily's runner; scores must be labelled Common Measure evaluations on SimpleQA.

From the repository root, exercise the workflow without external calls:

```sh
cargo run -p commonmeasure-cli -- benchmark \
  --dataset demo/benchmarks/simpleqa/smoke.json \
  --output /tmp/commonmeasure-simpleqa-smoke --limit 5 --seed 0
```

The output directory must be new. This produces unavailable/unmeasured
outcomes, not benchmark scores. `summary.json` and `summary.csv` can be shared;
the remaining files are private source records. See the
[benchmark contract](../../../docs/contracts/simpleqa-benchmark.md).

For a scored run, set supplier credentials and
`COMMONMEASURE_INFERENCE_ENDPOINT` as for `commonmeasure run`, choose model
names the gateway supports, and add `--live`. `--model` and `--judge-model`
select the answer and grader models independently. `--providers` selects the
search suppliers (default `exa,tavily`); the no-context baseline is always
recorded. The fixture tests and the offline command above do not establish
live execution.

## OpenRouter

Set `COMMONMEASURE_INFERENCE_ENDPOINT` to OpenRouter's fixed endpoint and
exactly one of `OPENROUTER_API_KEY` (in the process environment) or
`OPENROUTER_API_KEY_FILE` (a path to an existing local key file):

```sh
export COMMONMEASURE_INFERENCE_ENDPOINT=https://openrouter.ai/api/v1/chat/completions
export OPENROUTER_API_KEY_FILE=/path/to/existing/openrouter-key
cargo run -p commonmeasure-cli -- benchmark \
  --dataset demo/benchmarks/simpleqa/smoke.json \
  --output /tmp/commonmeasure-simpleqa-openrouter \
  --model openai/gpt-4.1 --judge-model openai/gpt-4.1 --limit 5 --live
```

OpenRouter model IDs are namespaced (`openai/gpt-4.1`); the CLI does not
rewrite them. Answer generation and grading both go to OpenRouter; search
still goes to the selected suppliers. Key handling, routing and what stays
unknown (monetary inference charges among it) are in the
[benchmark contract](../../../docs/contracts/simpleqa-benchmark.md).
OpenRouter's own references: [authentication](https://openrouter.ai/docs/quickstart)
and [provider routing](https://openrouter.ai/docs/guides/routing/provider-selection).

Verification: environment-key authentication is **live-verified** by a
five-case smoke run on 22 September 2026 (sampled from the full dataset, seed
zero, Exa and Tavily, `openai/gpt-4.1` for answer and judge); key-file loading
is **fixture-tested**. Three HTTP 429 responses, not retried, left one case
per supplier and the baseline unmeasured, so `accuracy_all_selected` stayed
null. The run establishes the live path, not a supplier ranking or a
full-benchmark result. Larger runs need pacing, and there is no recovery for
ungraded answers that avoids repeating acquisition.

## Full dataset

To prepare all 4,326 cases, obtain
`datasets/simple_qa_test_set.csv` from the pinned upstream revision and run:

```sh
python3 demo/benchmarks/simpleqa/import_dataset.py \
  /path/to/simple_qa_test_set.csv /tmp/commonmeasure-simpleqa-full.json
```

The importer checks the exact upstream byte digest and row count before
writing. Use that JSON with the same command; `--limit 5 --seed 0` samples
five cases from the full population, while `--limit 4326` selects all cases.
Start with a small sample to verify gateway and supplier configuration.
