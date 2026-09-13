---
title: Open-source component landscape
draft: true
---

# Open-source component landscape

As of 21 August 2026. This compares components that Common Measure could
implement, use as a library, run as a sidecar or expose through a processor.
Every candidate still needs a licence, maintenance, security and performance
check against a pinned version.

## Selection criteria

The core must preserve what makes the product distinct:

- context jobs and supply plans, not generic LLM requests;
- enforcement before the context crossing;
- source, licence, price and transformation evidence;
- controlled comparisons with reproducible manifests;
- separate private evidence and Content Telemetry egress;
- explicit unknowns and assurance grades;
- local/customer-controlled deployment;
- swappable components behind narrow contracts.

We should borrow commodity execution and specialist models. We should own the
decision semantics, evidence model, experiment design and projections.

## Model routing and inference gateways

| Candidate | Shape | Fit | Recommendation |
|---|---|---|---|
| TensorZero | Rust gateway plus observability, evals, experiments and optimisation | Closest technical and product fit; Rust, OpenAI-compatible, self-hosted, Apache-2.0. Also overlaps most heavily with the experiment and console components | **Selected first sidecar; live-verified.** Keep ContextJob, context evidence and telemetry outside it |
| Bifrost | Go gateway with model catalogue, governance, adaptive load balancing, plugins and MCP gateway | Strong fast execution layer; narrower conceptual overlap than TensorZero | **Bake off second. Run as a sidecar.** Good if we want to own evaluation and optimisation above it |
| LiteLLM | Python SDK/proxy with very broad provider compatibility, routing, cost and guardrails | Widest compatibility and common ecosystem baseline; large Python dependency and rapid surface churn | **Compatibility benchmark, not default core.** Useful fallback and test oracle |
| Portkey Gateway | Lightweight gateway with routing, retries, load balancing and guardrail plugins | Mature integration surface; open/enterprise boundary and 2.0 transition need scrutiny | **Watch/bake-off optional.** Do not couple the product to its guardrail catalogue |
| RouteLLM | Python framework for learned strong/weak model routing and evaluation | Useful algorithm and benchmark for cost/quality routing, not a full control/evidence plane | **Use its ideas and offline benchmarks; keep it out of serving** |

Primary sources:

- https://github.com/tensorzero/tensorzero
- https://github.com/maximhq/bifrost
- https://docs.getbifrost.ai/providers/provider-routing
- https://github.com/BerriAI/litellm
- https://github.com/Portkey-AI/gateway
- https://github.com/lm-sys/RouteLLM

### Recommendation

Do not build provider SDK compatibility, key rotation, retries or low-level
load balancing. Keep our small `InferenceBackend` contract. TensorZero runs
behind it; Bifrost is the second candidate.

TensorZero is the more interesting first test because its Rust implementation,
evaluation system, A/B testing and feedback loop closely match the stack we
would otherwise build. That is also the risk: adopting it wholesale could make
Common Measure dependent on another platform's product model. Common Measure must retain
authority over its run manifest and evidence ledger while TensorZero handles
inference execution and selected experiment infrastructure.

Bifrost is attractive if we want a deliberately thinner gateway and to own the
experiment plane. Its model catalogue, governance and load balancing are still
gateway-local signals, not substitutes for job outcome optimisation.

Further Bifrost sources:

- https://docs.getbifrost.ai/features/governance/routing
- https://docs.getbifrost.ai/providers/supported-providers/overview

### Boundary between a gateway and this product

A model gateway answers: where and how should this model request execute?
Common Measure answers: which context and model plan best satisfies this context
job under the operator's quality, rights, risk, cost and latency objective?
A gateway can route the same model between inference providers, balance
keys and fail over. Common Measure holds that model route fixed while comparing
context providers, or compares model plans after the context effect is
understood.

### Experimental discipline

Adaptive gateway routing can invalidate a comparison if it silently changes
providers. Every controlled run therefore pins an explicit route or records
the complete gateway decision, catalogue/configuration version, fallback
chain and provider-level timing. Semantic caches are disabled or made an
explicit experimental variable. The sequence is: fixed model, vary context
plan; fixed context plan, vary model plan; a small factorial matrix across
both; only then a joint optimiser. The console must show whether a cheaper
or better outcome came from context, model selection or an interaction
between them.

## Context-window optimisation

The evidence behind this section — what degrades a context window, what the
benchmarks do and do not measure, and which of these operations earn their
cost — is [`docs/guide/context-window-optimisation.md`](../guide/context-window-optimisation.md). This section holds only
the component choices.

There are four separate operations often hidden under “context optimisation”:

1. deduplicate and normalise content;
2. rank sources or passages against the job;
3. select and order evidence within a token budget;
4. compress selected text.

They must remain separate transformations in the evidence record. Extraction
precedes all four and is larger than any of them: measured over four live
pages, 95% of the tokens in retrieved HTML are markup and boilerplate (guide,
§1).

| Candidate | Operation | Recommendation |
|---|---|---|
| deterministic hashing, canonicalisation and near-duplicate rules | deduplication | **Build in Rust.** Cheap, inspectable baseline required by the evidence system |
| trafilatura | main-text extraction | **Python sidecar or equivalent.** F1 0.958 against 0.50 precision for a plain HTML-to-markdown converter; the largest single token saving in the pipeline |
| `rerankers` / FlashRank-compatible models | passage reranking | **Python sidecar plugin.** Unified API makes model comparison easy; retain native scores and model identity. Note FlashRank's last code change was 2025-09-15 |
| ColBERT/PyLate | late-interaction retrieval/reranking | **Experimental plugin.** Valuable for bounded corpora, too heavy for the first generic fetch path |
| LLMLingua / LongLLMLingua / LLMLingua-2 | token/context compression | **Python sidecar plugin, low priority.** One commit since October 2025. The “20x” headline is a 2023 result on few-shot reasoning prompts; the same team's successor claims 2–5x. Test against no-op, truncation and extractive selection |
| Chonkie | chunking and refinement | **Experiment utility, not runtime dependency initially.** Relevant when we own corpus ingestion rather than live external retrieval |

Maintenance status above was checked using commit participation
(`repos/OWNER/REPO/stats/participation`), not `pushed_at`, which a README or
CI edit resets.

Primary sources:

- https://github.com/microsoft/LLMLingua
- https://github.com/AnswerDotAI/rerankers
- https://github.com/stanford-futuredata/ColBERT
- https://github.com/lightonai/pylate
- https://github.com/chonkie-inc/chonkie
- https://github.com/adbar/trafilatura

### The PyPI `commonmeasure` linter

github.com/Abhijeet777ui/contextops is a deterministic, embedding-free
structural linter for an assembled LLM context payload: four capped penalty
dimensions — redundancy (near-duplicate items), density (formatting and
boilerplate token waste), structure (retrieval flooding, system-prompt
bloat, tool sprawl), concentration (over-reliance on one source) — summed
into a single 0–100 score. No model calls, two runtime dependencies,
actively shipping. Its README lists Claude Code and Codex users under "who it's not for"
because it cannot intercept a closed host's payload; the harness plugin
covers exactly that case. It also shares our name, binary name,
`~/.commonmeasure/` home and `COMMONMEASURE_` prefix
([`docs/knowledge-base/naming-collision.md`](naming-collision.md) §Practical collisions).

**Licence bars use.** It is under a Sustainable Use License (fair-code, not
open source): internal/non-commercial use
only, no inclusion in a commercial product or service whose core value
derives from it, distribution only free of charge and non-commercial.
Vendoring, bundling or shipping it as an add-on is prohibited. Internal use
— dogfooding, benchmarking, validation — is permitted.

**Recommendation:**

- **Do not vendor or bundle.** The licence forbids it and the name
  collision makes even an operator-installed integration adapter awkward
  (two `commonmeasure` binaries on one PATH).
- **Implement the concepts ourselves where wanted.** The four structural
  analyzers are deterministic, unprotectable ideas that fit the existing
  transform-stage optimiser and our rules — with one deliberate
  divergence: we keep the per-dimension results as raw evidence and never
  collapse them into a single universal score
  ([`docs/contracts/processor.md`](../contracts/processor.md)).
- **ContextBench is the interesting piece.** The repository carries a
  1,500-sample benchmark with evaluator-agnostic ground-truth labels for
  structural failure modes, plus a 9,500-payload adversarial companion
  (ContextSecBench: prompt-injection hiding, truncation smuggling, context
  poisoning) directly relevant to the injection screen. Validating our
  processors against it is internal use; verify the benchmark data's own
  licence before anything beyond that.
- **The marketplace relationship, if any, is author-listed.** Under the
  processor contract a third party lists their own tool under their own
  licence; we distribute nothing. Contacting the author is an operator
  action ([`docs/knowledge-base/naming-collision.md`](naming-collision.md)).

Primary sources:

- https://github.com/Abhijeet777ui/contextops
- https://pypi.org/project/contextops/
- https://raw.githubusercontent.com/Abhijeet777ui/contextops/main/LICENSE

### Required evidence

Every optimiser emits a transformation record: input hashes and token counts,
output hashes and token counts, removed source spans, ordering changes, model
and configuration identity, latency and cost. The original envelope remains
addressable. A transformation cannot silently replace provenance with a blob.

Optimisation is evaluated on downstream job outcome, citation support and
claim recall—not token reduction alone. Compression that removes the one fact
needed to answer is not optimisation. One measured instance: after a
compaction pass, three of three high-level facts survived and none of three
appendix-table details did (guide, §8).

## Verification and hallucination checks

“Hallucination detection” is too broad to be one score. The useful quick win is
a verification plugin that decomposes an answer into claims and records, per
claim:

- supported by a named source span;
- contradicted by a named source span;
- not covered by the supplied context;
- verification failed or unavailable.

| Candidate | Strength | Recommendation |
|---|---|---|
| RefChecker | modular claim extraction, checking and localisation; support/refute/neutral framing | **First research adapter.** Its evidence shape closely matches ours; use offline, not inline enforcement initially |
| RAGChecker | claim-level retrieval and generator diagnostics including recall, utilisation, noise and faithfulness | **Experiment evaluator.** Strong for comparing supply plans rather than per-request blocking |
| HHEM open model | cheap local factual-consistency signal for response against supplied text | **Optional fast signal.** Never present as universal truth or sole gate |
| Ragas | broad RAG metrics and integrations | **Test oracle/compatibility adapter**, not canonical metric semantics |

Primary sources:

- https://github.com/amazon-science/RefChecker
- https://github.com/amazon-science/RAGChecker
- https://github.com/vectara/hallucination-leaderboard
- https://github.com/vibrantlabsai/ragas

### Quick-win implementation

Build the verification **contract and evidence UI** ourselves; run checkers as
plugins. Start with deterministic checks—citation URL present, source retrieved,
content hash retained, cited substring exists—then add claim extraction and
entailment. A checker verdict is an evaluation with method and confidence, never
a rewrite of the source record and never proof that an unsupported claim is
false.

## Provenance

| Candidate | Role | Recommendation |
|---|---|---|
| `c2pa-rs` | create, parse and validate C2PA manifests and CAWG identity assertions | **Use the official Rust library**, behind our provenance plugin. Check its MSRV against the workspace `rust-version` before choosing in-process use or an isolated worker |
| Encypher | independent verification, text/sub-document markers, signing and C2PA services | **Provider plugin and corroboration partner**, not a proprietary core dependency |
| Content Telemetry | report observed retrieval/grounding/citation lifecycle | **Projection standard**, not asset provenance or truth verification |

Primary sources:

- https://github.com/contentauth/c2pa-rs
- https://opensource.contentauthenticity.org/docs/rust-sdk/docs/usage/
- https://encypher.com/solutions/ai-companies

C2PA proves signed claims, asset integrity and provenance history within its
trust model. It does not establish that content is factually true. Encypher can
add independent verification and sub-document text attribution. Common Measure
joins those signals to the observed crossing without collapsing provenance,
licence, factual support and source authority into one score.

## Agent harnesses

Agent harnesses are not components to embed but hosts Common Measure attaches
to, and the most advanced of them now build parts of the same discipline
themselves.
QM (`https://github.com/yc-software/qm`, MIT) is the reference example:
provenance-labelled admission screening, tighten-only policy scopes, four
coding harnesses behind one mediable tool surface, and a published
screening-proxy wire contract an external policy service serves by
configuration alone. It caps spend but never values what was bought, and its
audit is best-effort rather than evidence-grade. The full read, including
integration sockets and patterns worth adopting, is
[`qm-harness-review.md`](qm-harness-review.md) in this directory.

## Build / borrow boundary

### Build and own

- ContextJob, SupplyPlan, ModelPlan and policy semantics;
- provider and plugin capability contracts;
- append-only decision/transformation/evaluation evidence;
- experiment manifests and fairness rules;
- optimisation objective and explanation;
- assurance grades and negative-space/gap reporting;
- operator console projections;
- Content Telemetry egress boundary.

### Run or import behind plugins

- model gateways;
- rerankers and compression models;
- claim extractors, NLI and factual-consistency models;
- C2PA parsing/signing libraries;
- Encypher and other independent verification services.

### Exclude

- entire agent frameworks;
- a second generic observability product;
- vendor dashboards as our system of record;
- marketplace settlement or publisher repositories;
- binary “truth” scores with no inspectable evidence.
