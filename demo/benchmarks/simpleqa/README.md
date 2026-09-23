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

For a scored run, configure existing supplier credentials and
`COMMONMEASURE_INFERENCE_ENDPOINT` as described in the ordinary runtime
configuration, select model names supported by that gateway, and add `--live`.
`--model` and `--judge-model` select the answer and grader models independently.
No new gateway or supplier credential store is introduced. Live execution
is not established by the fixture tests or the offline walkthrough.

## OpenRouter

Direct OpenRouter access is **live-verified** for the five-case smoke run below.
Configure its fixed endpoint and
either `OPENROUTER_API_KEY` in the process environment or
`OPENROUTER_API_KEY_FILE` pointing to an existing local key file. Setting both,
an empty key or an unreadable file refuses configuration. The key is only sent
in the Authorization header to that endpoint; redirects are not followed.

Example with an existing key file and supplier credentials. The live smoke used
the environment-key setting; key-file loading remains fixture-tested:

```sh
export COMMONMEASURE_INFERENCE_ENDPOINT=https://openrouter.ai/api/v1/chat/completions
export OPENROUTER_API_KEY_FILE=/path/to/existing/openrouter-key
cargo run -p commonmeasure-cli -- benchmark \
  --dataset demo/benchmarks/simpleqa/smoke.json \
  --output /tmp/commonmeasure-simpleqa-openrouter \
  --model openai/gpt-4.1 --judge-model openai/gpt-4.1 --limit 5 --live
```

OpenRouter uses namespaced model IDs; its
[GPT-4.1 entry](https://openrouter.ai/openai/gpt-4.1) is `openai/gpt-4.1`.
The CLI does not rewrite model names. Both answer generation and grading use
OpenRouter; search still uses the selected suppliers. Temperature remains zero.
Provider routing and fallbacks follow OpenRouter account defaults. Requested
and reported execution fields are retained separately; this does not claim
that every call used the same upstream provider. Monetary inference charges
remain unknown in this first integration even when OpenRouter reports pricing
fields that the existing parser does not read. See the
[API authentication example](https://openrouter.ai/docs/quickstart) and
[routing documentation](https://openrouter.ai/docs/guides/routing/provider-selection).

### Live smoke, 22 September 2026

Five cases sampled from the full 4,326-case dataset, seed zero, ran through Exa
and Tavily with five results requested and `openai/gpt-4.1` for answer and judge.
All 14 successful answer calls and 12 successful grader calls reported
`openai/gpt-4.1` and provider `OpenAI`. Two grader calls and one no-context
answer returned HTTP 429; no request was retried. Exa had three correct answers
and one abstention among four graded cases; Tavily had four correct answers
among four graded cases. Each provider and the baseline retained one unmeasured
case, so full-selected accuracy remained null. This is a smoke sample with
incomplete grading, not a supplier ranking or full-benchmark result.

The run used source commit `3dbbe01`, the resolved operator policy, and binary
SHA-256 `05d399ee3650eca14bebf7fbca8ffdbdb31086bff4dea1abc8aee5d08c21ef30`.
Private evidence and the aggregate sharing files are retained outside git;
the owning live brief records their local location. Credential scans of the
retained output found no key values. Larger runs still need pacing and a
separate recovery design for ungraded answers that avoids repeating acquisition.

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
