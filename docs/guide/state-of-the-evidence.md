---
title: The state of the evidence, August 2026
---

# The state of the evidence, August 2026

*Companion to the working guide. That document teaches the subject; this one
grades the evidence behind it, claim by claim.*

The guide is [`context-window-optimisation.md`](context-window-optimisation.md).

Context-window research is published faster than it is reproduced. Most
recent results have not been independently checked.

The useful question is which parts of the research can be built on. This
document answers that claim by claim.

## How to use it

Look up a claim before you cite it, design around it, or let it change your
architecture. The grade tells you how far the claim can be relied on.

Three limits. **"Replicated" means an independent reproduction was found,
not that one was run for this document.** **Absence of replication usually
means the field is young, not that a claim is wrong**: most single-study
entries here are probably true. And this is a snapshot: several grades will move within months,
which is why each entry carries a date.

## The grades

| Grade | Meaning |
|---|---|
| **Replicated** | An independent group reproduced the effect, ideally by a different method |
| **Single study** | One team. No independent reproduction found. Probably true; do not base a large decision on it |
| **Contested** | A second study disagrees, and nobody has reconciled them |
| **Refuted** | A reproduction attempt failed, or explained the original away |
| **Vendor** | Reported by a party selling something, with competitors configured by that party |
| **Practitioner report** | A working system described by the people who built it. Design rationale, no measurement |
| **Folklore** | Widely repeated, no traceable primary source |

The Vendor grade records who chose the experiment. It does not question the
reporter's integrity. Some vendor numbers are the best available: the API
pricing and caching mechanics in chapter 9 are vendor documentation and are
authoritative, because the vendor owns the product.

Practitioner reports are worth reading and are not evidence. Much of the most
actionable material in this field is somebody describing a system that works
for them, with no baseline and no numbers. Copy the design if it suits you. Do not
assume every part of it is doing something — nobody tested which parts matter.

## Contents

1. [Do not cite these](#1-do-not-cite-these)
2. [Long-context degradation](#2-long-context-degradation)
3. [Benchmarks](#3-benchmarks)
4. [Acquisition and retrieval](#4-acquisition-and-retrieval)
5. [Extraction and chunking](#5-extraction-and-chunking)
6. [Ranking, selection and ordering](#6-ranking-selection-and-ordering)
7. [Compression](#7-compression)
8. [Caching and economics](#8-caching-and-economics)
9. [Agent memory](#9-agent-memory)
10. [Evaluation methodology](#10-evaluation-methodology)
11. [What changed in the last year](#11-what-changed-in-the-last-year)
12. [Open problems](#12-open-problems)
13. [Where to start reading](#13-where-to-start-reading)

---

## 1. Do not cite these

Each of these is repeated confidently in current writing. Checking finds none
of them supported as stated.

| Claim | Status | What is actually true |
|---|---|---|
| **"Adding random documents improves RAG accuracy, by up to 35%"** | **Refuted** | Reproduced under the original setup, then shown to vanish under modern prompting. A 15-token generation cap, no chat template and a forced no-answer instruction were penalising the low-document baselines. Under normal settings the effect is −0.23% to −1.30%, and truncated outputs were 53.6–73.6% of errors. SIGIR 2026 |
| **"Effective context is 60–70% of advertised"** | **Folklore** | No traceable source. The attributable figures are lower and narrower: half of models claiming 32K fail at 32K (RULER 2024); 10 of 12 fall below half their short-context baseline at 32K without lexical overlap (NoLiMa 2025) |
| **"Code is about twice as token-dense as prose"** | **Folklore** (untraceable) | No source for the original. Measured here on nine files in one repository: 20–40% denser per character, prose 191–231 tokens per 1,000 characters against code 246–271, and a lockfile at 372. Nine files does not settle it either |
| **"A web page is about 80% boilerplate"** | **Folklore** (untraceable) | No primary measurement, and it refers to text rather than tokens. Removing tags, scripts and other non-content HTML removed 87–99% of tokens on four pages here, median 92%; the original HTML inputs were not retained |
| **"Context poisoning"** | **Folklore as an effect**, useful as a term | Every citation traces to one anecdote about an agent playing Pokémon, which does not appear in current versions of the technical report it is attributed to. No controlled study injects a false fact and measures propagation. Attribute the term to Breunig, not to a lab |
| **"Anthropic documents degradation past 30–50 tools"; "keep it under 20 per turn"; "5–7 MCP servers is the ceiling"** | **Folklore** | No primary citation resolves. The defensible numbers are a 7–85% range (LongFuncEval) and an elbow at 40–60 agents on one production catalogue, whose authors warn it is catalogue-specific |
| **"LLM judges agree with humans about 80% of the time"** | **Over-generalised** | From GPT-4 pairwise preference on one 2023 benchmark. It is also a *ceiling set by human disagreement* (81% human–human), not a quality guarantee. 2026 studies on other tasks report 71% agreement and κ=0.51 |
| **"Temperature 0 gives you reproducibility"** | **Refuted** | A thousand completions of one prompt at temperature 0 produced 80 unique outputs. Kernel results depend on server batch size |
| **"LLMLingua gives 20× compression with little loss"** | **Superseded** | True as published, on few-shot reasoning prompts, which are highly redundant text. The same team's successor claims 2–5× |
| **"Lost in the middle" as a live constraint on text RAG** | **Contested** | See §2. A 2026 reproducibility study across five models found performance "nearly flat across placements" |

---

## 2. Long-context degradation

| Claim | Grade | Evidence |
|---|---|---|
| Length alone degrades performance, holding task and evidence constant | **Replicated** | Three independent groups, three methods: padding a two-fact problem drops accuracy 0.92→0.68 by 3,000 tokens (Levy, ACL 2024); word-replication accuracy falls from ~100% to 40–60% between 25 and 10,000 words across 18 models (Chroma, Jul 2025); replacing irrelevant tokens with whitespace and masking attention still costs 13.9–85% across five models (Du, EMNLP 2025) |
| Effective context is far short of advertised | **Replicated** | RULER (NVIDIA, Apr 2024): only half of models claiming 32K+ hold at 32K. NoLiMa (Adobe, Feb 2025): 10 of 12 below half their short-context baseline at 32K once lexical overlap is removed. An et al. (Oct 2024) give a mechanism — long relative positions are rare in training data — and recover >10 RULER points by remapping offsets |
| Too many tool definitions degrade selection | **Replicated** | IBM LongFuncEval (Apr 2025): 7–85% drop as tool count grows. Independent production catalogue of 584 tools (Jun 2026): routing F1 58.2%→42.1%, recall-driven |
| ~10 points of tool-selection degradation survives a perfect shortlister | **Single study** | One catalogue, one paper. If it generalises, retrieval has a ceiling here and the remedy is deduplicated descriptions. Authors warn the numbers are catalogue-specific |
| Attention sinks do necessary work rather than being a defect | **Single study; widely adopted mechanism** | Mechanism proposed 2023 and explained through over-mixing in 2025. No independent reproduction found; adoption is implementation evidence, not replication |
| Multi-turn conversation costs ~39%, mostly as unreliability | **Single study** | 200,000+ simulated conversations, 15 models. Decomposition: aptitude −16%, unreliability +112%. Large and careful, but one team |
| Pruning context can *raise* accuracy | **Single study** | 63% fewer tokens, +20.6 points, on a 50-task production benchmark; replicated internally across three categories and two models. No independent reproduction |
| Perfect-instruction-following collapses by ~80 simultaneous instructions | **Single study** | Single-author preprint, Jul 2026. The failure mode near capacity was refusal, not fabrication: 0 of 5,760 probes invented anything |
| The U-curve is a property of causal attention geometry, present at initialisation | **Single study** | Theoretical, Mar 2026, with an empirical check on untrained networks. Not independently verified, and it is in tension with §3's failure to reproduce the U-curve behaviourally |
| Degradation curves are model-specific, not universal | **Replicated** | Databricks (Nov 2024): five models improved to 100K while two degraded after 32K, failing in different ways — refusals for one, instruction-following collapse for another. Consistent with every later study that tested more than one model |
| Effective context length is improving faster than window size | **Single study** | Epoch AI, Jun 2025, 123 models: advertised windows growing ~30× annually, usable length at 80% accuracy improving over 250× in nine months. Read this against the degradation rows above |

**Contested: does position matter for current models on text?** Liu et al. (2023) established the U-curve on 2–6K-token inputs with GPT-3.5 and Claude-1.3. A SIGIR 2026 reproducibility study across five current models and two datasets found performance "nearly flat across placements", and diagnosed topic sampling as the dominant variance source — stable conclusions needed 1,000–2,000 topics. Against that, a May 2026 audit of nine models found end-to-middle drops on *reasoning* tasks. These may be compatible; nobody has reconciled them. What is not contested: position effects are large for tool results (oldest ignored) and for multimodal input (strong primacy).

---

## 3. Benchmarks

| Claim | Grade | Evidence |
|---|---|---|
| Long-context benchmarks disagree with each other | **Single study**, methodologically strong | HELMET (ICLR 2025), 59 models: no synthetic task correlates above 0.8 with downstream performance; RULER correlations all below 0.85; HELMET's own seven categories correlate poorly with each other |
| Needle-in-a-haystack is saturated and mostly measures string matching | **Replicated** | RULER (2024) and NoLiMa (2025) independently. One vendor-published result illustrates it: perfect needle retrieval to 1M tokens alongside 19.0% on multi-hop traversal above 128K, same model |
| Long-output and state-tracking tasks are far harder than long-input tasks | **Single study** each | LoCoDiff: all models below 50% above 25,000 tokens on exact file reconstruction, on a benchmark whose largest prompt is 97,500. RECON: best non-oracle system 22.4% on source-conflict tasks |
| Agentic reliability is much worse than agentic capability | **Single study** | τ-bench `pass^k`: under 50% at one attempt, under 25% at eight. Almost no other benchmark reports this statistic, so nobody has tried to reproduce it |
| Benchmark contamination is real and measurable | **Single study** | Models identify buggy file paths from issue text alone at 76% inside SWE-bench and 53% outside |
| Contamination detection does not work | **Single study** | Three detection methods, 25 models: only 201 of 335 evaluations correct. The authors conclude that statistical auditing cannot replace transparent provenance |
| Vendors report on benchmarks they own | **Observation, not a study** | MRCR and Graphwalks are vendor-published datasets, corrected twice without announcement, so figures published before and after those dates are not comparable. One major 2025 launch reported no long-context benchmark at all |
| Only one mainstream leaderboard publishes cost | **Observation** | Terminal-Bench: two systems statistically tied on accuracy at a 3.7× cost difference. Every other benchmark would rank them identically |

---

## 4. Acquisition and retrieval

| Claim | Grade | Evidence |
|---|---|---|
| Long context beats chunked retrieval on quality, given budget, but they agree far more often than they differ | **Single study** | 7.6–13.1 points across nine datasets; **63% of queries produced identical answers**, 70% differed by under 10 points |
| Routing per query captures most of the benefit at a fraction of the cost | **Single study** | Self-Route: 38.6% of tokens (65% cost cut) at near long-context quality; over half of queries answerable from retrieval alone. One extra model call. The largest reported saving for the least added mechanism, and unreproduced |
| The crossover sits around 128K and moves with model size | **Single study** | LaRA, 2,326 cases, 11 models. Weak models gained 6.5–38.1% from retrieval at 128K; frontier models favoured long context at 128K |
| Retrieval leads on hallucination-sensitive tasks | **Single study** | 10–22% advantage, attributed to withholding distractor mass |
| Reader accuracy saturates well before retriever recall | **Replicated** | Liu et al. 2023 (20→50 documents adds +1.5%/+1% while recall climbs), and consistent with every distractor study since |
| Semantically similar wrong passages are the distractors that cost most | **Replicated** | Chroma's distractor experiments and an independent hard-negative study (over 60% of queries had one in the top ten) |
| Contextual chunk headers cut retrieval failure 35–67% | **Vendor** | Anthropic's own evaluation. Mechanism is sound and cost is stated ($1.02 per million document tokens). No independent reproduction found |
| Contextual chunking *hurts* in-document retrieval | **Single study** | SIGIR 2026 taxonomy paper. Important qualification to the row above: it helps across a corpus and harms within one document |
| Multi-query fusion is a retrieval-metric win that reverses on the end metric | **Single study** | Hit@10 fell 0.51→0.48 despite higher retrieval recall, because the reranker and token budget were already binding |
| Prompted agentic retrieval does not beat good one-shot retrieval | **Single study** | 91.0% one-shot against 90.0% iterative, with 59–97% more retrieval calls; combining them was worse than either. Trained search policies do win (+41%); that is a different claim, often conflated with this one |
| GraphRAG's advantage is preference-based, not accuracy-based | **Vendor**, and methodologically weak | Comprehensiveness/diversity/empowerment are LLM-judge pairwise preferences on questions with no ground truth. Faithfulness, the one objective axis, came out at parity. Microsoft's own successor prices the original's indexing at 1,000× |
| Keyword search overtakes agentic search above ~10M corpus tokens | **Single study**, days old | 28 nested corpus tiers; ~20-point margin at full scale; agent spends 39× more query tokens. If it replicates it bounds where sophisticated acquisition helps |
| Search providers return substantially different hosts | **Single study** (this guide) | 24 queries, three providers: pairwise host overlap 0.18–0.62, union 15.5 hosts per query against 8–9 each. Small sample, one run, English only |
| Ingestion-side quality gating beats filtering at query time | **Practitioner report** | Embedding content only if it clears a threshold — rare terms, minimum length, some signal of value. Cheaper than query-time filtering because it runs once per document, and it removes the low-signal material distractor studies say costs most. Widely done, never measured; the guide's chapters argue admission almost entirely at query time |
| Rewriting heterogeneous content into a canonical shape before embedding beats embedding it raw | **Practitioner report** | Reported as a significant accuracy gain with no figure. An extension of contextual retrieval, which is vendor-measured; cite that instead |

---

## 5. Extraction and chunking

| Claim | Grade | Evidence |
|---|---|---|
| A real extractor roughly halves token cost against a plain HTML-to-markdown converter, and can cut it tenfold | **Single study**, 28 extractors | Zyte benchmark, 181 pages, four-gram shingle scoring. Read the precision column: trafilatura 0.938 against converters at 0.10–0.52. Benchmark archived June 2026; unmaintained |
| Removing tags, scripts and other non-content HTML cuts raw-page tokens by 87–99% | **Single study** (this guide) | Four pages, crude tag strip, named tokeniser. Navigation and other non-content elements remained, and the original HTML inputs were not retained |
| A recursive splitter at 200–400 tokens is a strong baseline | **Single study** | Chroma, 5 corpora, 472 queries: 88.1% recall at 7.0% precision, within 4 points of every more complex method. LLM-guided chunking gained 3.8 points of recall and halved precision |
| Late chunking gives small, inconsistent gains | **Vendor** plus one independent head-to-head | +2 to +6.5 nDCG@10, and zero on one dataset. Gains grow with document length. The independent comparison concluded "neither technique offers a definitive solution" |
| Aggressive cleaning drops whole pages undetected | **Single study** | Taking the union of several extractors raised token yield up to 71% with no benchmark regression |
| Extraction quality varies far more on structured pages than on articles | **Single study**, with a disclosed conflict | Top systems converge at F1 0.93 on articles and spread to 0.41–0.84 on forums, product and listing pages. The author also wrote the top-scoring extractor |

---

## 6. Ranking, selection and ordering

| Claim | Grade | Evidence |
|---|---|---|
| Reranker quality flattens quickly with model size | **Reproducible** — public model card | 74.30 against 74.31 nDCG@10 between a 6-layer and 12-layer cross-encoder, at 1,800 against 960 documents per second. Anyone can rerun it |
| A dedicated cross-encoder beats a frontier LLM as reranker on quality, latency and cost together | **Vendor**, direction corroborated | 12–13% nDCG@10, 36–48× faster, 25–60× cheaper. Competitors configured by the vendor. A second vendor reports the same direction independently |
| Instruction-following is worth roughly as much as a model-generation upgrade | **Vendor** | Single-vendor evaluation. Mechanism plausible; magnitude unverified |
| Late interaction's controlled advantage over a same-size dense model is about one point | **Single study**, clean design | 57.22 against 56.20 nDCG@10, same backbone, same size. Larger claimed advantages usually compare against weaker baselines |
| Late interaction is widely used for visual documents | **Adoption evidence** | It skips OCR and layout parsing rather than competing with them. Publication activity suggests adoption, but does not establish that it should be the default |
| Submodular selection beats greedy top-k and MMR under a token budget | **Three independent single studies agreeing in direction** | All three formalise packing as knapsack-constrained subset selection; all three beat top-k and MMR |
| Retrieval recall and end-to-end accuracy decouple | **Two independent studies** | Facility-location selection had *lower* recall than top-k (0.909 against 0.933) and higher accuracy (52% against 50%). Separately, a 4.6× exact-match gap among questions where all gold documents were retrieved |
| Byte-exact deduplication removes 0.16% / 24% / 80% by corpus type, with no quality loss | **Single study**, four vendors tested | The three-regime split is the useful part: clean academic, enterprise, conversational |
| Cache-aware evidence ordering beats relevance ordering on cost at no quality loss | **Single study** | 20–33% median time-to-first-token reduction, 97.5% of oracle gain. Set against ordering's at-most 2–3 F1, this settles the trade-off in practice |
| Similarity ranking surfaces cited passages worse than random | **Single study**, one domain | Legal QA. If it generalises it matters a great deal for any product showing citations |
| Multimodal retrieval shows strong primacy — 16–26 points | **Single study**, five models | Opposite direction and an order of magnitude larger than any text ordering effect |
| Scaffolding harms by displacing evidence, not by being present | **Single study**, decisive ablation | Three commercial models stayed correct at a 95% coordination ratio when coordination tokens sat outside the evidence budget |

---

## 7. Compression

| Claim | Grade | Evidence |
|---|---|---|
| "Up to 20× compression with little performance loss" | **Superseded by the same team** | Measured on few-shot reasoning prompts — formulaic, highly redundant. The successor claims 2–5×. Treat 20× as a best case on favourable text |
| Extractive compression to ~6% of original with minimal loss | **Single study** | RECOMP, 2023. Its more useful contribution is returning an empty string when nothing is relevant, which is admission control rather than compression |
| Soft and learned compression works but needs model weights | **Multiple single studies** | Gist tokens, ICAE, xRAG. Unavailable through any hosted API, which is why it does not appear in practitioner writing |
| Compression removes specifics and keeps gist | **Vendor**, single run | After one compaction pass: 3 of 3 high-level facts preserved, 0 of 3 appendix-table details. Matches intuition, which is a reason to want it reproduced rather than a reason to trust it |
| Sufficient-context measurement separates retrieval failure from generation failure | **Vendor** (Google) | Automated rater at 93% accuracy; models answer correctly 35–62% of the time on insufficient context; one model's hallucination rate rising 10.2%→66.1% with insufficient context. Google's rater, Google's evaluation, and Google ships retrieval products. It is also the strongest single support the guide gives its own thesis, which is a reason to want it reproduced |
| Tool-schema compression is worth ~20 points of exact match at tight budgets | **Single study** | +20.5pp at 8,000 tokens against 2.6% uncompressed. May 2026, unreplicated |
| Context can be cut to ~75% with little loss and ~35% is dangerous | **Single study** | 92.7% success at 75% retention against a 93.8% full-context baseline. The paper was two days old when the guide cited it |
| Compression and caching work against each other | **Contested, and refined** | The guide's chapter 8 argues compressed context is derived, so it cannot live in a stable cached prefix, making the real comparison "compression cost against caching the uncompressed corpus". A July 2026 study measured the trade-off directly and complicates it; see below |

**The refinement.** A July 2026 paper measured cache hit rates on
a production API rather than assuming them
([Cache-aware prompt compression, arXiv:2607.15516](https://arxiv.org/abs/2607.15516)).
Two findings. The cache is not the
ideal the compression literature assumes: it found a two-tier architecture with
a sharp threshold near 3,500 tokens, below which the hit rate plateaued at
about **0.83** across 30-call sessions. And under that realistic hit rate,
**query-aware compression beats naive caching at compression ratios of about 6×
or higher**.

The guide's rule holds at modest compression ratios and inverts at high ones.
Measure the actual cache hit rate before reasoning about it, because the
literature on both sides assumes a number nobody checked. **Single study, three weeks old.**

---

## 8. Caching and economics

Most of this section is vendor documentation, which is the right source
because the vendor sets the product and the billing.

| Claim | Grade | Evidence |
|---|---|---|
| Cache read at 0.1×, write at 1.25× or 2×; break-even at one or two reads | **Vendor documentation** | Authoritative. Verify against current pricing before quoting, since it changes |
| Minimum cacheable prefix is model-dependent and not monotonic across generations | **Vendor documentation** | 512 to 4,096 tokens depending on model. Below the minimum, caching silently does not happen and no error is returned |
| Caching is a byte-exact prefix match; one changed byte invalidates everything after it | **Vendor documentation** | The single most consequential mechanic in the guide |
| Cached tokens still occupy the window | **Vendor documentation** | Caching changes what you pay, not how much room it takes |
| Real cache hit rates are well below 1.0 | **Single study** | ~0.83 below a ~3,500-token threshold on one model. No other measurement of this was found. It undermines cost models on both sides of the compression debate |
| KV-cache compression methods work, but their headline multipliers do not transfer | **Replicated mechanisms, unreplicated numbers** | Attention sinks, heavy-hitter eviction and KV quantisation are all standard practice. The 22× and 29× speedups were measured against 2023-era baselines on 2023-era models |
| Aggressive KV eviction can preserve accuracy while degrading faithfulness | **Single study**, Aug 2026 | Worth watching: accuracy is not a sufficient check on cache compression |

---

## 9. Agent memory

The weakest evidence in this document. Every number comes from a party selling
a memory product.

| Claim | Grade | Evidence |
|---|---|---|
| On the standard memory benchmark, a full-context baseline beats leading memory systems | **Replicated** | Present in one vendor's own results table (~73% baseline against ~68%), and independently: a plain filesystem scored 74.0% against a graph memory system's 68.5%. Two independent parties, same conclusion |
| Vendor memory benchmark numbers are irreconcilable | **Established by the dispute itself** | 84%, 75.14% and 58.44% for the same system on the same benchmark, depending on who configured the harness. That spread exceeds any effect either party claims |
| Harness configuration dominates system quality on that benchmark | **Single study** | Swapping only the embedding model in an otherwise identical pipeline moved accuracy 6.2 points and flipped the conclusion about which approach was better |
| Multi-agent beats single-agent by 90.2% | **Vendor**, internal eval | Composition unpublished. Same source reports multi-agent uses ~15× the tokens and that token usage alone explains 80% of performance variance — which substantially weakens the claim that the gain is architectural |
| Single agents match or beat multi-agent under matched token budgets | **Single study** | Argues from the data processing inequality that passing information through more agents can only lose it. Read against the row above, it suggests much of the multi-agent win is spend |
| Multi-agent failures are mostly design failures | **Single study** | 1,600+ annotated traces, 14 failure modes; ~42% specification and system design |
| Context editing plus a memory tool is worth 29–39% | **Vendor**, internal eval | No published composition, no external replication |
| Deferring tool definitions cuts ~85% of tool context and raises selection accuracy | **Vendor**, but mechanically verifiable | You can measure the token reduction yourself with a counting endpoint. The accuracy claim is vendor-internal |

**The structural problem.** Nobody runs an independent leaderboard for agent
memory. Every number comes from a party shipping a memory product, evaluated on
a benchmark whose conversations fit inside a modern context window. Both major
vendors report figures in the low 90s on it, self-published. Treat it as
retired and benchmark against
full-context on your own data.

---

## 10. Evaluation methodology

The best-established section here. Most of it is standard statistics rather
than new results.

| Claim | Grade | Evidence |
|---|---|---|
| Detecting a 3-point difference needs ~1,000 items | **Derivation, not an empirical claim** | Standard power analysis. It follows from the maths given the variance, so it does not need replication; it needs your variance |
| Detecting a 10-point difference in agent pass rates needs 120–200 tasks | **Single study**, Monte Carlo | Measured on a real agent suite. Task-level variance dominates run-level variance, so repeats barely help |
| Repeats saturate; the variance-reduction ceiling is two thirds | **Derivation** | K=2 cuts total variance by a third, K=4 by a half |
| Lowering temperature to reduce eval variance is counterproductive | **Single study**, worked example | Shifts variance into conditional means, which resampling cannot reduce, and can add bias |
| Items generated from the same source document are not independent | **Single study**, real benchmarks | Naive standard errors up to 3× too small. Directly relevant to context-window suites, which typically mint several questions per document |
| Temperature 0 is not deterministic | **Replicated** | Four independent groups: batch-invariance (80 unique outputs from 1,000 identical calls), hardware and batching (up to 9%), inference backend (beyond typical noise margins), and cross-run variation on open models |
| Prompt format alone swings results by more than most treatment effects | **Single study** | 50.1% against 72.4% on the same benchmark, with model rankings reordering |
| A carefully controlled context ablation found no effect | **Single study** | 291 runs, permutation tests, equivalence testing, power simulation, and a manipulation-validity probe confirming the context never flipped a near-miss. Also found task difficulty does not transfer between agents |
| Automatic attribution metrics correlate 0.96 at system level and much worse per example | **Single study**, authors' own caveat | Validated for ranking systems, not adjudicating answers. An A/B is a system-level comparison, so the caveat permits that use |
| A small entailment checker matches a frontier judge at ~400× lower cost | **Single study** | $0.24 against $107 for 13,000 pairs. Means you can score every response rather than sampling |
| Faithfulness scoring alone rewards evasion | **Observation from leaderboard data** | Two models post low hallucination rates with answer rates of 80.7% and 62.7% |
| Citation links resolve far better than they support | **Single study**, 14 models | Links resolve above 94%, relevance above 80%, actual support 39–77%. Support accuracy fell ~42% as tool calls scaled from 2 to 150 |
| Factuality metrics are biased against paraphrase and against long-range synthesis | **Single study**, five metrics, 11 datasets | The bias that most threatens this field's own advice: a context change enabling synthesis will score worse for reasons unrelated to truth |
| Prose-trained grounding detectors do not transfer to tool output | **Single study** | 0.17 against 0.689 span-F1 on code-agent sources |
| Repeated significance testing on accumulating data inflates false positives | **Replicated** — classical statistics | 70% false-positive rate on A/A data. This is textbook, not a new finding |

---

## 11. What changed in the last year

Six shifts a reader returning after twelve months should know about.

**Position bias went from settled to contested.** The U-curve was the field's
most-cited practical finding. A 2026 reproducibility study did not find it, and
diagnosed the disagreement as a sample-size artefact. Meanwhile the effect
turned out to be large in modalities nobody had checked — strong primacy in
multimodal retrieval, recency inversion for tool results.

**Selection under a budget acquired theory.** Context packing went from
folklore to a knapsack problem with a provable approximation guarantee, in
three independent papers within three months. The associated finding — that
recall and end-to-end accuracy decouple — invalidates a great deal of existing
retrieval tuning.

**Tool definitions became the recognised fixed cost.** Deferred loading, tool
search and code-execution patterns all arrived to address the same problem, with
reported reductions of 85–98%. This was barely discussed a year ago.

**Evaluation matured faster than the rest of the field.** Error bars, clustered
standard errors, power analysis, judge bias correction and equivalence testing
are now available in tooling rather than papers. One framework ships them
directly. A carefully controlled context ablation published this year returned
a well-powered null result.

**Agent memory grew and did not consolidate.** The number of systems,
benchmarks and claims grew, and there is no independent leaderboard. The founding project of the
category is now labelled legacy by its own maintainers while its organisation
ships a coding agent instead.

**Caching stopped being a billing detail.** Cache-aware ordering, cache-aware
compression and a measurement of production hit rates all arrived in 2026. The interaction between caching and every other optimisation in the
guide is now the live question.

---

## 12. Open problems

Ordered by how much a good answer would be worth.

**Does sophisticated acquisition have a scale ceiling?** One paper puts the
crossover where plain keyword search overtakes agentic search at ~10M corpus
tokens, with a ~20-point margin above it. One team, one construction. If it
replicates, it bounds an entire product category.

**How much of the multi-agent advantage is architecture and how much is
spend?** The vendor reporting the 90.2% advantage also reports that token usage
explains 80% of performance variance. The decisive experiment would be matched
token budgets across several task families, evaluated independently.

**Can position effects be reconciled?** Flat for text at moderate k, strong
primacy for multimodal, recency inversion for tool results, and a theoretical
account predicting a U-curve at initialisation. No framework covers all four.

**What is the real cache hit rate in production?** One measurement exists,
finding 0.83 rather than the 1.0 both the caching and compression literatures
assume. Cost models on both sides depend on this number.

**Does the tool-confusion residue generalise?** If ~10 points of degradation
survives a perfect shortlister on catalogues generally, there is a hard ceiling
on retrieval-based mitigation and the remedy lies with whoever writes tool
descriptions.

**Does compaction fidelity have a pattern?** One measured instance: gist
preserved, specifics lost. If that shape is general, compaction is unsafe for
any task whose answer is a detail, and that is knowable.

**What does a good source look like?** No benchmark scores whether a system
chose an authoritative source over a content farm. For open-web acquisition
this is the central quality question and it has no measurement.

---

## 13. Where to start reading

If you read nothing else, read these, in this order.

**To understand the problem.** Liu et al., *Lost in the Middle* (TACL 2024) is
the clearest statement of the question; it is partly contested, which makes it
more instructive. Then Chroma's *Context Rot* (Jul 2025) for the plainest
demonstration that length alone degrades performance.

**To calibrate your expectations.** HELMET (ICLR 2025) on why benchmarks
disagree, and RULER (2024) on effective length. Between them they will stop you
trusting a single number.

**To design an experiment.** Miller, *Adding Error Bars to Evals*
(arXiv:2411.00640) — short, practical, and the source of most of chapter 11.
Then Khatri's context-file ablation (arXiv:2607.27250) as a worked example of a
null result done properly.

**To build something.** Anthropic's context-engineering and tool-use
engineering posts for the practitioner framing, read alongside Cognition's two
multi-agent posts for the opposing view. All four are vendor sources and should
be read as such.

**To see a whole system.** Cerebras, *How We Built Our Knowledge Base* (Jul
2026) describes a working internal retrieval system end to end — sources,
distillation, one embeddings table, parallel retrievers, rank fusion,
reranking, context expansion. Read it for the assembly and the ordering, not
as an authority: it is a chip company's internal tool, its individual
techniques are standard and better sourced elsewhere, and it reports no
retrieval quality numbers. Useful mainly as an illustration of how rarely
production systems in this field are measured at all.

**Surveys, if you want breadth.** *Beyond the Parameters* (arXiv:2604.03174,
Apr 2026) covers in-context learning through retrieval to GraphRAG and includes
a claim-audit framework, which few surveys do. *Memory for Autonomous LLM
Agents* (arXiv:2603.07670, Mar 2026) covers 2022 to early 2026 with a
write–manage–read taxonomy.

**Full citations** for everything referenced here are in the companion guide's
sources section. This document deliberately does not repeat them.

---

*Grades reflect what could be established on 4 August 2026. Recheck any grade
you intend to rely on.*
