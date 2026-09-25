---
title: Run output contract
domain: edge
audience: integrator
section: reference
---

# Run output contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

`contextops-run/v7` is the shape a published run directory takes. It is what
`commonmeasure inspect` reads and what a reviewer reads directly. Terms like
run, plan, manifest and evidence log are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).
[`docs/contracts/session-evidence.md`](session-evidence.md) is the matching
contract for what a live harness session records.

A committed run carries the contract version it was sealed under and is read
as it declares itself; the dossier states when a run predates the current
contract.

## Run

From the repository root, with nothing configured:

```sh
cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output /tmp/commonmeasure-empty
cargo run -p commonmeasure-cli -- inspect /tmp/commonmeasure-empty
```

Every plan of that run is `unavailable`, each with a gap naming what is
missing ([`docs/GETTING-STARTED.md`](../GETTING-STARTED.md)).

`inspect` prints the run dossier: one document computed from the artefacts on
every invocation, never stored, in which every claim ends with a citation
(`s:`/`e:`/`r:`/`m:` plus a JSON pointer or seq) resolving to the record it
was read from. It recomputes the evidence log's digest and compares the
manifest hash across artefacts. `crates/commonmeasure-cli/tests/inspect_dossier.rs` checks that every
printed citation resolves.

A replay run serves recorded provider responses instead of calling any
provider. `<recordings>` is a directory holding the captures and their
`replay-manifest.json`; the project's own recordings are third-party provider
responses and are not published, so this form needs recordings of your own:

```sh
cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay.json \
  --replay <recordings> --output /tmp/commonmeasure-replay
```

`--replay <dir>` reads `<dir>/replay-manifest.json`, verifies each named
capture against the hash the manifest declares — over the canonical JSON of
the recorded body ([`docs/contracts/canonical-json.md`](canonical-json.md)), which is also what
the loopback origin serves — and serves those exact bytes to the same
adapters, parsers, policy, inference path and evidence storage a live run
uses. A provider the manifest does not cover
fails the run before it starts; there is no fallback to live access. The
response is fixed regardless of the run's query (each capture records the
request it answered), so policy, optimisation and model configuration can
vary against fixed supply. Inference is not replayed; it uses the configured
gateway.

External dependencies are configured explicitly:

- **Provider acquisition** happens only with `--live`, and only for providers
  whose credential variable is set (`commonmeasure_supply::required_variable` is the
  one list: `EXA_API_KEY`, `FIRECRAWL_API_KEY`, `KEENABLE_API_KEY`,
  `LINKUP_API_KEY`, `NIMBLE_API_KEY`, `OZONE_LIVE_API_KEY`, `PARALLEL_API_KEY`,
  `REDPINE_API_KEY`,
  `SEARCH1API_API_KEY`, `SERPDIVE_API_KEY`, `TAVILY_API_KEY`,
  `TINYFISH_API_KEY`, `TOLLBIT_API_KEY`, `YOU_API_KEY`). These calls are
  billable. **Skill invocation** is behind the same `--live` gate: it
  executes a third party's program on this machine. A skill needs no
  credential; it needs an entry in the catalogue at
  `COMMONMEASURE_SKILL_CATALOGUE`. The internal corpus
  (`COMMONMEASURE_INTERNAL_CORPUS`) is behind neither gate: a `query` plan reads
  supply the operator owns, so it runs in the `no-external-acquisition` mode
  too.
- **Inference** happens only when `COMMONMEASURE_INFERENCE_ENDPOINT` names a
  reachable OpenAI-compatible chat-completions endpoint.
  The exact OpenRouter endpoint additionally requires `OPENROUTER_API_KEY`
  or `OPENROUTER_API_KEY_FILE`; these settings are never used for another
  destination. Its gateway identity is `openrouter`; the request format and
  unknown-field semantics remain unchanged. Direct authentication and routing
  limits are specified in [the benchmark contract](simpleqa-benchmark.md).
- **Encypher output signing** happens only with the job's explicit disclosure
  block and the operator's remote signing configuration. It sends the answer and
  source-record metadata and is independent of the acquisition `--live` switch,
  like inference. Its processor charge remains unknown and is not included in
  acquisition-cost ranking. See [the processor contract](processor.md#optional-encypher-signing).

When required acquisition or inference configuration is absent, the affected
plan is `unavailable` and carries a gap naming what is missing
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §5). Optional output signing records
its own unavailable invocation when permission or configuration is missing.
Signing abstention leaves an answered plan answered, with its answer unlabelled.

A skill run:

```sh
COMMONMEASURE_SKILL_CATALOGUE=demo/skills/catalogue.json \
  COMMONMEASURE_INTERNAL_CORPUS=demo/corpus \
  cargo run -p commonmeasure-cli -- run demo/jobs/skill-publishable.json \
  --live --output /tmp/commonmeasure-skills
```

Its catalogue names bundles installed on the machine that produced it, which
another operator must repoint (`demo/skills/README.md`).

## Directory

```text
<output>/
  manifest.json      the sealed comparison and its hash
  summary.json       this contract
  evidence.ndjson    append-only run log, one JSON object per line
  responses/<plan>.json   the exact provider response bytes, when one was received
                          (a plan named for a skill provider carries a colon, which
                          the filename replaces; `response_ref` names the file)
  replay.json        replay runs only: every sealed response bound to its recording
  provenance/<plan>.txt   suites declaring output_provenance: the answer with its C2PA
                          manifest embedded under Annex A.8, when it could be signed
  provenance/<plan>.c2pa  the same manifest store on its own
```

A run is written to a staging directory beside the output — `<output>.staging.<id>`,
the identifier being that run's own — and moved into place only once
complete. An interruption leaves the previous run intact
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §4).

One writer per output path. Each run stages under its own name, so two runs
publishing to one path never mix their files, and the path holds the last
complete run — the same replacement a second run into one directory makes
when the first has already finished. Two runs colliding both publish, and the
later one is the one held. More than two contending for one path can leave a
publish unable to take it; that run fails with the error and publishes
nothing, and the path still holds a whole run. Point concurrent runs at
separate output paths.

The order of the members within these files carries no meaning and is not
sealed: a build's JSON library decides it, and the dependency set the
workspace resolves can change it. Every hash here is taken over the canonical
form of the value ([`docs/contracts/canonical-json.md`](canonical-json.md)), so a file rewritten
in another member order still verifies against the seal it carries. Compare
runs by their hashes, never by their bytes.

`responses/` holds unredacted provider bodies, which may contain licensed
content; do not publish it without checking the supplier's terms. A skill
run's `responses/` holds no provider content: it is the output of local
programs run over the catalogued bundles, by children spawned with an empty
environment.

## Manifest

`manifest.json` holds `hash` and `manifest` (`contextops-manifest/v4`). The
hash is SHA-256 over the canonical JSON of the manifest
([`docs/contracts/canonical-json.md`](canonical-json.md)), so a reviewer recomputes it from the
sealed object with any conforming serialiser.

Manifest keys: `acquisition_mode`, `adapter_version`, `as_of`,
`cache_control`, `coverage_rubric`, `evaluators`, `governance`,
`inference_gateway`, `inference_request_shape`, `job`, `label`,
`manifest_version`, `model_plan`, `processors`, `replay_recordings`,
`result_limit`, `retry_policy`, `suite_version`, `supply_plans`,
`system_prompt`, `token_basis`, `transport_timeout_seconds`.

Manifest keys present only where the suite declares them: `fetch_target`,
`fidelity_judge`, `output_provenance`. A suite that declares none of the
three keeps the shape and the hash it had without them. `fetch_target` is the
exact URL every provider was asked to retrieve; `fidelity_judge` is set when
the suite declares the judge; `output_provenance` is the operator's
training-and-data-mining declaration for the run's own outputs
([`docs/contracts/processor.md`](processor.md) §`output-provenance`).

What the rest hold: `manifest_version` is this manifest's own shape;
`suite_version` and `label` identify the experiment, `label` being the name a
reader sees, so editing it publishes a different experiment under a different
hash. `job`, `supply_plans`, `model_plan` and `result_limit` are the
comparison. `system_prompt` is the composed prompt: the fixed instruction,
plus the citation directive when the job sets `require_cited_answer`.
`coverage_rubric` and `as_of` are the job's declarations, null when
undeclared, and are what the coverage and freshness evaluators measure
against. `governance` is the job's declared support-status rule set and
entitlement grant, null when undeclared, sealed here rather than in the
corpus so that changing one rule changes the hash. `acquisition_mode` says
whether the supply was live, replayed or not acquired at all, and
`replay_recordings` digests the recordings a replay run served.
`adapter_version` is the supply crate's. `processors` are the installed
processor manifests, each an identity and a configuration digest;
`evaluators` the installed evaluator identities
([`docs/contracts/experiment.md`](experiment.md)). `inference_gateway` is the gateway's name,
or null when none is configured. `inference_request_shape` is a digest of the
request body this runtime builds, taken over a fixed probe: the sampling
controls and the layout each admitted source is rendered into the window in.
It carries nothing about the job, and it moves when the request body changes,
because two runs that put the same sources in front of the same model in
different words are not one comparison. `token_basis`, `transport_timeout_seconds`,
`retry_policy` and `cache_control` state how footprints are counted, how long
an exchange may take, that a failed acquisition is recorded rather than
retried, and that provider caching is not controlled.

The gateway's name and presence are inside the hash: a run whose answers came
through a gateway and a run with no inference are different experiments, so
replaying a gateway-backed example without a gateway produces a different
hash (`gateway_presence_is_sealed_as_part_of_experiment_identity`,
`crates/commonmeasure-runtime/tests/replay_mode.rs`). The inference endpoint is outside
the hash: it is deployment configuration, and two machines running one job
are the same experiment.

Neither Exa, which serves `source: "cached"`, nor Firecrawl, which serves
`cacheState: "hit"`, offers a documented way to defeat its cache, which is
what `cache_control` records.

## Summary

Summary keys: `evidence_log`, `job`, `model_plan`, `plans`, `run`,
`schema_version`, `selection`, `suite_version`.

`run` carries `id`, `mode` (`live`, `no-external-acquisition` or `replay`),
`started_at`, `manifest_hash`, `fairness`, `token_basis` and
`evidence_complete`. In replay mode it also carries `replay_manifest`, the
path of the manifest the run served from.

`evidence_complete` is false when the log owes a gap it could not write, or
when the log could not be read back. It is read after the log has been
closed, so it accounts for the run's own terminal record
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §2).

`evidence_log` carries the closed log's `records` and `sha256`. The summary
attests to the log; the log is finished first.

`evidence_log.sha256`, `run.id` and `run.started_at` are the **declared
volatile fields**. `evidence_log.records` is not volatile: the same job
produces the same number of records. When acquisitions execute, the volatile
set also contains measured latencies and the identifiers and timestamps
inside decision and processor-invocation records. Everything else, the
semantic output included, agrees between two replay runs of one job against
one deterministically configured gateway
(`crates/commonmeasure-runtime/tests/replay_mode.rs`).

`job` is derived from the job's own constraint list, never restated as
literals; `crates/commonmeasure-cli/tests/run_contract.rs` checks the displayed values
against the job file.

`model_plan` carries `requested_model`, `gateway`, `endpoint` and
`configured`.

### Plans

Each plan carries:

- identity: `id`, `provider`, `capability`, `adapter_version`;
- `status`, exactly one of `completed`, `refused`, `unavailable`;
- `verification_state`, exactly one of the states in
  [`docs/contracts/provider.md`](provider.md);
- `eligibility`: whether the provider was allowed to be asked, and why;
- `acquisition`: `endpoint`, `http_status`, `latency_ms`,
  `provider_request_id`, `charge`, `response_ref`, `response_hash`,
  `response_bytes`, `result_count`; `null` when no acquisition happened.
  `http_status` is `null` when no HTTP was involved (a corpus query has no
  status code);
- in replay mode, `acquisition.replay`: the recording behind the sealed
  response: `source`, `captured_at`, `recorded_endpoint`,
  `recorded_response_sha256`, `redactions`, `permitted_use`, and
  `matches_recorded_input`, computed as sealed hash = recorded hash. A live
  or corpus acquisition has no `replay` key. A replayed acquisition's
  `endpoint` is the recorded endpoint, not the loopback origin's ephemeral
  port;
- for a provider that declares the `invoke` capability,
  `acquisition.invocation`: what was executed and under whose declaration. It
  carries the skill's `declared_name` (from `SKILL.md`) beside the operator's
  `catalogue_name`; `declared_version`, always null (the Agent Skill format
  has no version key; identity rests on `skill_md_sha256` and
  `entrypoint_sha256`, taken from the bytes read immediately before executing
  them); `declared_licence`, the bundle's own, which licenses the procedure;
  the `catalogue` that declared the invocation and the `root` it was
  contained to; the resolved `interpreter`, `entrypoint` and `cwd`; the
  verbatim `argv` with one `argv_source` per element (`operator` or `job`);
  the `environment` policy; the `exit_code`, null where a signal ended the
  child, with `termination` saying so; `stdout_bytes` and `stderr_bytes`
  with their hashes; and `duration_ms` against the declared `timeout_ms` and
  `maximum_output_bytes`. `requested_limit` records the plan step's result
  limit with `limit_applied: false`: one invocation produces one result.

  A non-zero exit is a result, not a run error. Only a failure to execute, an
  enforced timeout or a result over the declared cap makes the plan
  `unavailable`; an oversized result is refused whole, not truncated;
- for a provider that declares the `quote` capability, `acquisition.quote`:
  the quote-then-buy record, entered into evidence before the purchase
  decision. It carries the provider's `preview_id`, `workflow`,
  `billing_status`, verbatim `billing_note`, trial counters and
  `balance_before`; the quoted `charge` (basis `quoted`, in the provider's
  own unit); the sealed quote legs as `inspect_ref`/`inspect_hash` and
  `preview_ref`/`preview_hash`; and the `decision` (`confirmed`, `declined`
  or `not_attempted`) with its `decision_reason`. A declined quote is a
  `refused` plan whose `acquisition` holds only the quote record: the
  decision is in `policy_decisions` and its gap in `gaps`. A preview whose
  `workflow` is not the recorded `requires_confirm` buys nothing and offers
  none of the preview's sample content as supply: the plan is `unavailable`
  with the gap naming the workflow. The purchase decision is the plan's cost
  ruling; the post-hoc cap check does not run again over the receipt.

  `acquisition.quote.allowance` is the second pre-spend gate's record, kept
  apart from the cost ruling: the job's cap bounds one purchase, the
  principal's cumulative allowance bounds a period of them
  ([`docs/GLOSSARY.md`](../GLOSSARY.md) §Allowance). `consulted: false` records that the gate
  was not consulted: a run started without principal policy, or a replay,
  where no money moves. When consulted it carries the `principal`, whether
  an allowance is `declared`, and the `decision`: `reserved` (a reservation
  now held in the local ledger, its id recorded), `nothing_reserved` (trial
  coverage: a verifiable "no charge applies"), `declined` (strict refuses an
  exhausted allowance, an unknown price or an incomparable currency before
  money moves) or `proceeded_with_breach` (observe and prefer record the
  same facts and buy). `checks` is each declared period's arithmetic at the
  moment of the decision (period, period key in the declaration's own
  timezone, declared amount, spend already held, comparability and whether
  this purchase would exceed it). After settlement `settlement` carries the
  ledger's reconciliation note: the observed receipt committed, the
  difference from the quote stated, or the reservation released with its
  reason. The ledger writes a settlement twice before giving up; where it
  takes neither write, `settlement` is `reconciled: false` with the
  reservation id, the observed charge and the error, and the plan carries an
  `evidence_missing` gap stating that money moved and the ledger did not
  count it. The ledger's expiry sweep then releases the reservation and the
  period's committed total omits the charge. The ledger is
  `allowance/ledger.ndjson` under the operator home, append-only, one line
  per unit reserved, committed or released; it is local enforcement state
  and not part of the sealed run artefact.

  A remote search or fetch acquisition carries the same record as
  `acquisition.allowance`: where the adapter declares a spec-verified
  published price it is reserved before dispatch and reconciled against the
  observed receipt; where none is declared, an exhausted period refuses the
  dispatch under strict (`proceeded_with_breach` in observe and prefer) and
  an observed currency charge is committed on the receipt with nothing
  reserved. An unpriced dispatch is bounded only at the dispatch after it
  (`an_unpriced_provider_is_refused_only_once_the_allowance_is_exhausted`,
  `crates/commonmeasure-runtime/tests/allowance_dispatch.rs`). A corpus query and a
  skill invocation carry no allowance record: neither has a supplier price
  ([`docs/contracts/provider.md`](provider.md));
- `sources`: one record per discovered result;
- `source_count` (admitted sources) and `context_tokens_admitted`;
- `latency_ms`: the sum of the legs that ran. A plan whose inference did not
  happen reports its acquisition leg alone, and its `inference` is `null`;
- `inference`: the gateway record, or `null`.

  Inference keys: `answer`, `executed_model`, `executed_provider`, `gateway`,
  `input_tokens`, `latency_ms`, `output_tokens`, `provider_request_id`,
  `requested_model`, `route_reason`, `status`, `unavailable_fields`,
  `unreadable_fields`.

  A field the gateway did not
  report is `null` and named in `unavailable_fields`; a field it reported in
  a shape the runtime does not read — a fractional token count, a model that
  is not a string — is `null` and named in `unreadable_fields` instead. Both
  leave the value unknown, and the two lists say which happened, because an
  operator chasing a missing figure needs to know whether the gateway sent
  nothing or sent something unusable;
- `answer`: the gateway's output, or `null`;
- `policy_decisions`: one `DecisionRecord` per constraint that engaged;
- `processors`: one invocation record per processor invocation this plan
  caused ([`docs/contracts/processor.md`](processor.md)), in order. Empty for a plan that
  admitted nothing scannable and produced no answer. Every plan with an
  answer carries the `fidelity-verifier` invocation, the baseline included,
  the `fidelity-judge` invocation when the suite declares `fidelity_judge`,
  and the `output-provenance` invocation, naming the published label and
  its validation state, when the suite declares `output_provenance`;
- `gaps`: everything the plan needed and did not obtain;
- `evaluation`: one section per installed evaluator (`grounding`,
  `coverage`, `freshness`) on every plan, each a `contextops-evaluation/v1`
  record carrying its evaluator's identity (name, version, configuration
  digest), method, blind spots and the inputs it read. Every section is a
  pure function of job input and window content (no clock, no sampling), so
  none is in the volatile set. Runs published under `contextops-run/v3`
  carry the grounding record directly under `evaluation` and are read as
  they declare themselves.

  **`grounding`** holds one entry per `CITATION:` line the answer declared,
  each with exactly one of four verdicts: **supported** (the cited URL
  entered this plan's window and the quote occurs verbatim in that part's
  retained text; the record names the part's content hash and the byte span),
  **contradicted** (the cited URL entered the window but does not contain
  the quote), **uncovered** (the cited URL never entered the window),
  **unavailable** (the citation line does not parse; the line is carried
  verbatim). A plan where nothing was checkable (no answer, or a job that
  did not set `require_cited_answer`) is `unevaluated` with the reason
  stated.

  **`coverage`** measures the admitted window against the job's declared
  `coverage_rubric`: one entry per declared item, covered or uncovered, each
  covered item naming the matched phrase, the part and the byte span. The
  record names the rubric identity it measured against; the fraction is
  agreement with the rubric's author, not a measure of quality.
  `covered_count` / `item_count` / `fraction` exist only where measured. A
  job with no rubric, or a plan whose window was never assembled, is
  `unmeasured` with the reason stated; an assembled window that admitted
  nothing covers zero items.

  **`freshness`** measures each admitted part's declared date against the
  job's declared `as_of` (a reference date and a maximum age in days). Every
  part names its date's provenance: the supplier's own declaration where the
  adapter mapped one ([`docs/contracts/provider.md`](provider.md)), the operator's corpus
  manifest for internal supply, or the replay recording's capture date. The
  fraction exists only when every admitted part carries a parseable declared
  date: an undated part is unknown, not stale and not fresh, and is not
  dropped from the denominator, so a partially dated window publishes no
  fraction and is `unmeasured` naming the undated parts, with each dated
  part's own age and verdict recorded in `parts`. An empty window, an
  unassembled window and a job with no `as_of` are `unmeasured` with the
  reason stated.

### Verification states

A plan claims `live-verified` only when it completed a call against the real
supply and the exact response bytes are sealed in `responses/`. A replay
run's completed acquisitions claim `replay-tested`: the same sealed path fed
from a verified recording. Every other outcome is `planned`. A plan's state
is evidence about this run only.

### Charges

`acquisition.charge` has two optional halves:

- `money`: present only when the provider reported the charge in a currency,
  parsed from its own decimal text;
- `native`: the provider's own unit and number, with `basis` of `observed` or
  `quoted`.

Exa reports decimal USD. Firecrawl reports integer credits and no currency.
Tavily reports nothing, so its charge is `quoted` from the published price
with a note saying so. TollBit's search is not a priced operation. Redpine's
confirm receipt reports `cost_charged` and `balance_remaining`; a
trial-covered purchase is recorded as one `trial_queries` unit `observed`,
with the receipt verbatim in the note, never as a money charge of zero.

A skill invocation reports both halves absent: no supplier prices a local
execution. Both halves absent means the provider disclosed no price: unknown,
not zero ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §7).

### Sources

A source keeps `url`, `host`, `title`, `content_hash`, `licence`,
`retrieval_rank`, `tokens`, `admitted`, `admission_reason` and
`native_metadata`.

`licence.state` is `unknown` unless a supplier declared a machine-readable
licence as it handed over content. No open-web provider does. No adapter
promotes a domain name, `og:site_name` or a favicon into a rights claim. The
internal corpus can declare a licence state in its `corpus.json` manifest;
the declaration must be written down, because owning the directory is not a
rights statement.

A skill's source is the bundle that produced the result: a `file://` URL,
the empty host, and `licence.state` unknown. The bundle's `SKILL.md` may
declare a `license`, which licenses the procedure, not the output; it is
recorded in `acquisition.invocation.declared_licence` and never promoted
into a rights claim over the result. No `declared_date` attaches to a
produced result, so freshness is unmeasurable over a skill plan and a
freshness-weighted objective abstains naming the plan
(`crates/commonmeasure-runtime/tests/skill_supply.rs`).

Only standard output is the result. Standard error is sealed, sized and
hashed but never admitted as content.

`content_hash` is SHA-256 of the text as retrieved, before admission: a hash
over those bytes as they stand, not over a JSON document. It can be
recomputed from `responses/<plan>.json`.

`tokens` is the footprint the source would contribute: for a source that
passed the transform stage, the count after deduplication; for a refused or
duplicate source, the count of its retrieved text. `context_tokens_admitted`
sums the admitted, post-transform counts, and the context sent to the model
carries the transform's output hash, tied back to the retrieved text by the
optimiser's invocation record.

`native_metadata` keeps provider-specific fields namespaced rather than
normalised, so one provider's relevance score is never read as a
cross-provider measure.

### Selection

Selection keys: `explanation`, `method`, `objective`, `plan_id`,
`unavailable_inputs`.

`objective` is the job's own, restated so a reader need not open the job.
`method` names the arm that ran or abstained. `explanation` is the prose the
router wrote about this ranking: the candidates, their figures, the rubric
identity where one was used, and any plan left out.

The router selects a plan only when the job's objective is computable from
measurements this run obtained. Otherwise `plan_id` is `null` and
`unavailable_inputs` names what is missing.

Computable:

- **`minimise_latency`** and **`minimise_cost`** rank the completed plans on
  observed evidence. A candidate that withheld the measurement makes the
  router abstain naming it: an unreported charge is unknown, not free. Under
  replay the latency arm abstains, naming `latency`: a replayed exchange's
  latency times the loopback origin, not the recorded provider. A replayed
  charge is rankable: it is the recorded provider's own disclosure.
- **`weighted` over `coverage` and/or `freshness` alone** computes when
  every candidate carries the weighted measurements. Both are unitless
  fractions produced by this run's evaluators, so weighting them against
  each other is arithmetic on the declared weights; the selection's
  `explanation` names the rubric identity the ranking used and every
  candidate's figures. Joint maxima resolve to the first plan id in lexical
  order (the same rule the cost arm applies to equal charges), and the
  explanation states the tie whenever that rule decided the selection. The
  ranking reads the admitted windows, not the answers: a plan whose
  inference never ran is ranked on what it bought and admitted. A plan that
  neither ran to an answer nor carries a measured fraction is not a
  candidate; the `explanation` names each plan left out. A candidate
  without a weighted measurement (no rubric, no as-of date, or an empty
  window's undatable freshness) makes the router abstain naming the plan and
  the measurement.

Retained abstentions:

- **`quality`** weighted non-zero, and **`maximise_quality`**: no measure
  of answer quality exists. The installed evaluators measure citation
  support, rubric coverage and declared-date freshness; the fidelity
  verifier and judge record claim support and their agreement
  ([`docs/contracts/processor.md`](processor.md) §Status); none is mapped onto quality.
- **`policy_risk`** weighted non-zero: no measure exists beyond the recorded
  gaps.
- **fractions mixed with `cost` or `latency`**: a fraction has no price and
  no duration, and the runtime holds no exchange rate between them; the same
  clause refuses to weigh a currency against a millisecond when cost and
  latency are weighted together.

`crates/commonmeasure-runtime/tests/policy_and_selection.rs` proves each computable
arm and each retained abstention;
`crates/commonmeasure-runtime/tests/rankable_evaluation.rs` proves the committed
example's selection and its byte-identical evaluation records across two
replay runs.

## Replay binding

A replay run publishes `replay.json` beside its summary: the manifest it
served from, the provenance of every recording (source path, capture date,
redactions, permitted use), and one binding per replayed plan holding the
recorded input hash beside the sealed response hash with the computed
`matches_recorded_input`. A reviewer can start at `summary.json`, follow a
plan's `acquisition.replay.source` to the committed capture, and continue to
the capture's `README.md` and `NOTES.md` provenance.

## Evidence log

`evidence.ndjson` holds one JSON object per line, each with `seq`,
`timestamp`, `event` and `payload`. Every line is fsynced before the write
is acknowledged. The last record of a complete run is `run_completed`.
Processor invocations appear as `processor_invoked` records.

If a write fails, the failure window is held and materialised as an
`evidence_gap` record by the next successful write. If the terminal record
itself fails, one further append is attempted so the log records its own
gap, and `evidence_complete` is false either way ([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §2).

## Conformance

`crates/commonmeasure-cli/tests/run_contract.rs` runs the real binary into a temporary
directory and asserts this contract against what it wrote: documented keys
present, statuses and verification states drawn from the contract
vocabulary, nothing claimed with no dependency configured, no source
claiming a licence, no credential-shaped string in any artefact, no
accumulation on re-run, and two runs agreeing apart from the declared
volatile fields. This contract and the runtime change together.

## Current limitations

- Every evaluator is deterministic and narrow: grounding checks verbatim
  citations (paraphrase is invisible, uncited claims are not examined),
  coverage measures agreement with the job's own rubric, and freshness
  trusts declared dates (a supplier's claim, an operator's manifest or a
  replay capture date, none verified against when the content was written).
  Of the implemented providers, only TollBit's recorded search bytes declare
  dates. No evaluator measures answer quality. The fidelity verifier
  records per-claim verbatim support and the optional fidelity judge is
  measured for agreement against it ([`docs/contracts/processor.md`](processor.md) §Status);
  neither is a quality measure and neither feeds selection. A
  `coverage_gap` records that supply fell short of the request where the
  shortfall is knowable — a bounded corpus holding fewer matching documents
  than asked for, or a provider whose published search page is smaller than
  the job's result limit, the gap naming both numbers — and is distinct from
  the coverage evaluator's rubric measurement.
- `search` (every open-web adapter, Ozone Live, and Redpine behind its quote
  gate), `fetch` (Exa, Firecrawl, Linkup, Ozone Live, Parallel, Search1API and Tavily,
  dispatched only when the job names a `fetch_target`), `query` (the
  internal corpus), `quote` (the Redpine adapter's quote-then-buy gate) and
  `invoke` (catalogued local skills) are implemented. The rest of the
  capability vocabulary (`licensed`, `report` and `corroborate`,
  [`docs/contracts/provider.md`](provider.md)) is not declared or implemented by any
  adapter.
- Skill invocation is a supervised spawn, not a sandbox. The child runs as
  the operator, on the operator's filesystem, with whatever access the
  operator has. What is enforced: containment of the entrypoint to the
  declared root, an absolute interpreter, an explicit argument vector with
  no shell parsing it, an emptied environment, a wall clock ended by
  signalling the child's process group, and an output cap that refuses a
  result whole. What remains open is stated in
  [`docs/contracts/provider.md`](provider.md#skill-supply) §Skill supply.
- A skill's identity is not sealed by the manifest. The manifest names the
  supply plan, so it records that a skill provider ran; the digests of what
  ran are observed per run, in `acquisition.invocation`. Repointing a
  catalogue entry does not change the manifest hash, and a reader detects it
  by comparing those digests between runs.
- Token footprints are whitespace words, not model tokens, and every
  artefact that carries one names that basis.
