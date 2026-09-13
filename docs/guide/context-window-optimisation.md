---
title: "Context window optimisation: a working guide"
---

# Context window optimisation: a working guide

*What should go into a model's context window, and how do you know it helped.*

This guide is for people who build with agents and want to make informed
decisions about context. It assumes a working knowledge of prompts and the
outline of retrieval-augmented generation, but no familiarity with the
research literature. Figures carry the date they were checked because prices,
model limits and tooling change.

Specialist terms are defined on first use or in the [glossary](#glossary).
The companion [`state-of-the-evidence.md`](state-of-the-evidence.md) grades the evidence behind each claim.

## Contents

1. [What is actually in the window](#1-what-is-actually-in-the-window)
2. [Why more context is not better](#2-why-more-context-is-not-better)
3. [What the benchmarks do and do not tell you](#3-benchmarks)
4. [Acquisition: getting the right material](#4-acquisition)
5. [Extraction and chunking](#5-extraction-and-chunking)
6. [Ranking and reranking](#6-ranking-and-reranking)
7. [Selection and ordering under a budget](#7-selection-and-ordering)
8. [Compression](#8-compression)
9. [Caching, and what it costs to get the order wrong](#9-caching)
10. [Agent memory, compaction and isolation](#10-agent-memory)
11. [Proving a change helped](#11-proving-a-change-helped)
12. [What nobody knows yet](#12-what-nobody-knows-yet)
13. [Glossary](#glossary)
14. [About this guide](#about-this-guide)
15. [Sources](#sources)

---

## 1. What is actually in the window

A [context window](#g-context-window) is the bounded working material a model
can hold at once, usually measured in tokens. Text, encoded images and other
supported inputs can all count towards it.

Everything counts: the system prompt, every message in the conversation,
every tool result, every image, every attached document, and the definitions
of every tool the model can call. The model's own output counts too,
including any reasoning it does before answering.[^1] Input and output share
the overall context limit, although a model may also impose a lower maximum on
its output.

A "1M token window" therefore holds a million tokens of all of the above.
The space left for your own content is that limit minus what the other items
take, and several of them are easy to overlook.

### Why there is a limit

In naive dense [attention](#g-attention), the amount of comparison work grows
quadratically with input length: double the input and the work is roughly four
times as large.[^2] This scaling is one reason context windows have limits.

Vendors do not publish how they actually serve a million-token window, and
nobody uses the naive method any more. Quadratic cost explains
why the problem exists. It does not tell you what a specific endpoint costs
you today.

### Tokens are not a unit you can convert

A [token](#g-token) is a chunk of text — a short word, part of a long one, a
piece of punctuation. It is roughly four characters of English, but the
approximation is loose.

The same sentence is a different number of tokens on different models. It is
a different number on different generations of the *same* model: Claude 4.7
and later use a newer [tokeniser](#g-tokeniser) that produces about 30% more
tokens for identical text.[^3] Per-token prices did not change, so the same
document became about 30% more expensive to send.

You also cannot count another vendor's tokens. `tiktoken` is OpenAI's
tokeniser, and Anthropic's own guidance says using it on Claude undercounts
by 15–20% on ordinary text and worse on code.[^4] Anthropic and Google
publish no downloadable tokeniser at all. The only accurate count comes from
each vendor's own counting endpoint, which is free.[^3]

### How much of the window does real content take?

Token density varies a lot by content type, and the figures that circulate are
not sourced. Here is a measurement you can rerun: every file is in this
repository and the tokeniser is named.

| Content | Tokens per 1,000 characters | Characters per token |
|---|---|---|
| English prose (`PRODUCT.md`) | 191 | 5.22 |
| English prose, longer (`ROADMAP.md`) | 224 | 4.47 |
| Walkthrough, prose plus shell | 231 | 4.32 |
| JSON Schema | 235 | 4.26 |
| Rust source | 246 | 4.07 |
| NDJSON event log | 249 | 4.02 |
| JSON API record | 252 | 3.97 |
| Rust tests | 271 | 3.68 |
| Lockfile (`Cargo.lock`) | 372 | 2.69 |

Measured with tiktoken `o200k_base` on 4 August 2026 by
`docs/guide/measurements/token_density.py`. Absolute counts do not transfer to
Claude or Gemini, which publish no downloadable tokeniser; the ratios between
rows largely do.

These files do not support the claim that "code is about twice as dense as
prose": code cost about 20–40% more per
character than prose, not double. That is nine files in one repository and one
tokeniser, so treat it as a reason to measure your own corpus rather than as a
refutation.

The densest thing in the table is a lockfile: repetitive, highly structured,
machine-generated text. Anything
that looks like a table of near-identical rows tokenises badly. Logs,
lockfiles, dumps of similar JSON records and long lists of tool definitions
are the expensive shapes, and they are also the shapes most often pasted
into agent context.

### A large avoidable cost: raw HTML

Four real pages were fetched and measured twice, as retrieved and after
stripping tags and scripts with a crude regular expression.

| Page | Raw HTML | Tags and scripts removed | Saved |
|---|---|---|---|
| Wikipedia article | 145,535 | 18,760 | 87% |
| arXiv abstract | 12,321 | 1,210 | 90% |
| vLLM docs page | 205,908 | 10,867 | 95% |
| BBC News index | 363,439 | 3,945 | 99% |

**Removing tags, scripts, styles and similar HTML machinery removed between
87% and 99% of the tokens, median 92% on these four pages.** This crude strip
left navigation, footers and related-article rails in place. A real
[extractor](#g-extraction) tries to remove those too.

Read the range rather than an average. The BBC entry is a news *index* — a
navigation page, not a document — and it is both the 99% outlier and half the
raw tokens in the sample, so a token-weighted aggregate across all four (95%)
mostly measures that one page. Excluding it gives 92%.

The figure usually quoted is "about 80% boilerplate", which comes from blog
posts rather than a measurement and refers to text rather than tokens. Four
pages cannot test that claim because this script did not identify boilerplate.
They show instead that HTML machinery alone can dominate the token count, and
that its share varies enough by page type that you should measure your own
corpus rather than take any single number, including this one.

For raw-web ingestion, removing HTML machinery is a large, inexpensive saving,
and it happens before any retrieval decision.

### What is in there before you add anything

A production agent carries a large fixed token overhead from its tooling.
Anthropic published numbers: the GitHub connector's 35 tools cost about
26,000 tokens, Slack's 11 tools about 21,000, and a five-connector setup
around 55,000 tokens before the conversation starts. Internally they have seen 134,000 tokens of tool
definitions.[^6]

Most of that is [JSON Schema](#g-json-schema), not prose. An independent
measurement of one badly-shaped server found **97% of its tokens were in the
input schema**, and that redesigning it cut 17,161 tokens to 773.[^7]

The order is fixed: tool definitions, then the system prompt, then the
messages. Within the messages, tool results are usually the largest item in
any long-running agent — larger than the definitions that produced them. One
published breakdown of a real session put fixed overhead at about 28,000
tokens and conversation plus tool results at 185,000, so roughly 87% of the
combined 213,000 tokens were conversation and tool results.[^8]

### Available, loaded, invoked

These are three different costs, and conflating them is the most common
accounting error in agent design.

- **Available** — a tool exists and could be used. It may still need a small
  discovery entry, but its full definition is absent.
- **Loaded** — its description sits in the window. Costs its full size on
  every request, whether or not it is ever called.
- **Invoked** — it was actually called. Costs its arguments plus its result,
  on top of the above.

The API implements the distinction. Deferred tool
loading sends every definition to the service but keeps unused ones out of
the model's window; Anthropic reports this taking a 77,000-token tool set
down to 8,700, an 85% cut.[^6] Skills work the same way: a skill costs about
100 tokens while available (its name and one-line description) and loads its
full instructions only when triggered.[^9] An independent count
across Anthropic's 17 official skills put discovery at a median of about 80
tokens each, roughly 1,700 tokens for all seventeen.[^10]

An inventory that does not separate these three states can make a fleet of 200
tools look cheap because they are rarely called. Without deferred loading, the
definitions occupy the window on every request.

### What you cannot see

A hosted API exposes less instrumentation than its list of returned metrics
suggests.

You get total input tokens, cache counters, output tokens, and on Anthropic a
count of reasoning tokens. You do **not** get a breakdown of which input
tokens were system prompt, tool definitions, history or retrieved evidence.

The available metrics leave several blind spots:

- **Counting and billing disagree.** Anthropic states that token counts "may
  include tokens added automatically for system optimizations" and that you
  are not billed for those.[^3] Never expect a counting dashboard to
  reconcile against an invoice.
- **Reasoning content is not returned** on current frontier models. You can
  see how many tokens it took. You cannot see what it said.
- **Cache misses are undiagnosed by default.** A prefix that fails to match
  produces no error; it shows up only as a zero.
- **Nothing tells you which tokens the model actually used.** Context rot is
  documented as a phenomenon and is unobservable per request.

That last one is the subject of the next chapter.

---

## 2. Why more context is not better

Context windows grew faster than models' ability to use them.

This chapter is the evidence for that claim, and a warning about how it is
usually cited. A great deal of confident writing on this subject quotes 2023
numbers about 4,000-token models as though they were properties of 2026
million-token ones.

### Lost in the middle: what it actually said

The founding result is Liu et al., published as a preprint in July 2023 and
in TACL in 2024.[^11] They varied only *where* the answer-bearing document
sat among 10, 20 or 30 retrieved documents, holding the content constant.

GPT-3.5-Turbo scored about 75.5% when the gold document was first, fell to
about 53% when it sat around position 10, and recovered to about 63% at
position 20. A [U-shaped curve](#g-u-shaped-curve): good at the edges, poor
in the middle.

The same model scored **56.1%
with no documents at all**. In the 20- and 30-document settings with the
answer in the middle, it did *worse than that*. The retrieved context lowered
accuracy below the closed-book score. The oracle condition (the gold document alone) scored 88.3%.

A second finding from the same paper is cited less often:
extended-context variants were not better at using context than their
short-context siblings. Claude-1.3 and Claude-1.3-100K scored 48.3% and 48.2%
closed-book, 76.1% and 76.4% oracle. The larger window did not improve the
model's use of it.

**How to cite this, because it is contested.** These are
GPT-3.5-Turbo and Claude-1.3 numbers on 2,000–6,000-token inputs. If you see
"a 20-point drop" attributed to a current frontier model, it is a 2023 result
presented as a 2026 one.

**The U-curve may also not survive replication.** A SIGIR 2026
reproducibility study tested five current models on two datasets under a
controlled protocol and found performance "nearly flat across
placements".[^109] Their diagnosis is that topic sampling dominates the
variance, and that stable conclusions need 1,000–2,000 topics — far more than
most published ordering experiments use.

Set against that, a May 2026 audit of nine models did find end-to-middle drops
on *reasoning* tasks, growing with context length, while noting that newer
releases show smaller drops.[^12] The two may be compatible — different task
types, different protocols — but nobody has reconciled them.

As of August 2026: **position sensitivity is real for tool
output and for multimodal input, and is doubtful for ordinary text retrieval
at moderate k.** Chapter 7 works through what that means for how you order
evidence.

### Context rot: length alone degrades performance

Chroma's July 2025 report tested 18 models on tasks with no reasoning content
whatsoever.[^13] Their sharpest result: accuracy on *replicating a list of
repeated words* falls from near-100% at 25 words to roughly 40–60% at 10,000
words. The task does not get harder, only longer.

The same work also found:

- Low semantic similarity between the question and the target degrades far
  faster with length than high similarity. Standard needle tests, where the
  question shares words with the answer, are the easy case.
- One distractor reduces accuracy, four reduce it further, and the harm is
  uneven across distractors rather than proportional to their number.
- On a memory benchmark, a focused ~300-token prompt beat the same content
  embedded in ~113,000 tokens by roughly 15–40 points depending on the model
  family.

**Shuffling the filler text to destroy its
coherence consistently improved retrieval** across all 18 models. Coherent
surrounding prose competes for attention. Well-written context is not
automatically safer context.

A separate 2024 study isolates length even more cleanly. Levy et al. hold a
two-fact reasoning problem constant and pad it with irrelevant text: average
accuracy falls from 0.92 to 0.68, with degradation beginning around 3,000
tokens — orders of magnitude below the models' advertised limits.[^14]

### Effective length versus advertised length

"Effective context length" is the length at which a model still does the job,
as opposed to the length it will accept without erroring.

- **RULER** (NVIDIA, April 2024): despite near-perfect scores on simple
  needle tests, "only half" of models claiming 32K or more maintained
  satisfactory performance at 32K.[^15]
- **NoLiMa** (Adobe, February 2025) removes the lexical shortcut, so the
  model must associate rather than keyword-match. Of 12 models claiming 128K
  or more, **10 fell below half their own short-context baseline at
  32K**.[^16]
- **Why**: long relative distances are rare in training data even when the
  training window is long, so the far end of the window is undertrained. One
  paper puts effective length near *half* the training length and recovers
  more than 10 points on RULER by remapping position offsets.[^17]

The statistic "effective context is 60–70% of advertised" circulates widely
with no traceable source. The figures above are narrower,
lower and attributable.

### The counterweight

Epoch AI's tracking of 123
models found that while advertised windows grew about 30× annually, the input
length at which the best models still hit 80% accuracy improved **over 250×
in nine months**.[^18] The failure modes are real. They are also receding
faster than the windows are growing.

Databricks' 2,000-experiment study points the same way: Llama 3.1 405B declined
after 32K and GPT-4-0125 after 64K, but o1-mini, GPT-4o, Claude 3.5 Sonnet
and Claude 3 Opus improved roughly monotonically to 100K.[^19] Failure modes
were model-specific too — Claude 3.5's copyright refusals rose from 3.7% to
49.5% between 16K and 64K, while DBRX's instruction-following failures rose
from 5.2% to 50.4%. "Long context degrades" is too coarse. Ask which model,
which task, and which failure.

### Too many tools

**LongFuncEval** (IBM, April 2025) isolated three stressors and measured each:
growing tool definitions from 8K to 120K tokens degraded Mistral-large by
94%, Llama-3.1-70B by 72% and GPT-4o by 13.8%; longer tool responses degraded
Mistral-large by 91% and GPT-4o by 7%; longer conversations cost 13–40%.[^20]

The best-instrumented result is from a real production catalogue of 110
agents and 584 tools, evaluated on GPT-5.4, GPT-5.1 and Claude Sonnet
4.5.[^21] Routing F1 on implicit queries fell from 58.2% to 42.1% as the
catalogue grew from 51 to 584 tools. The degradation was driven by recall
(55%→37%) more than precision (68%→60%).

An **oracle shortlister** — one that always includes the
correct tool — still dropped from 79.0% to 68.8%. About **10 points of the
gap cannot be closed by better retrieval**. That residue needs deduplicated
descriptions and disambiguated names, not a better index. Embedding-based
shortlisting to about 20 candidates recovered 10–11 points. The elbow sat
around 40–60 agents, and the authors warn the specific numbers are
catalogue-specific even if the shape transfers.

Instructions have a ceiling too. One 2026 preprint crossing format,
instruction count and context length found the perfect-response rate
collapses to zero by 80 simultaneous instructions for every model, format and
placement tested.[^22] The failure mode near capacity was *refusal*,
not fabrication: 0 of 5,760 probes produced an invented answer.

### Conversations decay

Laban et al. sharded fully-specified instructions across conversational
turns, across 200,000+ simulated conversations and 15 models.[^23] Every
model dropped, by **39% on average**; o3 fell from 98.1 to 64.1.

Their decomposition is the useful part. Aptitude fell about 16%.
Unreliability rose about **112%**. The models did not get less capable; they
got less consistent. And concatenating the shards back into a single turn
largely restored performance, so the cause is the turn structure, not the
information. Their summary: "when LLMs take a wrong turn in a conversation,
they get lost and do not recover."

### Pruning can improve accuracy

Cutting context can improve accuracy. Microsoft's 50-task expense
benchmark on a production agent:[^24]

| Configuration | Accuracy | Tokens |
|---|---|---|
| Full context | 71.0% | 1,480,996 |
| Keep last 5 tool call/response pairs | 79.0% | 535,274 |
| Pruning plus windowed summarisation | **91.6%** | 553,374 |

**Cutting context by 63% raised accuracy by 20.6 points.** Reproduced by the
same authors across three task categories and on Claude Sonnet 4.5 — not
independently.

Input tokens were 99.75–99.87% of total usage, so optimising output length is optimising
the wrong thing. And individual tool responses ran 500–3,000 tokens while
accumulated histories reached 50,000–150,000+.

LongFuncEval found that for *tool results*, accuracy
at position 8 exceeded position 1 by 5–75%. For tool output it is the oldest
results being ignored, not the middle ones. Lost-in-the-middle intuitions do
not transfer wholesale.

### The four-failure taxonomy, and how much to trust it

Drew Breunig's June 2025 framing — context **poisoning**, **distraction**,
**confusion** and **clash** — is the most-used vocabulary in this area.[^25]
It is a practitioner blog post, correctly cited as the origin of the terms.
Its evidential backing is uneven:

- **Distraction** and **clash** have real support (the multi-turn and
  Databricks results above).
- **Confusion** has strong support that arrived *after* the post, in the tool
  and instruction work above.
- **Poisoning** has essentially only an anecdote — a Gemini agent playing
  Pokémon, which does not appear in any current version of the Gemini 2.5
  technical report. As of August 2026 I could find no controlled experiment
  that injects a false fact into an agent's context and measures how far it
  propagates. Use the term; attribute it to Breunig, not to Google; and do
  not present it as measured.

### Dead claims

**"Adding random documents improves RAG accuracy."** The original 2024 result
reported up to 35% improvement from irrelevant documents.[^26] A SIGIR 2026
reproducibility paper reproduced it under the original setup and then showed
it vanishes under modern prompting — a 15-token generation limit, no chat
template and a forced no-answer instruction were penalising the low-document
baselines.[^27] Under normal settings the effect is between −0.23% and
−1.30%. Truncated and malformed generations accounted for 53.6–73.6% of
errors in the original configuration.

The opposite finding survives: *semantically similar but wrong* passages are
the expensive kind of noise, and they are common in top-10 dense retrieval
results.[^28]

**"Effective context is 60–70% of advertised."** No source. See above.

---

## 3. Benchmarks

The available measurements are less reliable than their scores suggest. This
chapter is about which ones to trust, and for what.

### The one finding that matters most

**The benchmarks disagree with each other.** HELMET, a meta-benchmark built
to test this question across 59 models, found that no synthetic long-context
task reaches an average correlation above 0.8 with real downstream tasks, and
that RULER's correlations are all below 0.85.[^29] Between HELMET's own seven
categories, correlations are low.

A model ranking from one long-context benchmark
does not transfer to another. If you pick a model because it topped a
leaderboard, you have chosen for that leaderboard's task mix, not for yours.

### Needle-in-a-haystack is a floor test

[NIAH](#g-niah) hides one sentence in filler text and asks the model to find
it.[^30] Every frontier model passes it at every depth to a million
tokens, so it does not separate models.

It is also largely a string-matching test. Remove the word overlap between
question and answer — which is what NoLiMa does — and 11 of 13 models fall
below half their own short-context score by 32K.[^16]

OpenAI's GPT-4.1 material reports perfect
needle retrieval to 1M tokens, and **19.0% on multi-hop graph traversal above
128K** on the same model.[^31] Both numbers are true. Only one of them
resembles your workload.

If your evaluation is a needle test, you are measuring whether the model can
find a sentence, not whether it can use a document.

### What each benchmark is actually for

| Benchmark | Use it to answer | Status |
|---|---|---|
| **RULER** | Where does this model stop working? | Active; the standard effective-length probe |
| **NoLiMa** | Can it retrieve without keyword overlap? | Active, still discriminative |
| **HELMET** | Which benchmark should I even trust? | Active; the reference meta-benchmark |
| **LongBench v2** | Hard reasoning over long documents | Active, unsaturated |
| **LongProc** | Can it produce long structured *output*? | Active |
| **LoCoDiff** | Can it track evolving state across a history? | Active; hard |
| **τ-bench / τ²-bench** | Is the agent *repeatably* right? | Active; the reliability probe |
| **Terminal-Bench** | What does the accuracy cost in dollars? | Active; the only one reporting cost |
| **BEIR / MTEB** | Which retriever or embedder? | Active, heavily optimised against |
| **RAGTruth** | Is the output grounded in what was retrieved? | Active |
| **NIAH** | Is the model catastrophically broken? | Saturated |
| **LongBench v1** | — | Saturated, and short by 2026 standards |
| **LoCoMo** | — | Saturated and unreliable; see chapter 10 |

Two of these deserve expanding.

**LoCoDiff** gives a model a git history and asks it to reproduce the exact
final file.[^32] There is no filler and no planted needle; every token has to
be reproduced.
Best score is 79%, and **all models fall below 50% above 25,000 tokens**, on
a benchmark whose largest prompt is 97,500. Set that against the same models'
near-perfect needle scores at a million tokens.

**τ-bench** is the only widely-used benchmark reporting `pass^k` — success on
*all* k independent attempts rather than the best one.[^33] GPT-4o succeeded
on under 50% of retail tasks at one attempt and **under 25% at eight**. For
anything user-facing, that is the number that matters, and almost nothing
else reports it.

### Length alone degrades performance, even with perfect retrieval

This is the strongest evidence against "just use a bigger window". Du et
al. isolated length by replacing irrelevant tokens with whitespace, masking
them with forced attention, and placing all the evidence immediately before
the question — every trick to remove distraction and position effects.
Performance still fell **13.9% to 85%** across five models.[^34]

Even a perfect retriever does not avoid this: length itself is a cost.

### Contamination cannot be audited away

Models identify buggy file paths from SWE-bench issue text alone at up to 76%
inside the benchmark and 53% outside it, and reproduce functions at 35%
versus 18%.[^35] That is measured memorisation.

Detection of contamination also fails. A 2026 study tested three contamination-detection
methods across 25 models and found **only 201 of 335 evaluations produced
correct outcomes**.[^36] Detection fails under distribution shift and lacks
statistical power, because benchmarks are orders of magnitude smaller than
training corpora. Their conclusion: statistical auditing "cannot replace
transparent data provenance".

The 2026 response is self-refreshing benchmarks built from post-cutoff
material — EvoBrowseComp regenerates 800 live-web questions through an
automated pipeline specifically because static sets leak.[^37] Expect this to
become the dominant design.

### Vendors report on evals they own

OpenAI's MRCR and Graphwalks are OpenAI-published datasets, so nobody outside
can build a held-out variant. Both have been corrected without announcement — MRCR in
December 2025, Graphwalks in February 2026 — which means figures published
before those dates are not comparable with figures after.

Google's Gemini 3 launch material reported LMArena, GPQA Diamond, MathArena
Apex and MMMU-Pro, and **no long-context benchmark**, despite the
million-token window being a headline feature.[^38]

### Only one leaderboard publishes cost

Terminal-Bench 2.1, fetched 4 August 2026:[^39]

| System | Accuracy | Cost |
|---|---|---|
| Claude Code / Fable 5 | 83.8% ± 1.2% | $552.67 |
| Codex / GPT-5.5 | 83.1% ± 1.1% | $2,059.19 |

Statistically indistinguishable accuracy at 3.7× the cost. Every other
benchmark in this chapter would rank those two identically.

### What nothing measures

If you build a retrieval or acquisition system, you have to measure these
yourself:

- **Cost per correct answer.** Terminal-Bench aside, every leaderboard
  reports accuracy at unbounded token spend.
- **Latency.** No long-context benchmark reports wall-clock time.
- **Whether the right *source* was chosen** from a live, adversarial, open
  web. BrowseComp scores a final short answer, not source selection.
  Nothing scores authoritative-over-content-farm, or primary-over-aggregator.
- **Provenance and attribution fidelity.** HELMET's citation category is the
  only serious attempt, and HELMET singles it out as correlating *worst* with
  everything else.
- **Multi-source synthesis and conflict resolution.** RECON is the first
  benchmark to take source conflict seriously, and the best non-oracle system
  scores 22.4%.[^40]
- **Freshness and staleness detection.** Nothing measures whether a system
  knows its evidence has gone stale.
- **Abstention.** Benchmarks reward answering. A system that says "I could
  not find a reliable source" scores the same as one that confabulates.
- **Robustness to adversarial retrieved content.** If you retrieve from the
  open web, that text is untrusted input, and no mainstream retrieval
  benchmark treats it that way.

---

## 4. Acquisition

If the right material never enters the pipeline, no amount of ranking,
compression or ordering will recover it.

### Long context versus retrieval: what the comparison actually shows

One comparison across nine datasets found that, given enough budget, long
context beat chunked retrieval on answer quality — by 7.6 points on
Gemini-1.5-Pro, 13.1 on GPT-4o, 3.6 on GPT-3.5-Turbo across nine
datasets.[^42]

But the same paper found **about 63% of queries produced identical answers
under both**. The two approaches agreed far more than they differed in that
study.

The useful question is which to use per query.
**Self-Route** asks the model whether the retrieved chunks suffice and
escalates to full context only when they do not: 38.6% of the tokens on
Gemini-1.5-Pro (a 65% cost cut) and 61% on GPT-4o (39% cut), at near
long-context quality.[^42] Over half of queries were answerable from
retrieval alone.

The crossover point is roughly 128K tokens and it moves with model size. At
32K long context leads by about 2.4%; at 128K retrieval leads open-source
models by about 3.7%, while GPT-4o and Claude 3.5 Sonnet still favour long
context.[^43] Weak models (3B–12B) gained 6.5%–38.1% from retrieval at 128K.
By task, long context leads reasoning by ~9% and comparison by 14–15%, while
**retrieval leads hallucination detection by 10–22%** — because withholding
distractor mass is the point.

### Retrieval fails in four ways, and three are query problems

From manual error analysis:[^42] multi-step reasoning (the retriever cannot
chain), vague queries (nothing to match on), long complex queries (the
retriever cannot parse them), and implicit questions (the answer is not
lexically or semantically present).

Only the last is an index problem. This taxonomy, from one paper's manual
error analysis, is the most useful diagnostic found for this guide, because it tells
you which fix to apply instead of applying all of them.

### How many sources is enough

Reader accuracy saturates long before retriever [recall](#g-recall) does.
Going from 20 to 50 retrieved documents gained about **+1.5% for
GPT-3.5-Turbo and +1% for Claude-1.3** while recall kept climbing.[^11] Past
that point, the marginal end-to-end benefit was small despite the extra
retrieved evidence.

Both the number and the similarity of distractors matter.
Semantically-close-but-wrong passages can do disproportionate harm, and over 60% of
queries in one evaluation had at least one hard distractor in the top ten
from a dense retriever.[^28]

Anthropic's own sweep found k=20 best for their contextual-retrieval setup;
other practitioner guidance converges on 5–10 after reranking.[^44] Both are
right for their own pipelines. Tune k against your end
metric, not in the abstract.

### Techniques, and whether they pay for themselves

| Technique | Reported benefit | Verdict |
|---|---|---|
| Hybrid dense + BM25 with rank fusion | Consistent recall lift over either alone, negligible added latency | **Worth it.** Near-zero cost |
| Cross-encoder reranking | Part of the 5.7%→1.9% failure chain | **Worth it.** See chapter 6 |
| Contextual chunk headers | −35% retrieval failure alone, −67% with reranking (vendor's own eval) | **Worth it** for corpus search; see chapter 5 |
| Query rewriting | Addresses the vague-query failure directly | **Worth it**, routed to that failure |
| Query decomposition | Addresses multi-step failure | **Situational.** Route to it |
| Multi-query / RAG-Fusion | Raises recall, then Hit@10 *fell* 0.51→0.48 | **Not worth it** if you already rerank and truncate |
| HyDE[^119] | Beat unsupervised retrievers in 2022 | **Situational.** Its premise mostly does not hold for current retrievers |
| Step-back prompting[^120] | +7% to +27% on PaLM-2L, 2023 | **Situational.** Unreplicated on 2026 models |
| Prompted iterative retrieval | +9.0 vs +10.0 for good one-shot | **Situational.** Choose it to save tokens |
| RL-trained search policy | +41% / +20% over baselines | **Worth it** if you can train |
| Full GraphRAG | Preference wins, parity on faithfulness | **Not worth it** at full price |
| Multi-agent deep research | +90.2% on an internal eval | **Situational.** ~15× tokens |

### What "hybrid" actually needs to contain

"Use hybrid search" is repeated everywhere without saying which components or
why. The components are classical information retrieval, and each exists to
cover a specific failure of the others:

- **Full-text search** (BM25[^122] and its relatives) catches the exact tokens
  embeddings blur — error
  strings, flag names, host names. When someone pastes a literal error, an
  exact match is the best evidence and no amount of semantic similarity should
  outrank it.
- **Embeddings** catch paraphrase. Someone asking "restore hangs after
  manifest load" and the engineer who answered "checkpoint stalls on the NFS
  mount" share no vocabulary.
- **Term rarity** (inverse document frequency[^118]) separates signal from
  filler. "Sounds good, thanks!" sits
  close to many queries in embedding space and scores near zero once you
  weight by how rare its words are.
- **Age decay**, because answers expire. Two threads may both answer the
  question, and the older one may describe infrastructure that no longer
  exists.

The fusion step matters as much as the components. Reciprocal rank fusion
scores each document `weight / (k + rank)` in every list it appears in, with
`k = 60` from the original 2009 paper.[^116] That constant is what makes
**consensus beat a single strong vote** — a document ranking moderately across
four retrievers outranks one that tops a single list. It also sidesteps the
calibration problem: ranks fuse without the incompatible scoring scales of
BM25 and cosine similarity needing to agree.

None of this is new; the 2009 result is that rank fusion beats the learning-
to-rank methods of its day. Age decay is how practitioners handle staleness,
which chapter 12 lists as unmeasured. They treat recency as one signal among
several rather than a cut-off. Nobody has published how much it helps.

Three of the techniques above deserve expanding, because the received wisdom
is wrong.

**Multi-query fusion is a benchmark win that reverses in production.** An
enterprise deployment saw Hit@10 fall from 0.51 to 0.48 despite higher
retrieval-level recall.[^45] The reason is structural: fusion widens the
candidate pool, but the reranker and the token budget were already the
binding constraint, so the extra candidates get discarded and only the
latency remains. Treat this as a risk to test for any recall-widening trick
evaluated without the downstream reader in the loop, especially when the
reranker or token budget already binds.

**Agentic retrieval is weaker than the discourse suggests.** A direct
comparison found one-shot retrieval with a wider net and a chunk filter
scored 91.0% against iterative retrieval's 90.0%, while the iterative version
made 59–97% more retrieval calls.[^46] Combining the two underperformed
either alone. Iterative retrieval's case is token efficiency, not
accuracy.

The important distinction: Search-R1's +41% comes
from *reinforcement learning on the search policy*.[^47] That does not
transfer to "give the model a search tool and prompt it to iterate", which is
what most people mean by agentic retrieval.

**GraphRAG's headline wins are preference wins, not accuracy wins.**
Comprehensiveness, diversity and empowerment are LLM-judge pairwise
preferences on open-ended questions with no ground truth. Faithfulness — the
one objective axis measured — came out *similar* to baseline retrieval.[^48]
Microsoft's own successor prices full GraphRAG's indexing at 1,000× that of
LazyGraphRAG, which is a strong concession about the original's cost.[^49] I
found no independent replication showing GraphRAG beating a well-tuned hybrid
pipeline on a ground-truth benchmark.

### How much do two search providers agree?

If you are choosing between search providers, or considering running more than
one, the practical question is how much a second one adds. Here is a
measurement: 24 queries across six kinds — factual, fresh, technical,
commercial, vague and analytical — sent to three providers on 4 August 2026,
ten results each, search only. Seventy-two calls, four of which failed and are
excluded rather than counted as empty.

Results are compared by **host**, not URL. Host diversity is a coarse proxy for
source diversity: pages on one host may be independent, and pages on different
hosts may repeat the same material.

| Provider | Results | Distinct hosts | Host concentration |
|---|---|---|---|
| Exa | 10.0 | 8.3 | 0.18 |
| Tavily | 9.0 | 7.8 | 0.16 |
| Firecrawl | 10.0 | 9.0 | 0.13 |

Overlap between providers, as the share of hosts they agree on:

| Pair | Overlap |
|---|---|
| Tavily and Firecrawl | **0.62** |
| Exa and Firecrawl | 0.21 |
| Exa and Tavily | 0.18 |

This has several implications.

**A second provider adds real coverage.** Any one provider returned about
eight or nine distinct hosts; all three together returned **15.5 per query**.
So roughly half the hosts one provider gives you, the others do not.

**But not all second providers are equal.** Tavily and Firecrawl agree with
each other far more than either agrees with Exa. Exa contributed **61% unique
hosts**; the other two contributed about 26% each. If you already run one of
that pair, adding the other helps much less than the overlap table suggests.
Which providers pair well is worth measuring for your own
queries — the pairing here is a property of these three, not a general result.

**Results are not concentrated on one site.** Concentration ran 0.13–0.18,
where 1.0 would be a single host taking everything. No provider dumped ten
results from one domain.

The overall host distribution matters too. Across all 295 distinct
hosts, the most frequent were `reddit.com` (30 appearances), `youtube.com`
(24), `arxiv.org` (19), `medium.com` (19) and `docs.vllm.ai` (19). Forum,
video and blog content sit near the top for technical queries. That is not a
fault in the providers; it reflects where the answers are. It does mean
a pipeline with no source-quality policy will pass a large share of Reddit
content to the model.

Limits: 24 queries is small, one run per query, English only, and a snapshot
of three products that change. Rerun it with your own queries rather than
inheriting these numbers — the script is
`docs/guide/measurements/source_overlap.py`.

### Live web acquisition

Two facts shape the architecture.

**A snippet and a page differ by an order of magnitude.** An average page is
about 2,500 tokens, a large documentation page about 25,000, a research PDF
about 125,000.[^50] A ten-result search costs a few thousand tokens; fetching
those ten pages costs 25,000; fetching ten PDFs exceeds a million-token
window. Search to find candidates, then fetch selectively.

**Search is priced per query and fetching per token.** Anthropic charges $10
per 1,000 searches and nothing per fetch beyond tokens. Brave is $5 per
1,000. Exa is $7 per 1,000 searches plus $1 per 1,000 pages *per content
type*. Firecrawl bills one credit per page. Jina Reader bills output tokens
with a 10,000-token minimum per search.[^51] These shapes should determine
your ratio of searching to reading.

Two constraints to design around: platform fetch tools respect
`robots.txt`, and they cannot fetch a URL the model invented — it must have
appeared in context already. Search-then-fetch is enforced, not only
advisable. And crawling is becoming a priced resource: Cloudflare's
pay-per-crawl uses HTTP 402 plus cryptographic crawler signatures so
publishers can allow, charge or block.[^52]

---

## 5. Extraction and chunking

Two steps that sit between fetching a document and retrieving from it. Both
are usually done badly, and one of them matters far more than the literature
suggests.

### Extraction is where the tokens are

Chapter 1 measured four pages: removing tags, scripts and similar HTML
machinery removed 87–99% of their tokens. For raw-web ingestion, extraction
can therefore remove a large cost before any retrieval decision.

The reference comparison is Zyte's article-extraction benchmark — 181 pages,
scored by four-gram shingle matching against article-body ground truth,
normalised per document. It was expanded to 28 extractors across four
languages in March 2026 and then **archived that June**.[^53] It is
the most complete comparison available, and it is not maintained.

| Extractor | F1 | Precision | Recall |
|---|---|---|---|
| `rs_trafilatura` (Rust) | 0.970 | 0.951 | 0.990 |
| `go_trafilatura` | 0.960 | 0.940 | 0.980 |
| **trafilatura** (Python) | 0.958 | 0.938 | 0.978 |
| Readability (JS) | 0.947 | 0.914 | 0.982 |
| readability-lxml | 0.922 | 0.913 | 0.931 |
| `justext` | 0.804 | 0.858 | 0.756 |
| BeautifulSoup (plain) | 0.665 | 0.499 | 0.994 |
| `html2text` | 0.662 | 0.499 | 0.983 |
| `htmd` (Rust) | 0.184 | 0.102 | 0.970 |

**Read the precision column, not the F1 column.** Precision here is roughly
the share of the output that is actually article body. Every plain
HTML-to-markdown converter has recall near 0.99 and precision between 0.10
and 0.52: they drop no body text and keep all the non-body text.

So `htmd` returns much more non-body text than the other entries, while
trafilatura's output overlaps the article-body ground truth far more precisely.
These are four-gram shingle scores, not token measurements, so they do not by
themselves establish a token-cost multiplier.

Three practical notes. Readability — the most-deployed extractor, and
what most "reader mode" pipelines use — scores 0.914 precision but has
repeatedly measured lower recall than trafilatura in the table above, meaning
it drops body text without any indication; a recall failure at the acquisition layer is
invisible in every downstream metric. `MarkItDown`, with 171,000 GitHub stars, is a *converter*: on HTML it
wraps markdownify and removes no boilerplate at all. And none of these tools
render JavaScript, which is an unmeasured failure across a large slice of the
modern web.

One caution against over-cleaning. A February 2026 paper found that
extractors scoring similarly on benchmarks produce very different *page
survival*, and that taking the union of several extractors raised token yield
by up to 71% with no benchmark regression.[^54] Aggressive cleaning drops
whole pages, and nothing downstream reports it.

### Chunking matters less than you have been told

Chroma's evaluation — five corpora, 328,208 tokens, 472 queries, scored at
token level.[^55] Chroma sells a vector store rather than a chunker, so they
have less stake in this result than most, but it is one company's unreplicated
evaluation:

| Strategy | Recall | Precision |
|---|---|---|
| Recursive splitter, 200 tokens, no overlap | 88.1% | 7.0% |
| Cluster semantic chunker, 200 tokens | 87.3% | 8.0% |
| LLM-guided semantic chunker | 91.9% | 3.9% |

The spread between best and worst strategy was about 9% recall. The LLM-guided
splitter bought 3.8 points of recall and halved precision. Reducing overlap
improved the overlap-with-ground-truth measure.

**A plain recursive splitter at 200–400 tokens with no overlap is a strong
default.** Revisit it only after you have evidence that chunking is your
bottleneck, which it usually is not.

The pattern that helps here is [parent-document retrieval](#g-parent-document)
— widely used, with no published measurement of it found:
match on small chunks because they match precisely, then hand the model the
larger section each chunk came from. It sidesteps the precision/recall
trade-off structurally instead of tuning along it. Treat it as a design pattern
rather than an evidenced technique.

### Contextual retrieval: the best-documented vendor upgrade

Chunks lose their referents. "The company" and "this quarter" mean nothing
once a paragraph is separated from its document. Contextual retrieval fixes
this by having a model write a 50–100 token situating header for each chunk
at index time.

Anthropic's chain, on top-20 retrieval failure rate. This is Anthropic
evaluating its own technique, with no independent reproduction found:[^44]

| Configuration | Failure rate | Reduction |
|---|---|---|
| Baseline | 5.7% | — |
| + contextual embeddings | 3.7% | −35% |
| + contextual BM25 | 2.9% | −49% |
| + reranking | 1.9% | −67% |

Cost: about **$1.02 per million document tokens**, one-off, and only that
cheap because the document prefix is cached.

One important qualification. A SIGIR 2026 taxonomy paper found contextual
chunking improves *corpus-level* search and **degrades in-document
retrieval**.[^56] Adding document-level context makes chunks more
distinguishable across a corpus and less distinguishable from their siblings.
If you are searching within one document, it hurts.

### Two steps further, both unmeasured

Contextual retrieval annotates a chunk with its situating context. Two
extensions of the same idea show up in production systems, neither with
published numbers behind it.

**Rewriting into a canonical shape.** Rather than embedding source text at
all, an extraction pass produces a structured record — for a support thread,
the question someone would search for, a summary, the resolution, the systems
involved — and that record is embedded instead. The reasoning is that
heterogeneous sources embed badly together because their shapes differ: a code
comment, an incident record and a wiki paragraph are not comparable objects
even when they answer the same question. Normalising them makes the vector
space mean something consistent.

**Gating what enters the index at all.** Content is embedded only if it clears
a quality threshold — containing a rare term, reaching a minimum length, or
carrying some signal of value such as reactions or links.

The second matters more, because it is admission control at
*ingestion* rather than at query time, and every other chapter here argues
about the latter. It happens once per document rather than once per query, and
it removes the low-signal material that chapter 4's distractor
findings say costs you most. If your corpus is conversational — and chapter
7's deduplication numbers found high repetition in one conversational corpus
— ingestion gating is a low-cost design pattern worth testing.

Both are sensible and neither is evidence. Take them as design patterns to
test, not as findings.

**Late chunking** is a cheaper variant of contextual retrieval: embed the
whole document at token level, then pool into chunks, so each chunk embedding
is conditioned on its neighbours with no per-chunk model call. The technique
and the numbers are both Jina AI's, evaluating their own method.[^57] The gains
are smaller and less consistent than contextual retrieval's — nDCG@10 went 64.20→66.10 on SciFact, 23.46→29.98 on NFCorpus,
and *unchanged* on Quora. Gains grow with document length. A head-to-head
found contextual retrieval slightly ahead but far more expensive, concluding
that "neither technique offers a definitive solution".[^58]

## 6. Ranking and reranking

Retrieval and ranking are different jobs. Retrieval asks "which hundred of
these million documents might be relevant?" Ranking asks "which five of these
hundred actually are?" They have different cost profiles, and the second
contributes most of the quality.

### Why a second pass helps

First-stage retrieval has to be fast over the whole corpus, so it compares
pre-computed representations. An [embedding](#g-embedding) for a passage is
computed once, before anyone asked a question, so it cannot be shaped by the
question.

A [cross-encoder](#g-cross-encoder) reads the question and one candidate
passage *together* and scores the pair. That is more accurate, too slow to
run over a corpus, and affordable over fifty candidates.

This is why the standard design retrieves many candidates, reranks them, and
admits few.

### The measured gain

The clearest published chain is Anthropic's, on top-20 retrieval failure rate
— their own evaluation of their own technique:[^44] contextual embeddings took the baseline from 5.7% to 3.7%; adding
contextual BM25 reached 2.9%; **adding reranking reached 1.9%**. So the
reranking step alone removed about a third of the failures that survived
everything before it.

### Bigger rerankers stop helping quickly

Published sizing data, from the standard cross-encoder
family trained on MS MARCO:[^97]

| Model | nDCG@10 (TREC DL19) | Documents per second |
|---|---|---|
| TinyBERT-L2 | 69.84 | 9,000 |
| MiniLM-L6 | 74.30 | 1,800 |
| MiniLM-L12 | **74.31** | 960 |

About 4.5 points for a 5× throughput cost, then no meaningful gain on this
benchmark while throughput roughly halved again. If your queries resemble the
training distribution, the six-layer model is the stronger starting point.

The newer models add a 32,000-token context — enough
to rerank whole documents rather than fragments — and **instruction
following**, where you tell the reranker in plain words what "relevant" means
for this query. One vendor measures the instruction as worth roughly as much
again as the model-generation upgrade itself.[^98] That is a vendor
evaluation, but the mechanism is plausible: an instruction encodes domain
semantics no generic relevance model can infer.

### Do not use a frontier model as your reranker

The frontier model seems like the strong option. On quality,
latency and cost simultaneously, a dedicated cross-encoder wins.

One vendor's comparison across 13 datasets puts its cross-encoder ahead of
GPT-5 by 12.6% and Gemini 2.5 Pro by 13.4% on nDCG@10, while being 36× and 48×
faster and 25–60× cheaper.[^99] Treat the magnitudes as vendor-configured; the
direction is corroborated independently, and the mechanism is not in dispute —
a listwise LLM reranker needs several sliding-window passes where a
cross-encoder needs one batched forward pass.

The same source concedes one exception: LLM reranking does help when
first-stage retrieval is bad. Fix the first stage instead.

### It is cheap relative to what it saves

From chapter 8: a
reranker over 20,000 candidate tokens costs about $0.001 at published
per-token reranking prices, and saves about $0.09 of frontier-model input.
Roughly **90:1**.

That ratio collapses in one specific regime — when the tokens it saves were
already going to be served at a cached-read or budget-model rate. Against a
cheap model on a warm cache, a reranker can cost more than it saves.

One third-party latency measurement: hosted rerankers
measured at 595–603 ms end to end against 188 ms for the same class of model
self-hosted.[^100] That gap is the network round trip, not the model. If p95
latency matters, self-host. (Methodology on that benchmark is underspecified
and a different source reports 392 ms for the same hosted model — treat the
*ratio* as real and the absolutes as indicative.)

### The options, roughly ordered by cost

| Approach | What it does | When |
|---|---|---|
| **Bi-encoder** (plain embeddings) | Pre-computed vectors compared by distance. The document is compressed before the query is known, so query-specific evidence is destroyed at index time | First stage, always |
| **Late interaction** (ColBERT-style) | One vector per token, scored by best-match-per-query-term. Recovers term-level sensitivity at retrieval time, at real index cost | See below |
| **Cross-encoder reranker** | Reads query and passage together with full attention | The default second stage |
| **LLM reranker** | Prompts a general model to order passages | Rarely. See above |

**On late interaction specifically.** The cleanest controlled comparison —
same backbone, same size, single-vector against late-interaction — puts the
gap at **57.22 against 56.20 nDCG@10**.[^101] About one point. Storage costs
have fallen sharply (residual compression, product quantisation, on-disk
token embeddings), so the old objection is weaker than it was, but for English
text retrieval a dense first stage plus a cross-encoder is the better spend.

There is one exception: **for visual documents — PDFs, slides, scans,
tables — late interaction over page images is the default**, because it skips
OCR and layout parsing entirely rather than competing with it.[^102]

### Rank for attribution, not similarity, if you need citations

On a legal question-answering
benchmark, semantic similarity did not correlate with which passages the model
actually cited — **similarity-based ranking performed worse than random
selection** at surfacing the cited paragraphs.[^103]

If your product shows citations, the passages that support the answer and the
passages that look most like the question are different sets. Rank for the
first and measure it separately.

### Put the context back after you rank, not before

Reranking works on chunks, and chunks have had their surroundings cut off. The
heading that said which version this applies to, the preconditions above and
the caveat below were all removed by whatever split the document.

The fix is cheap and easy to get the wrong way round: once the winners are
chosen, pull their neighbouring sections back in. Expanding *before* ranking
would defeat the point, since you would be scoring padded chunks against each
other. Expanding after leaves the ranking calculation unchanged and gives the
model a complete passage instead of an orphaned paragraph; its effect on the
reader still needs measuring.

This is chapter 5's parent-document retrieval applied at the end of the
pipeline rather than the start, and the two combine: match and rank on small
chunks, then give the model the larger section.

### The one thing to measure

Measure **retrieval failure rate at your actual k**, not nDCG: how often is
the answer absent from what the model finally sees? That is the number that
predicts end quality, and it is the number Anthropic's chain reports.



## 7. Selection and ordering

You have ranked candidates. Now decide how many to admit and in what order.

Selection under a token budget is less well covered than retrieval or
reranking. The claims below are marked as measured or as reasoning.

### The budget is shared, and most of it is already spent

From chapter 1: a real session spent about 28,000 tokens on fixed overhead and
185,000 on conversation and tool results. Retrieved evidence competes with the
system prompt, tool definitions, skills, memory, conversation history, tool
results and the reserved space for the answer.

Deferring tool definitions frees more room than tightening
your retrieval will (85% of a tool budget, chapter 1). And on a
long-running agent, the retrieved-evidence budget is whatever survives the
conversation, which shrinks every turn.

### Scope before you select

The cheapest selection decision is the one that excludes most of the corpus
before retrieval runs at all.

Every enterprise search product does this, and the reason is that "search
everything everywhere" stops being useful as a corpus grows across teams —
compiler engineers do not want infrastructure runbooks in their results, and
vice versa. The usual shape is to scope queries by default to a bundle of
sources relevant to the user's work, chosen once and changeable.

The principle precedes every technique in this chapter: **a
source that is out of scope cannot become a distractor.** Given chapter 2's
finding that semantically-close-but-wrong material is expensive, excluding an
irrelevant subject area is a useful filter before reranking.

The trade is recall on cross-domain questions, which is why the scoping should
be a default rather than a fixed restriction.

### How many to admit

**Measured.** Reader accuracy saturates well before retriever recall does:
going from 20 to 50 documents gained about +1.5% and +1% on two models while
recall kept climbing.[^11] Anthropic found k=20 best for one setup;[^44]
practitioner guidance after reranking converges on 5–10.

**Measured.** The risk scales with distractor *similarity*, not count. One
convincing near-miss hurts more than several obvious irrelevancies, and over
60% of queries had at least one hard distractor in the top ten of a dense
retriever.[^28]

**Measured.** Cutting admitted context can raise accuracy: 63% fewer tokens,
20.6 points better, in the Microsoft pruning study.[^24]

The rule that follows: retrieve many candidates, rerank them, admit few, and
treat k as something to tune downward against your end metric rather than upward
against recall.

### Greedy top-k is probably the wrong algorithm

Taking the k highest-scoring passages maximises per-item relevance, which is
not the same as assembling the most useful *set*. It systematically
over-selects near-duplicates, because near-duplicates of a good passage all
score well. Maximal marginal relevance[^121] was the classic answer.

Three independent 2026 papers formalise this as knapsack-constrained
subset selection: each candidate has a token cost and a marginal utility, and
you are choosing a set under a budget.[^104][^105][^106] If the objective is
monotone submodular — each added item helps less than the last — the
budget-aware algorithm in the cited clinical-text study carries a `1−1/e`, or
roughly 63%, approximation guarantee.[^104] That guarantee depends on the
stated objective, constraint and algorithm; it does not attach to every greedy
implementation.

The objective that wins is **facility location**: reward a chosen set for
being a good stand-in for everything in the candidate pool, not only for
being varied. Maximal marginal relevance penalises similarity to what you have
already picked but has no notion of covering the pool, which is why it comes
third.

The head-to-head result:[^105]

| Method | Evidence recall | End-to-end accuracy |
|---|---|---|
| Top-k | **0.933** | 50.0% |
| MMR | 0.895 | 44.0% |
| Facility location | 0.909 | **52.0%** |

**The winner had lower recall than top-k and higher accuracy.** Recall and
end-to-end quality can move in opposite directions, which is why recall alone
is the wrong target.

That decoupling has been measured directly: a diagnostic asking whether the
gold answer *survives as a contiguous span in the packed context* correlates
with exact-match at 0.39–0.55, against 0.31 for document recall — and there is
a **4.6× exact-match gap among questions where all the gold documents were
retrieved**.[^106] You can retrieve everything and still lose the answer in
packing.

So do not tune your packer on recall@k alone. Measure whether the answer
survives. Three 2026 papers agree on this and none has been replicated.

One baseline worth running: at very tight budgets, taking the
*front* of the document beats clever selection on front-loaded corpora, and
loses badly on corpora that are not front-loaded.[^104] Know which yours is.

### Removing redundancy

Deduplication is the cheapest correct step, and it has been measured. Byte-exact
chunk-level deduplication removes:[^107]

| Corpus type | Context removed |
|---|---|
| Clean academic | 0.16% |
| Enterprise | 24.03% |
| Conversational | 80.34% |

— with **zero quality regression across four model vendors**. Eighty per cent
of a conversational corpus is exact repetition.

So: run byte-exact deduplication always, because it is free. Add fingerprint
near-duplicate detection (MinHash, SimHash) when your corpus is scraped or
conversational. Use embedding-similarity deduplication only when those
two miss paraphrases — it is roughly seven times slower than the alternatives
for about 10% additional removal.[^108]

### In what order — and the U-curve may not survive

The received wisdom here is under serious challenge.

Chapter 2 reported the U-curve: performance best at the start and end of the
context, worst in the middle. A SIGIR 2026 reproducibility study tested five
models across two datasets under a controlled protocol and **failed to
reproduce it** — "performance is again nearly flat across placements".[^109]

Their diagnosis of the disagreement is the valuable part: **topic sampling
dominates the variance**, and small topic sets both manufacture and mask
ordering effects. Stable conclusions required 1,000 topics on one dataset and
2,000 on the other. Most published ordering experiments use far fewer. Read
that against chapter 11's sample-size numbers — this is the same problem,
diagnosed independently.

The one ordering effect that survived their protocol is small and points
against intuition: at k=50–100 on multi-hop questions, **reverse ordering —
best evidence last — gained 2–3 F1**. At k=5–10, ordering made almost no
difference. On the other dataset, no ordering sensitivity at any size.

Two effects that are large and do not transfer:

- **Multimodal is the opposite.** In retrieval-augmented visual QA, gold-first
  against gold-last produces gaps of **16 to 26 points** across five
  models.[^110] Primacy, strongly. Do not carry a text heuristic into a
  document-image pipeline.
- **Tool results invert too.** Accuracy at position 8 exceeded position 1 by
  5–75%.[^20] With tool output it is the *oldest* results being ignored.

And from chapter 2: coherent filler competes for attention
more than incoherent filler.[^13] Well-written surrounding context is not
automatically safer context.

**What should you do?** Below about 20 chunks on text, ordering is
close to a non-issue — order for prefix-cache stability instead, and take the
cost saving. Above 50 chunks on multi-hop tasks, put the best evidence last. In
multimodal, put it first.

### Order for the cache

Chapter 9 posed the tension between ordering for relevance and ordering for
cache stability. There is evidence for how to resolve it.

Keeping a prefix tree over recently-served evidence sequences and placing the
most reusable prefix first — purely at the prompt layer, no serving changes —
gave a **20–33% reduction in median time-to-first-token**, capturing 97.5% of
the benefit an oracle ordering would achieve, **with no degradation in answer
quality**.[^111]

Set that against the ordering literature above: relevance ordering is worth at
most 2–3 F1 on a narrow slice of tasks, and cache-aware ordering is worth
20–33% of your latency at no measured quality cost. On the one study that
measured this, the cache wins the trade. Unreplicated.

### Budget evidence first, then fit everything else round it

The failure mode when scaffolding grows is **displacement, not interference**.

A 2026 study at a 4,096-token window found performance holding through
moderate coordination overhead and then falling sharply once residual evidence
dropped to a few hundred tokens. The decisive ablation: when the coordination
tokens were added *outside* the fixed budget so evidence stayed intact, three
commercial models stayed correct **even at a 95% coordination ratio**.[^112]

The scaffolding was not the problem; crowding out the evidence was.

Budget evidence as a floor and fit the rest around
it, rather than the reverse. And compress tool schemas before anything else —
they are the highest-token, lowest-information consumer, and compressing them
recovered **20.5 percentage points of exact-match** at an 8,000-token budget
where uncompressed schemas scored 2.6%.[^113]

On how far you can cut: one study found 92.7% success at 75% retained context
against a 93.8% full-context baseline, with sharp divergence between 50% and
35% retention.[^114] Treat ~75% as safe and ~35% as dangerous — one paper,
days old at the time of writing — and measure where your own cliff sits.

### What is not known

This chapter cannot tell you:

- **No validated allocation formula** across system prompt, tools, memory,
  evidence, history and output. Anyone quoting percentages is quoting a
  heuristic.
- **Output reservation is undocumented.** No study found measures the cost
  of under- or over-reserving output tokens.
- **No mechanistic account** reconciling text-RAG order-insensitivity with the
  large multimodal primacy effect.
- **No published measurement of multi-source overlap or host concentration**
  beyond chapter 4's, which is 24 queries and a single run.



## 8. Compression

Compression is usually the first thing people try. It should be the last.

### The headline number does not survive checking

The most-cited claim in this area is "up to 20× compression with little
performance loss", from Microsoft's LLMLingua in October 2023.[^59] Two things
about it.

First, what it was measured on: GSM8K, BBH, ShareGPT and an arXiv set. GSM8K
and BBH are few-shot reasoning prompts — formulaic, highly repetitive text
where the same scaffolding recurs in every example. That is the most
compressible material there is, and the least like a retrieved web page.

Second, what happened next. The same team's successor, LLMLingua-2, claims
**2×–5×**.[^60] LongLLMLingua, the retrieval-oriented member of the family,
claims up to +21.4% accuracy at around 4× fewer tokens on
NaturalQuestions.[^61]

When a team's own follow-up paper claims a quarter of its predecessor's
headline, treat the headline as a best case on favourable text.

For completeness, the repository is MIT-licensed with 6,522 stars, and has had
one commit since October 2025 (checked 4 August 2026). It is not abandoned. It
is not under active development either.

### The families, briefly

| Family | How it works | Needs model weights? |
|---|---|---|
| **Token-level** (LLMLingua, LLMLingua-2) | Drop individual tokens judged low-information, by perplexity or by a trained classifier | No |
| **Extractive** (RECOMP, and truncation) | Select whole sentences or passages and discard the rest | No |
| **Abstractive** | Have a cheaper model summarise the retrieved documents | No |
| **Soft / learned** (gist tokens, ICAE, xRAG) | Compress into internal model states rather than text | **Yes** |

The last row is the practical dividing line. Soft compression is the most
elegant work in the area and is unavailable to anyone using a hosted API,
because it needs access to the weights.

RECOMP is worth singling out, and not for its compression rate.[^62] Its
extractive and abstractive compressors reach a compression rate as low as 6%
with minimal loss — but its most useful feature is that it can **return an
empty string** when the retrieved documents are not relevant. That is not
compression. That is admission control, and it is the more valuable idea.

### The arithmetic

Take 50 candidate chunks of 400 tokens — 20,000 tokens — reduced to 5 chunks,
saving 18,000 tokens per request. Prices are per million tokens, checked
4 August 2026.

| Approach | Cost of the step | Value of tokens saved | Ratio |
|---|---|---|---|
| Rerank ($0.05/M) → Opus 5 uncached input ($5.00/M) | $0.0010 | $0.090 | **90:1** |
| Rerank → Opus 5 cached read ($0.50/M) | $0.0010 | $0.009 | **9:1** |
| Rerank → Haiku 4.5 cached read ($0.10/M) | $0.0010 | $0.0018 | 1.8:1 |
| Rerank → cheap model, cached ($0.02/M) | $0.0010 | $0.00036 | **0.36:1 — losing money** |
| **LLM compression** (Haiku input $1.00/M) → Opus 5 uncached | **$0.020** | $0.090 | 4.5:1 |
| **LLM compression** → Opus 5 **cached read** | **$0.020** | $0.009 | **0.45:1 — losing money** |

**A reranker is nearly free relative to frontier input tokens and almost
always pays.** It stops paying when the tokens it saves were already being
served at a cached or budget rate.

**LLM-based compression is roughly twenty times more expensive than reranking
for the same job**, before counting its output tokens. Against uncached
frontier input it still nets out positive. Against *cached* reads it is a
clear loss.

### Compression and caching interact

At modest compression ratios, compression can cost more than a warm cache
saves. The interaction is not universal, because real cache-hit rates are
below 100%.

Compressed context is *derived*. It changes whenever the retrieved set
changes, which means it cannot live in a stable cached prefix. Caching requires
the same bytes every time; compression produces different bytes every time.

So the comparison is not "compressed tokens versus uncompressed
tokens". It is "compression cost versus the alternative of caching the
uncompressed corpus and never compressing at all". A summarisation pass that
runs on every request, over content that would otherwise have been a cache
hit, can spend more than it saves.

One July 2026 study measured a cache-hit plateau around 0.83 on one production
API and found query-aware compression overtaking naive caching at compression
ratios around 6×.[^124] That is one recent study, not a general threshold. The
decision therefore needs the observed cache-hit rate, compression ratio and
prices for the route in question.

Latency runs the same way. A compression call is a serial round trip before
prefill can start. On a cold cache it usually still wins, because prefilling
18,000 extra tokens takes longer. On a warm cache, where those tokens would
have been reused cache blocks rather than fresh prefill, it is added
latency with no saving.

### Measure the right thing

The compression ratio is a bad headline metric. It is the input to the
decision, not the outcome.

What to measure instead: downstream task score, claim recall (did every fact
needed to answer survive?), and citation support. Compression that removes
the one fact needed to answer is not compression.

There is a measurement of that failure. Anthropic's cookbook ran a
research agent over eight documents totalling about 329,000 tokens. After
compaction, **3 of 3 high-level facts were preserved and 0 of 3
appendix-table details survived**.[^63] Compression keeps the gist and loses
the specifics, which is fine until the specifics were the answer.

### When to compress at all

A decision rule, in order:

1. **Retrieve less.** Cheaper than compressing, and it removes distractor
   load rather than concentrating it.
2. **Extract properly.** Removing HTML machinery cut 87–99% of tokens on the
   four pages measured in chapter 1. A content extractor can remove more, but
   must be checked for lost body text.
3. **Rerank and truncate.** Cheap, and it improves quality rather than
   trading it.
4. **Cache the stable part.** A 90% discount for getting the ordering right.
5. **Then consider compression** — for content that is large,
   redundant, uncacheable because it varies per request, and headed for an
   expensive model.

Most pipelines are better served by the first four steps than by the fifth.

---

## 9. Caching

The largest cost lever is prompt caching: a 90% discount for putting your
content in a stable order.

### How it works, in one paragraph

When a model reads your prompt, it computes intermediate results for every
token — the [KV cache](#g-kv-cache). Prompt caching stores that work on the
server so a later request starting with identical text can reuse it. You pay a
small premium to write the cache and a large discount to read it. Note that
**cached tokens still occupy the window**: caching changes what you pay, not
how much space it takes.

### The numbers, checked 4 August 2026

Every figure in this section comes from the providers' own pricing and caching
documentation, fetched on 4 August 2026.[^123] They are the authority for how
their own billing works, and these change often — recheck before quoting.

| Provider | Mechanism | TTL | Minimum | Write | Read |
|---|---|---|---|---|---|
| **Anthropic** | Explicit breakpoints, max 4 | 5 min or 1 hour | 512–4,096 tokens by model | 1.25× (5 min), 2× (1 hour) | **0.1×** |
| **OpenAI** (GPT-5.6+) | Automatic, optional explicit | 30 min only | 1,024, strict | 1.25× | ~0.1× |
| **OpenAI** (earlier) | Automatic | 5–10 min idle, max 1 h | 1,024–2,048 | free | varies; gpt-4o only 50% |
| **Google Gemini** | Implicit by default | not documented | 2,048–4,096 | free | ~0.1× |
| **Google** explicit | Cache objects | settable | as above | free per token | **plus hourly storage** |

Break-even on Anthropic: the 5-minute cache pays for itself after one read
(1.25 + 0.1 = 1.35 against 2.0 uncached); the 1-hour cache needs two.

**Google bills storage by the hour.** Explicit context caching bills $1.00 per million
tokens per hour on the Flash line and $4.50 on Gemini 3.1 Pro Preview. A
200,000-token cache held for an hour on 3.1 Pro costs $0.90 in storage before a
single read. Anthropic and OpenAI charge a one-off write premium instead.

**Anthropic's minimum is not monotonic across generations**, and falling below
it produces no error: 512 tokens on Opus 5, 1,024 on Opus 4.8, 2,048 on Opus 4.7,
4,096 on Opus 4.6. Below the minimum, "requests to cache fewer than this number
of tokens will be processed without caching, and no error is returned".[^64]

### The tension with everything else in this guide

Caching matches an exact request prefix. A content change before a breakpoint
invalidates reuse at or after it; semantic equivalence does not preserve a
cache hit.[^123]

So content must be ordered by *stability*, not by *relevance*:

```
tools (frozen)  →  system prompt (no timestamps)  →  cached history
                →  [BREAKPOINT]  →  retrieved evidence, this turn's question
```

This creates a trade-off with reranking retrieved passages per query. The
usual architecture puts volatile evidence after the last stable breakpoint,
where it is billed at full rate. Repeated jobs are an exception: chapter 7's
cache-aware ordering result shows that recurring evidence sequences can be
arranged for prefix reuse. Measure reuse before treating retrieved evidence as
stable.

### What mis-ordering costs

A 50,000-token evidence block on Opus 5 ($5.00 input, $0.50 cached read, $6.25
five-minute write):

| Request state | Cost for the 50,000-token block |
|---|---|
| Warm cache read | **$0.025** |
| Uncached input after the breakpoint | $0.25 |
| Five-minute cache write | **$0.3125** |

If re-ordering forces a write on every request, it costs **more than not
caching at all**: about $288 per thousand requests more than warm reads, and
25% more than uncached input. An amortised comparison must also include the
initial write, later reads, expiry and the observed hit rate.

### What silently invalidates a prefix

Things that invalidate a prefix, worth grepping for:

- `datetime.now()` or `Date.now()` anywhere in the system prompt
- request IDs or UUIDs interpolated into the prefix
- `json.dumps()` without `sort_keys=True`, or iteration over a set
- per-user data in a shared system prompt
- conditional system sections — each flag combination is a distinct prefix
- a tool set that varies per user
- **the 20-block lookback ceiling**: Anthropic's breakpoints walk back at most
  20 content blocks, so an agent turn appending 30 tool calls misses on the
  next request with no error. Place an intermediate breakpoint every ~15
  blocks; you have four to spend.
- **concurrency**: a cache entry only exists after the first response begins,
  so N parallel identical requests all pay full price. Fire one, wait for the
  first token, then fire the rest.

Anthropic ships a diagnostics beta that names the cause — `system_changed`,
`tools_changed`, `messages_changed`, `model_changed` — reporting only the
earliest divergence.[^65]

### What you cannot touch

The KV-cache research literature is large and almost entirely below the API
boundary. StreamingLLM's attention sinks, H2O's heavy-hitter eviction,
SnapKV's compression, KV quantisation — all require self-hosting.[^66] On
vLLM, automatic prefix caching is on by default; SGLang's RadixAttention is
too, with configurable eviction.

One caution on those headline numbers: 22.2× and 29× speedups were measured
against 2023-era baselines on OPT and LLaMA-1/2. The mechanisms replicate; the
multipliers do not transfer to a well-configured 2026 server.

---

## 10. Agent memory

Everything so far concerned a single request. This chapter is about what
happens over hundreds of them.

### Where the tokens actually go

A published breakdown of a real session on a 1M-token window:[^8] system
prompt 6,200; system tools 11,600; MCP tools 1,200; memory files 3,300; skills
333; **messages 185,400**.

The listed fixed components sum to about 22,600 tokens, making messages about
89% of the listed total. The source reports about 28,000 tokens of fixed
overhead under a broader grouping; against that figure, conversation and tool
results are about 87%. Keep the categories alongside the percentage rather
than collapsing them into one exact share.

The pre-deferral failure shape is the opposite: examples circulate of MCP tool
definitions consuming tens of thousands of tokens before the user types
anything. I could not resolve a primary source for the specific figures that
get quoted, so treat the shape as real and the numbers as unverified.

So there are two different problems. Tool definitions are a fixed cost you fix
once, by deferring. Tool results are a growing cost you have to manage
continuously.

### The four verbs

LangChain's framing is the most useful organiser: **write, select, compress,
isolate**.[^67]

- **Write** — put things outside the window (files, memory, scratchpads).
- **Select** — bring back only what is needed.
- **Compress** — summarise or trim what remains.
- **Isolate** — give separate work its own window.

### Compaction, and what it loses

[Compaction](#g-compaction) replaces a long conversation with a summary.
Anthropic's server-side implementation triggers by default at 150,000 input
tokens, minimum 50,000, and the docs state the consequence: "once
content is compacted, the raw history is permanently discarded".[^68]

The measured fidelity result from chapter 8 is the one to remember: 3 of 3
high-level facts preserved, 0 of 3 detail-table entries. Compaction keeps the
shape of what happened and loses the particulars.

Claude Code's published order of operations is instructive because it treats
these as different tools: it clears older tool outputs first, and only
summarises if that is not enough. Clearing is mechanical and costs no
inference. Compaction costs a sampling pass and is irreversible. Use clearing
first.

What survives compaction there is documented: the system prompt and
project-root instructions are re-injected from disk; path-scoped rules and
nested instruction files are **lost** until a matching file is read again;
invoked skills are re-injected but capped at 5,000 tokens each and 25,000
total, truncated from the end — so critical instructions belong at the top of
the file.

Anthropic's own conclusion on long-running work is that "compaction isn't
sufficient" by itself, because it does not reliably preserve instructions
across sessions.[^69] The pattern that works is explicit handoff artefacts: a
progress file, a git history, a task list on disk that the next session reads.

### Sub-agents

Give a sub-agent its own window, let it read widely, and take back only a
summary — typically 1,000–2,000 tokens.[^70] A documented Claude Code example
has a research sub-agent read three files and return 420 tokens, with none of
the reads entering the main window.

### The multi-agent argument, fairly

Two primary sources published a day apart in June 2025, reaching opposite
conclusions.

**Anthropic, for.**[^71] An orchestrator with parallel sub-agents beat a
single agent by 90.2% on their internal research eval. Multi-agent uses about
15× the tokens of chat; single agents about 4×. Their own scoping is usually
dropped when the 90.2% is quoted: it suits "heavy parallelization, information
exceeding single context windows, and numerous complex tools" and is a **poor
fit** for "domains requiring all agents to share identical context, tasks with
many dependencies, and most coding work".

**Cognition, against.**[^72] Two principles: share full agent traces, not
individual messages; and "actions carry implicit decisions, and conflicting
decisions carry bad results". Their illustration is parallel sub-agents
building mismatched halves of the same game.

**The revision.** Cognition's April 2026 follow-up sharpens rather than
retracts: multi-agent works when "writes stay single-threaded and the
additional agents contribute intelligence rather than actions".[^73] They
run a reviewer with no prior context that catches an average of 2 bugs per
pull request, 58% of them severe — clean context *helps* the reviewer,
because of context rot.

**The complication.** Anthropic's own analysis found **token usage alone
explains 80% of performance variance** on one benchmark. A Stanford paper
tested the implication directly and found single agents matched or beat
multi-agent systems under *matched* thinking-token budgets, arguing from the
data processing inequality that passing information through more agents can
only lose it.[^74] Read together, these suggest much of the multi-agent gain
may come from the extra tokens rather than from the architecture.

The reconciled position both parties hold: **isolate for read-heavy,
parallelisable work; keep writes single-threaded.**

A separate study of 1,600+ annotated multi-agent traces found 14 distinct
failure modes, with specification and system-design issues accounting for
about 42% — most failures are design failures, not model failures.[^75]

### Progressive disclosure

The pattern behind skills, deferred tools and just-in-time retrieval: keep a
short pointer in the window, load the full text only if it turns out to
matter. The numbers from chapter 1 — about 100 tokens per available skill, 85%
of a tool budget recoverable by deferring — are all instances of it.

### Keep tools narrow and let the caller orchestrate

A related design choice: make each tool one primitive with narrow, stable
inputs and outputs, and keep the model out of the tool itself.[^117] The tool
runs a query, applies light scoring, and returns raw evidence rows. It does
not decide what to do next.

The agent is then the orchestration engine — it chooses which tools to call,
in what order, and how to combine the results. One production system reports
serving both an agent surface and a fixed plan-execute-synthesise web pipeline
from the same primitives.[^115]

One retrieval layer supports both an agent that
improvises and a pipeline that does not, because the orchestration lives above
it rather than inside it. And the tools stay cheap and fast, since a tool that
calls a model internally adds latency and cost to every use whether or not
that reasoning was needed.

There is a tension with chapter 2: more tools, each narrow, is the
shape that degrades selection accuracy past 40–60 entries. Narrow tools and
*few* tools pull against each other, and the resolution is deferred loading
rather than fewer capabilities.

### Memory benchmarks are not trustworthy yet

The vendor claims are strong; the evidence is not.

**On LoCoMo, the most-cited memory benchmark, a plain full-context baseline
scores about 73% while a leading commercial memory system scores about
68%.**[^76] Stuffing the whole transcript in beat the memory system. On that
dataset the memory layer was a net negative.

Letta reported **74.0% on LoCoMo from storing
conversation histories in a file**, beating a graph-based commercial system at
68.5%.[^77] With full-context at ~73% and a naive filesystem at 74%, the floor
and the plausible ceiling are about six points apart.

The vendor dispute is instructive. One vendor's paper reported a competitor at
65.99%; the competitor re-ran with what it says is a correct configuration and
got 75.14%; the first vendor replied that removing an adversarial category
both sides agree is broken drops the competitor to 58.44%.[^78] That is a
16–26 point spread for the same system on the same benchmark — **larger than
any effect either party claims**. Neither party has to be lying: on a benchmark
this easy, harness configuration dominates system quality.

A 2026 paper shows the confound is structural: swapping **only** the embedding
model in an otherwise identical pipeline moves accuracy by 6.2 percentage
points, and one embedding model flips the conclusion about which approach is
better.[^79]

Both vendors report figures in the low 90s on LoCoMo, on their own blogs
rather than in a shared evaluation. Treat it as retired. LongMemEval is
harder; MemoryAgentBench, which adds *selective forgetting*, is the only one of
the three that discriminates clearly, at around 61%.

**The rule: benchmark against a full-context baseline on your own data before
adopting any memory system.** On the standard benchmark, that baseline wins.

---

## 11. Proving a change helped

Every chapter so far offered a change you could make. This one is about
whether you can tell if it worked. Most published attempts cannot.

### Always include the boring baseline

On LoCoMo, the
most-cited agent-memory benchmark, **a plain full-context baseline scored
about 73% while the leading commercial memory system scored about 68%**, in
that system's own results table.[^76] A naive filesystem scored 74%.[^77]

The memory layer was a net negative, and the papers reported leadership
anyway — because they compared against *other memory systems* rather than
against not having one.

Your baselines are: do nothing, and do the simplest possible thing. For
retrieval, that is no retrieval and single-provider top-k. If your system does
not beat those, nothing else in the evaluation matters.

### How many test cases you actually need

These numbers decide whether your experiment can detect anything.

An evaluation is a survey, so a score has sampling error like any survey. For
a pass/fail score the standard error is √(s̄(1−s̄)/n). On a 164-item set that
is about 3 percentage points — so an 83.6% against 86.7% difference is
indistinguishable from noise.[^88]

Relevant planning figures include:

| To detect | You need | Source |
|---|---|---|
| A 3-point difference in item-level scoring | **~1,000 items** | Power analysis[^88] |
| A 10-point difference in agent pass rates | **~120–200 tasks** | Measured on a real agent suite[^89] |
| A stable majority verdict from an LLM judge | **~11 repeat trials** | Measured judge flip rates[^90] |

**Repeats do not substitute for items.** Re-running each question K times
shrinks only part of the variance: K=2 cuts total variance by a third, K=4 by
a half, and the ceiling is two thirds. On a fixed 198-item set, going from 1
repeat to 10 moved the minimum detectable effect only from 13.2% to
7.5%.[^88] On agent tasks the effect is stronger — task-level variance
dominates, and adding repeats "barely helps".[^89] **Add more tasks, not more
repeats.**

**Lowering the temperature to reduce noise does not work.** It shifts variance
into the conditional means, which no amount of resampling can reduce, and it
can introduce bias. In one worked example it *tripled* the minimum
variance.[^88]

And one that specifically affects context work: if you generate several test
questions per source document, your items are not independent. Naive standard
errors can be **more than three times too small** — on one real benchmark,
1.34 clustered against 0.44 naive.[^88] Cluster your errors on the document,
the repository, or whatever the shared source is.

### Change one thing — and "the model" is more than the model name

A 2026 paper measured what happens when you do not: swapping **only the
embedding model** in an otherwise identical pipeline moved accuracy by **6.2
percentage points**, and one embedding model flipped the conclusion about
which memory approach was better.[^79]

Published comparisons routinely vary the method, the model, the embedder and
the retrieval pipeline at once, then attribute the difference to the method.

But "hold the model fixed" is harder than setting temperature to zero, and
this is not widely known:

- **Temperature 0 is not deterministic.** A thousand completions of one prompt
  at temperature 0 produced **80 unique outputs**, all identical for the first
  102 tokens and then diverging. The cause is that kernel output depends on
  the server's batch size, which varies with load you do not control.[^91]
- **The serving stack moves your scores.** Backend, GPU type, GPU count and
  quantisation shift benchmark results by up to 9% under greedy
  decoding.[^92]
- **Prompt format is usually a bigger effect than the thing you are
  testing.** The same model scored 50.1% and 72.4% on the same benchmark under
  two standard prompt formats — a 22.3-point swing that also reorders model
  rankings.[^93]

So freeze the model *identifier*, the inference backend, the hardware class,
the decoding parameters, the reasoning-effort setting, and the prompt template
byte for byte. Then record all of it, because you will not reconstruct it
later.

### One run tells you nothing

τ-bench reports `pass^k` — success on all k independent attempts. GPT-4o
scored under 50% at one attempt and **under 25% at eight**.[^33] Almost every
other benchmark reports a single-run mean.

At any temperature above zero, a single comparison between two configurations
is mostly noise. Run each several times, report a spread, and be suspicious of
differences smaller than the run-to-run variance. The multi-turn study's
decomposition is the reason this matters: the damage from long conversations
was 16% lost capability and **112% increased unreliability**.[^23] A system
that is usually right and occasionally catastrophic scores well on a mean.

### A strong published context ablation found nothing

A context ablation with unusually careful controls returned a null result.

A study tested whether context files (`AGENTS.md`-style standing instructions)
help coding agents. The design covered three Python repositories, 15–17 real
merged pull requests each, gold tests as the outcome, two different agents,
three context strategies, three repeats, **291 runs**. The statistics were the
ones this chapter recommends: within-task permutation tests at 10,000+
iterations, equivalence
testing with a task-clustered bootstrap, paired tests with multiple-comparison
correction, and a Monte Carlo power simulation.[^89]

**No correctness benefit.** One agent went 53.3% to 55.6% (p=1.00); the other
went 58.8% / 56.9% / 52.9% (p=0.66).

**It ran a manipulation-validity probe.** Thirty-six probe cells on near-miss
tasks confirmed the context never converted a near-miss into a pass. Without
that, a null result cannot distinguish "the change does not help" from "the
change never fired". Failure analysis then showed the agents were stumbling on
implementation skill, not missing repository knowledge — so the context
supplied knowledge the agents did not lack.

**Its power analysis is the transferable part.** At 17 tasks × 3 repeats, the
minimum detectable effect stayed above 30 percentage points. Detecting a
10-point effect needs 120–200 tasks. Most published context comparisons are
smaller than this one.

**Task difficulty did not transfer between agents** (rank correlation 0.75,
with about 40% of tasks at floor or ceiling for one agent but not the other).
That alone may explain why published results on the same question contradict
each other.

A well-powered null result is more useful than an underpowered positive one.
If your evaluation cannot produce the first, it cannot be trusted to produce
the second.

### Measure whether the evidence was there at all

Answer accuracy conflates two failures: the evidence was missing, and the
model mishandled evidence it had. Separate them.

The clearest published version, from Google, classifies each (question,
context) pair by whether it contains enough to answer definitively,
independent of what the model then did. Their automated rater reaches **93%
accuracy** on that judgement — Google's measurement of Google's rater, and
Google ships retrieval products.[^94] Two findings follow from it:

- Models answer correctly **35–62% of the time even when the context is
  insufficient** — so retrieval improvements alone cannot account for
  end-to-end quality.
- With insufficient context, models hallucinate rather than abstain. One
  model's hallucination rate went from 10.2% with *no* context to **66.1%
  with insufficient context**. Partial evidence is more dangerous than none.

That second finding is the strongest case for admission control anywhere in
this guide. Retrieving something-but-not-enough is measurably worse than
retrieving nothing.

### Do not trust a single benchmark

HELMET found that no synthetic long-context task correlates above 0.8 with
downstream performance, and that its own seven categories correlate poorly
with each other.[^29] A ranking from one benchmark does not transfer.

For your own work the implication is: pick the evaluation that resembles your
job, and check that it correlates with the outcome you actually care about. If
you never measure that correlation, you have built a proxy and started
optimising it.

### The judge is a component, not an oracle

Much of this literature is scored by a model. Three cautions.

The memory dispute in chapter 10 is the cleanest demonstration: three
different numbers — 84%, 75.14%, 58.44% — for the same system on the same
benchmark, depending on who configured the harness. That spread is larger than
any effect either party claimed. Harness configuration dominated system
quality.

Grounding scores can be wrong in a specific way: a system can
score as faithful because it grounded on the wrong
document. Scoring whether the answer follows from the retrieved evidence does
not check whether the retrieved evidence was the right evidence.

And the automatic metrics have a known ceiling. On the aggregate
fact-verification leaderboard, the best system reaches about 77% balanced
accuracy; on long-form expert answers, **every** system sits near 59%.[^82]
These are useful instruments, not oracles.

### Automatic metrics are validated for comparing systems, not judging answers

This distinction decides whether a metric is usable, and it is routinely ignored.

The standard automatic attribution metric reports a **system-level correlation
of 0.96 with human judgement, and instance-level agreement its own authors
describe as much lower and more variable** — they warn explicitly against
per-example use.[^81] The same pattern holds for automatic coverage scoring:
strong run-level rank correlation, and per-topic correlation around 0.49.

**An A/B test of a context change is a system-level comparison.** You
are ranking two configurations over a suite, which is what these metrics were
validated for. Use them freely there.
Do not use them to gate an individual output in production; that is a
different claim than the validation supports.

### Entailment scoring can be cheap enough to run on everything

A fine-tuned entailment checker scored 13,000 claim-document pairs for
**$0.24**; the frontier model it matched cost **$107** for the same set, at
comparable balanced accuracy.[^82]

For this kind of claim-document entailment scoring, that changes the
experimental design: score every response in both arms rather than sampling.
Other metrics may have different costs.

### Faithfulness alone rewards evasion

The failure you get if you measure only one thing.

On Vectara's hallucination leaderboard — Vectara sells the detector that does
the scoring — two models post low hallucination rates with **answer rates of
80.7% and 62.7%** — they achieve faithfulness partly by
declining to answer.[^83] A system that says less is scored as more truthful.

So run **two** metrics, always: faithfulness (is every claim supported by what
was retrieved?) and coverage (did the answer contain the facts it needed?). A
context change that makes the model terser will improve the first and damage
the second, and only the pair tells you which happened.

### Citations resolve far better than they support

Across 14 models,
inline citations in research reports were scored on three things
separately:[^84]

| What was checked | Result |
|---|---|
| The link resolves | **over 94%** for frontier models |
| The linked page is relevant | **over 80%** |
| The linked page actually supports the claim | **39–77%** |

Most citation dashboards measure the first two. They run 20–50 points
optimistic against real support.

Also: **support accuracy fell by about 42% on average as
tool calls scaled from 2 to 150.** A context change that enables more
retrieval can improve coverage while making attribution worse. If you measure
only coverage, that reads as a win.

A related trade-off, from a deep-research benchmark: one system scored 90.2%
citation accuracy with about 31 effective citations, another 81.4% with about
111.[^85] Precision per citation and number of citations trade against
each other. Reporting one without the other makes any system look better than
it is.

### A bias that penalises synthesis

An evaluation of five factuality
metrics across 11 datasets found they disagree with one another and misestimate
system-level performance, with two named biases: **against heavily paraphrased
output, and against output that draws on distant parts of the source.**[^86]

Read that against chapters 4 to 7. A context change that lets the model
synthesise across a whole document instead of copying locally is the
kind of change that will score *worse* on these metrics, for reasons unrelated
to whether it is true.

If you adopt an automatic factuality metric, calibrate it on a small
human-annotated sample of *your own* data before you let it decide anything.
Every paper here that measured cross-domain transfer found it poor.

### Prose detectors do not transfer to tool output

If your agent's context is code, tool results or structured documents rather
than prose, off-the-shelf grounding detectors are close to useless: one
reported **0.17 span-level F1** on code-agent sources against **0.689** for a
model fine-tuned on that material.[^87]

Measuring agentic grounding with a detector trained on news summarisation
measures noise.

### Metrics, and what each one misses

| Metric | Measures | Misses |
|---|---|---|
| Recall@k | Did the right document appear in the k you fetched? | Whether the model then used it. **Granularity matters more than the metric** — see below |
| Precision@k | What share of what you fetched was useful? | Whether the useful ones were enough |
| nDCG@10 | Is the ordering good? | Cost; and it assumes value decays with rank, which the U-curve contradicts |
| **Sufficient context** | Could *anyone* answer from this context alone? | Whether the answer was right. The best coverage metric — separates retrieval failure from generation failure |
| **Retrieval failure rate at k** | How often is the answer absent from what the model sees? | Nothing much — prefer this one |
| Faithfulness / groundedness | Does the answer follow from the retrieved text? | Whether the retrieved text was right, or current. Rewards saying less |
| Claim recall | Did every fact needed to answer survive? | Whether the answer was well-formed |
| Citation support | Can each claim be traced to a span? | Whether the span still says that. Decompose into resolves / relevant / supports |
| Answer accuracy | Was it right? | Everything about how and at what cost |

One warning that outranks the choice of metric. In an applied study, **word-level
recall correlated 0.35 with human answer scores while document-level recall
correlated 0.05** — the same metric name, a sevenfold difference in
usefulness.[^95] Decide your granularity deliberately and state it.

And validate any metric on **at least two systems** before trusting it. A
metric that correlates well on one system may be measuring question
difficulty rather than system quality — in which case it scores every system
the same and is useless for the comparison you actually want.[^95]

### Measure cost, because most leaderboards omit it

Terminal-Bench is the only mainstream leaderboard publishing dollars, and it
found two systems tied on accuracy at a 3.7× cost difference.[^39] Every
long-context benchmark reports accuracy at unbounded spend.

Report **cost per correct answer**, not accuracy. And report latency, which no
long-context benchmark reports at all.

### Contamination is not auditable

Models identify buggy files from benchmark issue text at 76% inside the
benchmark and 53% outside it.[^35] And detection methods fail: only 201 of 335
contamination evaluations produced correct outcomes across 25 models.[^36]

You cannot audit your way out. The defences are held-out sets you built
yourself, data you can prove is post-cutoff, and self-refreshing evaluations.

### A checklist

1. **Write down the decision rule before you look at any data** — the minimum
   effect you would act on, the metric, and the power you need.
2. **Size the suite from that effect, not from convenience.** ~1,000 items for
   3 points; ~120–200 agent tasks for 10 points. If you cannot reach it, say
   so and report an equivalence bound instead of claiming "no difference".
3. **Keep tasks that pass or fail 100% of the time in headline performance.**
   They add little information to a within-model comparison, so a predeclared
   comparative analysis may report them separately; do not drop them after
   seeing the result.
4. **Run a manipulation-validity probe.** Confirm on a few near-misses that
   the change *can* flip an outcome. Otherwise a null result tells you nothing.
5. Freeze the model, inference backend, hardware class, decoding settings,
   reasoning effort, evaluator version, prompt template and time window.
   Record all of them.
6. Include a do-nothing baseline and a simplest-thing baseline.
7. Vary exactly one component per comparison.
8. Interleave the arms rather than running one after the other, so load and
   model updates do not correlate with the treatment.
9. Run each item 4–6 times; beyond that the variance reduction has saturated.
   Do not lower the temperature to reduce the noise.
10. **Analyse paired per-item differences, not the two headline means**, and
    **cluster the standard errors** on whatever the shared source is.
11. Report `pass^k` if anything user-facing depends on consistency.
12. Score **two** things — faithfulness and coverage — never one. Scoring
    faithfulness alone rewards a change that makes the model evasive.
13. Score every response rather than sampling. It costs cents, and sampling is
    where your variance comes from.
14. Score citation *support* separately from link resolution and relevance.
    They differ by 20–50 points.
15. Calibrate any automatic factuality metric against a human-annotated sample
    of your own data. Size that sample from class prevalence and the uncertainty
    your decision can tolerate; report sensitivity, specificity and their
    confidence intervals rather than applying a universal accuracy threshold.
16. Record cost and latency alongside quality, per configuration.
17. Record what was *missing* — the gap — not only what scored.
18. Record retries, refusals and failures rather than discarding them.
19. Keep raw provider responses so the run can be replayed.
20. Check your evaluator against the real outcome on a holdout set, and record
    the correlation. Validate any new metric on at least two systems.
21. Assume public benchmarks are contaminated; keep a private set, and treat
    the holdout as single-use.
22. Publish the configuration of the systems you lost to, not only your own.
23. When you ship, confirm online with a test that stays valid under repeated
    checking. Naive peeking at an accumulating dashboard produced a **70%
    false-positive rate** on data with no real effect.[^96]



## 12. What nobody knows yet

These are the gaps I hit while writing — places where the evidence runs out,
or where what circulates as knowledge has no source.

### Claims that are repeated and unsupported

**"Adding random documents improves RAG accuracy."** Refuted in July 2026. The
original effect was largely an artefact of a 15-token generation limit; most
of it traced to truncated and malformed outputs rather than any benefit from
noise.[^27] It is widely repeated.

**"Effective context is 60–70% of advertised."** No traceable source. The
attributable figures are lower and narrower — see chapter 2.

**"Code is about twice as token-dense as prose."** Not supported by the nine
files measured in chapter 1, where it was 20–40% denser per character. Small
sample; measure your own.

**"A web page is about 80% boilerplate."** Appears only in blog posts with no
primary measurement, and refers to text rather than tokens. Chapter 1 measures
87–99% of *tokens* across four pages — a different and more useful quantity,
on too small a sample to settle the original claim.

**"Context poisoning."** The term is useful and the mechanism is plausible.
The evidence is one anecdote about an agent playing Pokémon, which does not
appear in any current version of the technical report it is attributed to. As
of August 2026 I could find no controlled experiment that injects a false fact
into an agent's context and measures how far it propagates.

**Most published tool-count thresholds.** "Anthropic documents degradation past
30–50 tools", "fewer than 20 per turn", "5–7 MCP servers is the ceiling" —
these circulate widely with no primary citation I could resolve. The
defensible numbers are LongFuncEval's 7–85% range and one production
catalogue's elbow at 40–60 agents, whose authors explicitly warn the figures
are catalogue-specific.

### Things you will have to measure yourself

The published benchmarks do not cover these, so if they matter to your system,
budget for measuring them.

**Source selection quality.** Benchmarks score the final answer. None scores
whether the agent chose an authoritative source over a content farm, a primary
document over a paraphrase, or a current page over a cached one. For
acquisition work this is the metric that matters most.

**Attribution fidelity.** Whether a claim traces to a specific span of a
specific document, with that span still present at that URL. Chapter 11 has
the tooling; no benchmark applies it end to end.

**Staleness.** Whether a system knows its evidence has gone out of date, as
opposed to whether the evidence happens to be current.

**Abstention.** Benchmarks reward answering. A system that correctly says "I
could not find a reliable source" scores the same as one that confabulates.

**Retrieved web content as untrusted input.** If you retrieve from the open
web, that text is adversarial by default, and retrieval benchmarks do not
treat it that way.

**Contradiction between sources.** Chapter 4's measurement shows three
providers returning largely different hosts for the same query. What happens
when two of them disagree is a decision your system makes and nothing scores.

### Things that might be true and need checking

**Does sophisticated acquisition have a scale ceiling?** One paper reports
that above roughly 10M corpus tokens, plain keyword search beats an agentic
searcher by a margin approaching 20 points while the agent spends 39× more
query tokens.[^41] One team, one construction, days old. If it replicates it
matters a great deal.

**Does the confusion residue generalise?** The finding that about 10 points of
tool-selection degradation survives a perfect shortlister is from one
catalogue. If it generalises, there is a ceiling on what any selection system
can achieve, and part of the remedy belongs to whoever writes the supply's
descriptions.

**Do degradation curves transfer across models?** Databricks found five models
improving to 100K and two degrading after 32K, failing in different ways.
Nothing suggests a policy fitted on one transfers to another.

**Is compaction fidelity measurable in general?** The 3-of-3-versus-0-of-3
result is one run over eight documents. It matches intuition, which is a
reason to be suspicious of it rather than reassured.

### How to read anything in this area

Six habits that would have caught most of the errors above:

1. **Check what the number was measured on.** "20× compression" measured on
   few-shot reasoning prompts is not a claim about your web pages.
2. **Check the date and the model.** A 2023 result about a 4,000-token model
   is not a property of a 2026 million-token one.
3. **Check whether the baseline was included.** On the main memory benchmark,
   the missing full-context baseline explains the result.
4. **Check who ran it.** Vendor evaluations of competitors are usually
   misconfigured, in both directions.
5. **Check whether the benchmark still discriminates.** A test everyone passes
   measures nothing.
6. **Prefer the follow-up paper to the headline.** When the same team's next
   paper claims a quarter of the previous one, believe the quarter.

---

## Glossary

Terms are defined in plain words, with no jargon inside the definitions.

<span id="g-attention"></span>**Attention** — the mechanism by which a model
decides, for each word it produces, how much to draw on each earlier word.

**Balanced accuracy** — the average of the success rate on positive examples
and the success rate on negative examples, so a large class cannot dominate.

**Bootstrap** — estimating uncertainty by repeatedly resampling the observed
items and recalculating the result.

<span id="g-compaction"></span>**Compaction** — replacing a long conversation
with a written summary of it and carrying on from there.

<span id="g-context-window"></span>**Context window** — the bounded working
material a model can hold at once, counting what you send and what it writes
back in that turn. Encoded images and other supported inputs can count too.

**Context rot** — the tendency for a model to get less reliable as you give it
more input, even when the extra input is harmless and the task stays easy.

<span id="g-cross-encoder"></span>**Cross-encoder** — a model that reads the
question and one candidate passage together and scores how well they match.
Slow and accurate, so it is used to re-order a shortlist rather than to
search.

**Dense retrieval** — finding documents by meaning: both question and passage
become lists of numbers, and closeness between the lists stands in for
closeness in meaning.

**Distractor** — a passage that looks relevant but is not, and which makes the
answer worse.

**Effective context length** — the length up to which a model still does the
job properly, as opposed to the length it will accept without erroring.

**Equivalence test** — a test of whether a difference is small enough to count
as practically equivalent, rather than merely failing to find a difference.

<span id="g-embedding"></span>**Embedding** — a list of numbers representing a
piece of text, arranged so that similar texts get similar numbers.

<span id="g-extraction"></span>**Extraction** — pulling the real content out of
a web page and throwing away the navigation, footers and adverts.

**Hard negative** — a distractor that is especially convincing because it
closely resembles what was asked for.

**Hybrid search** — running word-matching and meaning-matching lookups at the
same time and combining the two result lists.

**F1** — the harmonic mean of precision and recall. It is high only when both
are high.

**Facility location** — a selection objective that rewards a chosen set for
representing the whole candidate pool.

<span id="g-json-schema"></span>**JSON Schema** — the machine-readable
description of what arguments a tool accepts. Usually the bulk of a tool
definition's size.

<span id="g-kv-cache"></span>**KV cache** — the intermediate results a model
works out while reading your input, kept in memory so it does not recompute
them for every word it writes.

**Late chunking** — feeding a whole document through the meaning-encoder
first, then cutting the result into pieces, so each piece carries traces of
its surroundings.

**Lost in the middle** — the pattern where a model uses material at the start
and end of a long input well and largely misses what sits between them.

**MCP (Model Context Protocol)** — a standard way of plugging external tools
into an agent. Convenient, and expensive in window space if left unmanaged.

**MMR (maximal marginal relevance)** — a selection rule that balances
relevance to the query against similarity to items already selected.

**nDCG at k** — a ranking score that gives more credit when relevant results
appear near the top of the first k positions.

**Oracle** — an idealised condition supplied with information a real system
would have to discover, used to show the best possible result for that stage.

<span id="g-niah"></span>**Needle in a haystack** — a test where one specific
fact is hidden inside a large body of unrelated text and the model is asked to
find it.

<span id="g-parent-document"></span>**Parent-document retrieval** — matching on
small pieces because they match precisely, then handing the model the larger
section each small piece came from.

**pass^k** — the fraction of tasks a system gets right on every one of k
separate attempts, rather than on its best attempt.

**Prefill** — the first phase of answering, where the model reads your whole
input before writing anything. Its cost grows with how much you sent.

**Precision** — the share of selected material that is relevant. High
precision does not show that all relevant material was found.

**Permutation test** — a comparison that repeatedly shuffles which result is
labelled as belonging to each system to estimate how surprising the observed
difference is.

**Prefix** — the content at the very start of a request. Prompt caching reuses
it only when the provider recognises an exact matching prefix.

**Progressive disclosure** — keeping a short pointer in the window and
fetching the full text only when it turns out to be relevant.

**Prompt caching** — storing the unchanging front portion of a request on the
server so repeat requests are cheaper. It does not make that portion smaller.

**Quadratic cost** — in naive dense attention, the property that doubling the
input roughly quadruples the comparison work.

**Submodular objective** — a score with diminishing returns: adding an item
helps less when similar useful items have already been selected.

**Sensitivity and specificity** — respectively, the share of true positive
cases and true negative cases that a test identifies correctly.

**TTL (time to live)** — how long a cached entry remains eligible for reuse.

**RAG (retrieval-augmented generation)** — fetching relevant material and
putting it in the prompt before asking the model to answer.

<span id="g-recall"></span>**Recall at k** — of everything in your collection
that would have helped, the share that appeared in the k things you handed the
model.

**Reranking** — taking a rough shortlist and re-ordering it with a slower,
better model before deciding what to keep.

**Reasoning tokens** — text the model writes to work a problem out before
answering. You are billed for it as output even when you never see it.

**Sub-agent** — a second instance of the model, given its own fresh window,
that does a job and reports back a short answer.

<span id="g-token"></span>**Token** — a chunk of text the model treats as one
unit: a short word, part of a long one, or a piece of punctuation. Roughly
four characters of English.

<span id="g-tokeniser"></span>**Tokeniser** — the software that chops text into
tokens before the model sees it. Different vendors use different ones, so the
same sentence has different token counts on different services.

<span id="g-u-shaped-curve"></span>**U-shaped curve** — a graph of accuracy
against the position of the important information: high at both ends, low in
between.

---

## Appendix: one production system, end to end

The techniques in chapters 4 to 10 are discussed one at a time. This is what
they look like assembled, in a system that runs.

Cerebras published the design of their internal knowledge base in July
2026.[^115] It answers about 15,000 questions a day across data-centre
operations, chip design, hardware, training, inference and cloud, three months
after launch, and is queried by people, automations and agents. Individual
techniques from it appear in the chapters where they belong; the value of
seeing it whole is the ordering.

```
sources          Slack, wiki, code, incidents, team databases
   |             one connector per source, one row shape
distillation     LLM rewrites each item into a canonical record       -> ch 5
   |             gated on term rarity, length, reactions              -> ch 5
index            a single embeddings table, any source queryable
   |
scope            default project excludes most of the corpus          -> ch 7
   |
retrieve         six lists in parallel: lexical, semantic, per-source -> ch 4
   |
fuse             reciprocal rank fusion, k=60, consensus beats a vote -> ch 4
   |
rerank           small model scores the merged candidates, keep 10    -> ch 6
   |
expand           pull neighbouring sections back into the winners     -> ch 6
   |
synthesise       answer with citations
```

**Gating happens before indexing and scoping before retrieval** — the two cheapest filters run
earliest, so everything downstream works on less. And **expansion happens
last**, after ranking, so the ranker never scores padded chunks.

The same primitives serve two orchestration strategies: agents call the tools
directly and decide the sequence themselves, while the web interface runs a
fixed plan-execute-synthesise pipeline over the identical layer (chapter 10).

### What it cannot tell you

No retrieval quality figures. No comparison against a simpler configuration.
No evaluation methodology.

15,000 questions a day is an adoption number. It tells you people find it
useful enough to keep using; it does not tell you whether the distillation
step, the gating, the six-way fan-out or the LLM reranker each adds
measurable value, or whether hybrid search with a cross-encoder would reach
the same result for a fraction of the complexity.

The write-up is more candid than most, but it illustrates chapter 11's
point: a system can be carefully designed,
widely adopted, and still leave you unable to say which parts are doing the
work.

## Appendix: tooling, checked 4 August 2026

Every status below was checked against the GitHub and package-registry APIs on
4 August 2026. Tooling ages badly, so treat this as a snapshot and rerun the
checks rather than trusting the table.

### How to check whether a project is alive

`pushed_at` is unreliable. It updates when someone edits a README or a bot
bumps a dependency. The reliable signal is commit participation — 52 weekly commit
counts, via `repos/OWNER/REPO/stats/participation`.

The clearest illustration, from this survey:

| Project | Commits, 52 weeks | Last 13 weeks | Last 4 weeks |
|---|---|---|---|
| cognee | 6,252 | 1,665 | 383 |
| mem0 | 837 | 384 | 103 |
| graphiti | 394 | 110 | 51 |
| langmem | 52 | 24 | 7 |
| **letta** | **2,542** | **6** | **2** |

Letta has a high annual figure, 24,000 stars and a recent push date, and its
development stopped about thirteen weeks ago. Its README describes the
repository as the "legacy Letta server" and points users elsewhere; the
organisation's active repositories are all a TypeScript coding agent. Anyone
running `pip install letta` today installs 247 transitive packages of software
its own maintainers label legacy.

Check participation, read the README, and look at what else the organisation
is pushing.

### Picks by stage

| Stage | Use | Avoid, and why |
|---|---|---|
| **Extraction** | **trafilatura** (Apache-2.0, F1 0.958, precision 0.938) | **MarkItDown** — 171k stars, but it is a *converter*: on HTML it removes no boilerplate. **Readability** — recall 0.729 in one benchmark run, so it drops about a quarter of the body text with no indication. **html2text** — GPL-3.0, F1 0.662, effectively dormant |
| **Chunking** | Recursive splitter at 200–400 tokens; **Chonkie** if you want late chunking off the shelf; **semchunk** for token-exact splits | **semantic-chunkers** — no commit or release since June 2025. **jina-ai/late-chunking** — reference implementation, untouched since December 2024; the technique lives on as an embeddings API flag |
| **Compression** | **LLMLingua** (MIT) if you have proved you need it | Not dead, but one commit since October 2025. Verify the ratio on your own text before believing the headline |
| **Memory** | **Graphiti** (lightest serious option, 27 transitive packages); **Mem0** (most popular, 33); **Cognee** (most active, but 129 packages and 26 MB) | **Letta** — declared legacy by its maintainers, 247 packages. **LangMem** — dependabot commits only since late 2025 |
| **Retrieval frameworks** | **Haystack 3.0**, **LangChain** (light: 3 direct dependencies, 36 transitive), **LlamaIndex**, **txtai**, **DSPy** | **R2R** — no commit since November 2025 despite ~8,000 stars. **Verba** and **Cognita** — both formally archived |
| **Serving** | **vLLM** (prefix caching on by default — the prose docs describe it as opt-in, so trust the source), **SGLang** (RadixAttention, configurable eviction) | **HuggingFace TGI** — archived, and its own README recommends vLLM, SGLang or llama.cpp |
| **Evaluation** | **Inspect AI** (UK AISI) — the only framework shipping clustered standard errors, bootstrap intervals and epoch reducers, so it implements chapter 11's statistics rather than leaving them to you; **promptfoo** for the fastest declarative before/after with CI gating; **DeepEval** for the metric catalogue; **Langfuse** or **Phoenix** as the trace substrate | **ARES** — no commit since March 2025, despite having the most interesting confidence-interval method of the set. **OpenAI Evals** — last release May 2024, README redirects to a hosted product. **Deepchecks** — last LLM release December 2024. **RAGAS** — the standard vocabulary, but the repo has moved orgs and has not shipped since early 2026; fine to cite, risky on a critical path |

Two licence warnings, because the GitHub badge is unreliable in both
directions. Several projects that look permissive are not: Morphik is BSL 1.1,
`context-mode` is Elastic License 2.0, Dify and holaOS use modified Apache-2.0
with multi-tenancy bans, and OpenViking and chunkr are AGPL-3.0. Conversely,
GitHub's `NOASSERTION` flag concealed plain Apache-2.0 or MIT licences on
several others. Read the LICENSE file.

A note on star counts: several repositories created in 2026 show growth curves
that are hard to credit, including one claiming more stars than LangChain. A
fork-to-star ratio well below what comparable projects show is a useful
check.

---

## About this guide

The authors build software that decides which content an AI agent may take
in and keeps a record of what it took in, including a plugin for Anthropic's
Claude Code. Anthropic accounts for about one citation in six; where an
Anthropic post evaluates an Anthropic technique, the text marks it as a
vendor evaluating itself. Anthropic's API documentation is treated as
authoritative for the behaviour of its own products.

Two measurements are the authors' own, based on small samples. They are
labelled with their sample sizes where they appear.

## Sources

Every URL cited below returned HTTP 200 on 4 August 2026. That confirms the
page exists, not that it says what is claimed; every figure quoted in the
text was checked against the source rather than an automated summary.
Several vendor claims are internal evaluations with unpublished
composition, marked as such in the text.

Two measurements used in chapters 1 and 5 are original: token density by
content type, and the raw-HTML-versus-tag-stripped-text token ratio. Both
used tiktoken `o200k_base` over files in this repository plus four live web
pages. The repository files can be rerun. The four original HTML inputs
were not retained, so their published figures cannot be reproduced exactly.
Neither measurement transfers exactly to Claude or Gemini tokenisers.

### References

[^1]: Anthropic, *Context windows*, `platform.claude.com`, fetched 4 August
    2026.
[^2]: Vaswani et al., *Attention Is All You Need*, arXiv:1706.03762, June
    2017, Table 1.
[^3]: Anthropic, *Token counting*, fetched 4 August 2026.
[^4]: Anthropic `claude-api` reference material, bundled 2026.
[^6]: Anthropic, *Advanced tool use*, 24 November 2025.
[^7]: `github.com/zhang-liz/mcp-token-benchmark`, 8 July 2026.
[^8]: JD Hodges, published `/context` breakdown, 22 March 2026.
[^9]: Anthropic, *Agent Skills overview*, fetched 4 August 2026.
[^10]: Aurimas Griciūnas, *Agent Skills progressive disclosure*, 11 March
    2026.
[^11]: Liu et al., *Lost in the Middle*, TACL, February 2024.
[^12]: Zhang et al., *Positional Failures in Long-Context LLMs*,
    arXiv:2605.23170, 22 May 2026.
[^13]: Hong, Troynikov and Huber, *Context Rot*, Chroma, 14 July 2025.
[^14]: Levy et al., *Same Task, More Tokens*, arXiv:2402.14848, ACL 2024.
[^15]: Hsieh et al., *RULER*, arXiv:2404.06654, April 2024.
[^16]: Modarressi et al., *NoLiMa*, arXiv:2502.05167, February 2025.
[^17]: An et al., *Why Does the Effective Context Length of LLMs Fall
    Short?*, arXiv:2410.18745, October 2024.
[^18]: Epoch AI, *Context windows*, 25 June 2025.
[^19]: Leng et al., *Long Context RAG Performance of LLMs*,
    arXiv:2411.03538, November 2024.
[^20]: Kate et al., *LongFuncEval*, arXiv:2505.10570, April 2025.
[^21]: Gillespie and Perry, *Scaling Enterprise Agent Routing*,
    arXiv:2606.17519, 16 June 2026.
[^22]: Eliav, *Prompt Design at Scale*, arXiv:2607.19257, 21 July 2026.
    Single-author preprint.
[^23]: Laban et al., *LLMs Get Lost In Multi-Turn Conversation*,
    arXiv:2505.06120, May 2025.
[^24]: Lodha et al., *Less Context, Better Agents*, arXiv:2606.10209, 8 June
    2026.
[^25]: Drew Breunig, *How Long Contexts Fail*, 22 June 2025.
[^26]: Cuconasu et al., *The Power of Noise*, arXiv:2401.14887, SIGIR 2024.
[^27]: Mazuryk et al., *The Powerless Noise*, arXiv:2607.03615, SIGIR 2026.
[^28]: Amiraz et al., *The Distracting Effect*, arXiv:2505.06914, 2025.
[^29]: Yen, Gao, Chen et al., *HELMET*, arXiv:2410.02694, ICLR 2025. 59 models,
    seven categories. <https://princeton-nlp.github.io/HELMET/>
[^30]: Greg Kamradt, *LLMTest_NeedleInAHaystack*, MIT.
    <https://github.com/gkamradt/LLMTest_NeedleInAHaystack>
[^31]: OpenAI, GPT-4.1 launch material, 14 April 2025. MRCR 57.2% at 128K,
    46.3% at 1M; Graphwalks BFS 61.7% under 128K falling to 19.0% above.
[^32]: LoCoDiff, Mentat AI / AbanteAI, 8 May 2025.
    <https://abanteai.github.io/LoCoDiff-bench/>
[^33]: Yao et al., *τ-bench*, arXiv:2406.12045, June 2024; and *τ²-bench*,
    arXiv:2506.07982, June 2025.
[^34]: Du, Tian, Peng et al., *Context Length Alone Hurts LLM Performance
    Despite Perfect Retrieval*, arXiv:2510.05381, EMNLP 2025 Findings.
[^35]: *The SWE-Bench Illusion*, arXiv:2506.12286, June 2025, final December
    2025.
[^36]: Zarzecki, Dubiński and Cygert, *The Reliability Gap in Benchmark
    Auditing*, arXiv:2606.03305, 2 June 2026.
[^37]: Wang et al., *EvoBrowseComp*, arXiv:2606.13120, 11 June 2026.
[^38]: Google, Gemini 3 launch material, 18 November 2025.
[^39]: Terminal-Bench 2.1 leaderboard, fetched 4 August 2026.
    <https://www.tbench.ai/leaderboard/terminal-bench/2.1>
[^40]: Arya, *RECON*, arXiv:2607.16716, 18 July 2026.
[^41]: Wang, Xu et al., *BM25 Wins at Scale*, arXiv:2607.26497, 29 July 2026.
    One team, one corpus construction; unreplicated.
[^42]: Li, Li, Zhang, Mei and Bendersky, *RAG or Long-Context LLMs?*,
    arXiv:2407.16833, EMNLP 2024 industry track. Source of the Self-Route
    results and the four-way failure taxonomy.
[^43]: *LaRA*, arXiv:2502.09977, ICML 2025. 2,326 test cases, 11 models.
[^44]: Anthropic, *Introducing Contextual Retrieval*, 19 September 2024.
[^45]: Industry RAG-Fusion deployment study, arXiv:2603.02153, 2 March 2026.
[^46]: *Fishing for Answers*, arXiv:2509.04820, September 2025.
[^47]: Jin et al., *Search-R1*, arXiv:2503.09516, March 2025.
[^48]: Edge et al., *GraphRAG*, arXiv:2404.16130, April 2024. Preference-based
    evaluation; faithfulness at parity with baseline retrieval.
[^49]: Microsoft Research, *LazyGraphRAG*, 25 November 2024.
[^50]: Anthropic, *Web fetch tool*, fetched 4 August 2026.
[^51]: Provider pricing pages for Anthropic, Brave, Exa, Firecrawl and Jina,
    all fetched 4 August 2026.
[^52]: Cloudflare, *Introducing pay-per-crawl*, 1 July 2025, modified 15 July
    2026.
[^53]: Zyte, `scrapinghub/article-extraction-benchmark`. 181 pages, four-gram
    shingle scoring; expanded to 28 extractors 1 March 2026, archived 24 June
    2026.
[^54]: Li et al., *Beyond a Single Extractor*, arXiv:2602.19548, 23 February
    2026.
[^55]: Smith and Troynikov, *Evaluating Chunking Strategies for Retrieval*,
    Chroma, 3 July 2024.
[^56]: Chunking taxonomy paper, arXiv:2602.16974, SIGIR 2026, 19 February
    2026.
[^57]: Günther, Mohr, Williams, Wang and Xiao, *Late Chunking*,
    arXiv:2409.04701, September 2024, revised July 2025.
[^58]: *Reconstructing Context*, arXiv:2504.19754, 28 April 2025.
[^59]: Jiang et al., *LLMLingua*, arXiv:2310.05736, 9 October 2023. "Up to 20x
    compression with little performance loss", measured on GSM8K, BBH,
    ShareGPT and an arXiv set.
[^60]: Pan et al., *LLMLingua-2*, arXiv:2403.12968, 19 March 2024. Claims
    2×–5×.
[^61]: Jiang et al., *LongLLMLingua*, arXiv:2310.06839, ACL 2024.
[^62]: Xu, Shi and Choi, *RECOMP*, arXiv:2310.04408, 6 October 2023.
[^63]: Isabella He, Anthropic cookbook, context-engineering tools, 20 March
    2026. Compaction fidelity: 3 of 3 high-level facts kept, 0 of 3
    appendix-table details.
[^64]: Anthropic, *Prompt caching*, fetched 4 August 2026.
[^65]: Anthropic, *Cache diagnostics*, beta `cache-diagnosis-2026-04-07`.
[^66]: Xiao et al., *StreamingLLM*, arXiv:2309.17453; Zhang et al., *H2O*,
    arXiv:2306.14048; Li et al., *SnapKV*, arXiv:2404.14469. Headline
    multipliers are against 2023-era baselines.
[^67]: LangChain, *Context engineering for agents*, 2 July 2025. Its
    "auto-compact at 95%" figure is stale.
[^68]: Anthropic, *Compaction*, fetched 4 August 2026. Default trigger 150,000
    input tokens; raw history permanently discarded.
[^69]: Anthropic, *Effective harnesses for long-running agents*, 26 November
    2025.
[^70]: Anthropic, *Effective context engineering for AI agents*, 29 September
    2025.
[^71]: Anthropic, *How we built our multi-agent research system*, 13 June
    2025.
[^72]: Walden Yan, Cognition, *Don't Build Multi-Agents*, 12 June 2025.
[^73]: Walden Yan, Cognition, *Multi-Agents: What's Actually Working*, 22
    April 2026.
[^74]: Tran and Kiela, *Single-Agent LLMs Outperform Multi-Agent Systems Under
    Equal Thinking Token Budgets*, arXiv:2604.02460, 2 April 2026.
[^75]: Cemri et al., *Why Do Multi-Agent LLM Systems Fail?*, arXiv:2503.13657,
    NeurIPS 2025. 1,600+ annotated traces, 14 failure modes.
[^76]: Chhikara et al., *Mem0*, arXiv:2504.19413, April 2025 — the
    full-context baseline figure is in the paper's own results table. Critique:
    Zep, *Is Mem0 Really SOTA in Agent Memory?*, 6 May 2025. Zep is an
    interested party.
[^77]: Letta, *Benchmarking AI agent memory*, 12 August 2025. 74.0% on LoCoMo
    from a plain filesystem.
[^78]: `getzep/zep-papers` issue #5, Mem0 reply, 8 May 2025.
[^79]: Kuan Wang, *MemDelta: Controlled Baselines and Hidden Confounds in Agent
    Memory Evaluation*, arXiv:2606.29914, 29 June 2026.
[^81]: Bohnet et al., *Attributed Question Answering* (AutoAIS),
    arXiv:2212.08037, December 2022. System-level Pearson r = 0.96 against
    human attribution judgement; instance-level agreement explicitly described
    as much lower and more variable. The underlying human standard is Rashkin
    et al., *Measuring Attribution in Natural Language Generation Models*,
    arXiv:2112.12870, *Computational Linguistics* 49(4), 2023.
[^82]: Tang et al., *MiniCheck*, arXiv:2404.10774, EMNLP 2024, and the
    LLM-AggreFact leaderboard. Cost to score ~13,000 pairs: $0.24 for the
    fine-tuned checker against $107 for GPT-4 at comparable balanced accuracy.
    Leaderboard best about 77.4% balanced accuracy; every system sits near 59%
    on long-form expert answers. Leaderboard page carries no timestamp.
[^83]: Vectara hallucination leaderboard, HHEM-2.3, updated 11 May 2026.
    Answer rates of 80.7% and 62.7% accompany two of the low hallucination
    rates. Note Vectara's own paper puts an open HHEM variant at 52–67% F1 on
    one benchmark, so absolute rates carry more precision than the detector
    supports.
[^84]: Citation verification in deep-research agents, arXiv:2605.06635, 7 May
    2026. 14 models; link validity above 94%, relevance above 80%, actual
    support 39–77%; support accuracy falling about 42% as tool calls scale
    from 2 to 150.
[^85]: *DeepResearch Bench*, arXiv:2506.11763, June 2025. 100 PhD-level tasks;
    the precision-versus-volume trade-off across systems.
[^86]: Godbole and Jia, *Verify with Caution*, arXiv:2501.14883, January 2025.
    Five factuality metrics across 11 datasets; biases against paraphrased
    output and against output drawing on distant parts of the source.
[^87]: Span-level hallucination detection beyond prose, arXiv:2607.00895, 1
    July 2026. 0.17 span-F1 for a prose-trained detector on code-agent sources
    against 0.689 for one fine-tuned on that material. Single paper,
    unreplicated.
[^88]: Evan Miller (Anthropic), *Adding Error Bars to Evals*,
    arXiv:2411.00640, 4 November 2024. Source of the power formula, the
    1,000-question rule, the clustered-standard-error ratios (1.34 against
    0.44 on one benchmark), the resampling ceiling, and the warning against
    lowering temperature to reduce variance.
[^89]: Prakhar Khatri, *Do Context Files Help Coding Agents?*,
    arXiv:2607.27250, July 2026. 291 runs, three repositories, two agents,
    three strategies; no correctness benefit; manipulation-validity probe;
    120–200 tasks needed for a 10-point effect; difficulty not portable across
    agents. Data and code at
    <https://github.com/codeprakhar25/context-files-coding-agents>.
[^90]: Yagubyan, judge-reliability study, arXiv:2606.13685, 2026. 13.6% mean
    pairwise flip rate; 11 trials for a stable majority verdict. Note the
    listed submission date is internally inconsistent.
[^91]: Horace He, *Defeating Nondeterminism in LLM Inference*, Thinking
    Machines, 10 September 2025. 1,000 completions of one prompt at
    temperature 0 produced 80 unique outputs, diverging at token 103;
    batch-invariant kernels fix it at 1.6–2× latency.
[^92]: Yuan et al., arXiv:2506.09501, June 2025, and Pape, Evertz and
    Schönherr, arXiv:2605.19537, May 2026. Hardware, batching, quantisation
    and inference backend shift benchmark scores by up to 9%.
[^93]: Biderman et al. (EleutherAI), *Lessons from the Trenches on
    Reproducible Evaluation*, arXiv:2405.14782, May 2024. 50.1% against 72.4%
    on the same benchmark from prompt format alone, with rank reordering.
[^94]: Joren et al. (Google), *Sufficient Context*, arXiv:2411.06037, ICLR
    2025. Autorater at 93% accuracy; models answer correctly 35–62% of the
    time on insufficient context; one model's hallucination rate rising from
    10.2% with no context to 66.1% with insufficient context.
[^95]: Brabant (Orange Research), *Evaluating RAG Metrics in Applied
    Contexts*, arXiv:2607.07302, 8 July 2026. Word-level recall correlating
    0.35 against document-level recall at 0.05; the single-system confound;
    inter-rater ceiling of 0.85 on that data.
[^96]: Netflix Technology Blog, sequential A/B testing, 12 February 2024. A
    70% false-positive rate from repeatedly applying a fixed-horizon test to
    accumulating A/A data.
[^97]: Sentence-Transformers cross-encoder model table, MS MARCO line, current
    August 2026. <https://sbert.net/docs/cross_encoder/pretrained_models.html>
[^98]: Voyage AI, *rerank-2.5*, 11 August 2025. Instruction-following and 32k
    context; the quoted uplifts are the vendor's own evaluation with
    competitors configured by the vendor.
[^99]: Voyage AI, *The case against LLMs as rerankers*, 22 October 2025. 13
    datasets, 8 domains. Vendor evaluation; direction corroborated
    independently by ZeroEntropy, magnitudes not.
[^100]: Particula, reranker latency comparison, 22 May 2026. Methodology
    underspecified (batch size, hardware, concurrency unstated) and a separate
    source reports 392 ms for one of the same models. Ratio indicative, not
    the absolutes.
[^101]: DenseOn against LateOn, arXiv:2607.27178, 31 July 2026. Same backbone,
    149M parameters each: 56.20 against 57.22 nDCG@10 on BEIR. The cleanest
    controlled comparison of single-vector against late interaction.
[^102]: ColPali, arXiv:2407.01449, June 2024, and the 2026 compression
    follow-ups. Late interaction over page images, skipping OCR and layout
    parsing.
[^103]: Elganayni and Saleh, *Re-Ranking Through an Attribution Lens*,
    arXiv:2606.03728, 2 June 2026. On a legal QA benchmark, similarity-based
    ranking surfaced cited paragraphs worse than random selection.
[^104]: *Budget-Aware Routing for Long Clinical Text*, arXiv:2605.00336, 1 May
    2026. Knapsack formulation, monotone submodular objective, (1−1/e)
    guarantee, budgets from 256 to 16,384 tokens; the front-of-document
    baseline result.
[^105]: PACMS, arXiv:2606.20047, 18 June 2026. Facility location for agent
    context; the recall-against-accuracy decoupling table.
[^106]: *What Survives Into Context*, arXiv:2607.00725, 1 July 2026. The
    answer-in-context diagnostic, its correlation with exact match, and the
    4.6× gap among fully-retrieved questions. Single-author preprint; the
    author notes gains shrink as the reader gets larger.
[^107]: Byte-exact deduplication in retrieval-augmented generation,
    arXiv:2605.09611, 10 May 2026. The three-regime result, zero quality
    regression across four vendors.
[^108]: Cross-attention calibrated deduplication, arXiv:2607.24332, 27 July
    2026; and H3D, arXiv:2607.08382, 9 July 2026, benchmarking MinHash,
    SimHash, Winnowing, FuzzyHash and FlyHash.
[^109]: Gabín, Perez and Parapar, *Lost in the Evidence?*, arXiv:2605.27105,
    SIGIR 2026. Five models, two datasets; the U-curve fails to reproduce;
    1,000–2,000 topics needed for stable conclusions; reverse ordering worth
    2–3 F1 at k=50–100 on multi-hop.
[^110]: *Lost at the End*, arXiv:2606.16494, June 2026. Multimodal primacy
    bias; 16–26 point gold-first against gold-last gaps across five models.
[^111]: CacheWeaver, arXiv:2606.19667, 18 June 2026. Cache-aware evidence
    ordering; 20–33% median time-to-first-token reduction, 97.5% of oracle
    gain, no quality degradation.
[^112]: RCWT, arXiv:2607.12216, 13 July 2026. The displacement result: three
    commercial models stayed correct at a 95% coordination ratio when
    coordination tokens were added outside the evidence budget.
[^113]: Tool-schema compression, arXiv:2605.26165, 24 May 2026. +20.5
    percentage points exact-match at 8,000 tokens against 2.6% uncompressed.
[^114]: *Control Under Compression*, arXiv:2608.01056, 2 August 2026. 92.7%
    success at 75% retained context against a 93.8% full-context baseline.
[^115]: Cerebras, *How We Built Our Knowledge Base*, 15 July 2026,
    `cerebras.ai/blog/how-we-built-our-knowledge-base`. Practitioner report:
    architecture and design rationale, no retrieval quality figures and no
    stated evaluation method. Cited here for the assembled pipeline, not as an
    authority on retrieval — the individual techniques it uses have their own
    sources, given above. Listed on the Cerebras blog index, checked 4 August
    2026; the article URL returns HTTP 500 to automated fetches, so the
    content used here is as supplied rather than fetched.
[^116]: Cormack, Clarke and Büttcher, *Reciprocal Rank Fusion Outperforms
    Condorcet and Individual Rank Learning Methods*, SIGIR 2009. The origin of
    the method and of `k = 60`.
    <https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf>
[^117]: Anthropic, *Writing tools for agents*, 11 September 2025. Narrow tool
    surfaces, response-format control, truncation that instructs rather than
    silently cuts.
[^118]: Spärck Jones, *A statistical interpretation of term specificity and its
    application in retrieval*, Journal of Documentation, 1972. The origin of
    inverse document frequency.
[^119]: Gao, Ma, Lin and Callan, *Precise Zero-Shot Dense Retrieval without
    Relevance Labels* (HyDE), arXiv:2212.10496, December 2022.
[^120]: Zheng et al. (Google DeepMind), *Take a Step Back*, arXiv:2310.06117,
    October 2023. The +7% to +27% figures are from this paper, on PaLM-2L.
[^121]: Carbonell and Goldstein, *The Use of MMR, Diversity-Based Reranking for
    Reordering Documents and Producing Summaries*, SIGIR 1998.
[^122]: Robertson and Walker, *Some Simple Effective Approximations to the
    2-Poisson Model for Probabilistic Weighted Retrieval*, SIGIR 1994; the
    Okapi BM25 line of work.
[^123]: Provider pricing and prompt-caching documentation, all fetched
    4 August 2026: `platform.claude.com/docs/en/build-with-claude/prompt-caching`
    and `.../about-claude/pricing`; `developers.openai.com/api/docs/guides/prompt-caching`
    and `.../pricing`; `ai.google.dev/gemini-api/docs/caching` and
    `.../pricing`. Vendor documentation, and authoritative for their own
    billing.
[^124]: Yan Song, *Cache-Aware Prompt Compression: A Two-Tier Cost Model for
    LLM API Caching*, arXiv:2607.15516, 17 July 2026. One author, one provider
    API; the measured crossover is not a general threshold.
