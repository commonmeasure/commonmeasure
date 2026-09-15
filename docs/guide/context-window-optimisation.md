---
title: "Context window optimisation: a working guide"
---

# Context window optimisation: a working guide

*Choosing what a model reads and checking whether it helps.*

An agent can find the right document and still miss the answer. Extraction
may drop a table, selection may leave out a caveat, or a long conversation
may obscure the evidence it needs. The same pipeline can also spend most of
its tokens on repeated instructions, tool results and page markup.

Context optimisation means following that material from source to answer:
what enters the window, what survives each transformation, what the model can
use and what it costs. This guide works through those decisions, from counting
tokens and acquiring sources to memory and evaluation. It assumes familiarity
with prompts and the outline of retrieval-augmented generation, but no prior
reading of the research.

Prices, limits and tooling carry the dates they were checked. Specialist terms
are explained as they arise or in the [glossary](#glossary); the companion
[evidence review](state-of-the-evidence.md) gives a more detailed
assessment of the supporting evidence.

## Contents

1. [What is actually in the window](#1-what-is-actually-in-the-window)
2. [How more context changes an answer](#2-how-more-context-changes-an-answer)
3. [What the benchmarks do and do not tell you](#3-benchmarks)
4. [Acquisition: getting the right material](#4-acquisition)
5. [Extraction and chunking](#5-extraction-and-chunking)
6. [Ranking and reranking](#6-ranking-and-reranking)
7. [Selection and ordering under a budget](#7-selection-and-ordering)
8. [Compression](#8-compression)
9. [Caching, and what it costs to get the order wrong](#9-caching)
10. [Agent memory, compaction and isolation](#10-agent-memory)
11. [Proving a change helped](#11-proving-a-change-helped)
12. [Where local measurement still matters](#12-where-local-measurement-still-matters)
13. [Glossary](#glossary)
14. [Appendix: one production system, end to end](#appendix-one-production-system-end-to-end)
15. [Appendix: tooling](#appendix-tooling-checked-4-august-2026)
16. [About this guide](#about-this-guide)
17. [Sources](#sources)

---

## 1. What is actually in the window

Before deciding which documents to retrieve, work out how much room they will
have. A [context window](#g-context-window) is the bounded working material a
model can hold at once, usually measured in tokens. Text, encoded images and
other supported inputs can all count towards it.

The window includes the system prompt, conversation messages, tool results,
attached documents and the definitions of tools the model can call. The
model's output counts too, including reasoning before its answer.[^1] Input
and output share an overall limit, although the output may have a lower limit
of its own. A million-token window therefore does not leave a million tokens
for documents: everything else needs space as well.

### Why there is a limit

[Attention](#g-attention) lets a model relate one part of its input to another.
In naive dense attention, the comparison work grows quadratically: doubling
the input means roughly four times as much work.[^2] That helps explain why
long inputs are costly to process.

Serving systems use optimisations beyond that naive calculation, and vendors
do not publish all the details of how they serve million-token windows. The
quadratic relationship explains the underlying problem; it is not a formula
for predicting a particular API's price or latency.

<span id="tokens-are-not-a-unit-you-can-convert"></span>

### Count tokens with the model you will use

A [token](#g-token) is a short word, part of a longer word, punctuation or
another piece of text. The familiar estimate of four English characters per
token is useful for a rough sketch, but too loose for a budget.

A [tokeniser](#g-tokeniser) determines those pieces. Different models can
count the same sentence differently, even across generations from one
provider. Anthropic's documentation in August 2026 described a
newer tokeniser for Claude 4.7 and later producing about 30% more tokens for
identical text. With unchanged per-token prices, that also raises the cost of
sending the document.[^3]

OpenAI's `tiktoken` does not give an exact count for Claude or Gemini.
Anthropic's guidance puts its undercount at 15–20% on ordinary Claude text,
and higher on code.[^4] Neither Anthropic nor Google published a downloadable
tokeniser in August 2026. Use the relevant provider's free counting
endpoint when the count needs to be accurate.[^3]

### How much of the window does real content take?

Two files of the same character length can occupy quite different amounts of
context. This matters when an agent reads a mixture of prose, source code,
logs and generated files. The table below measures nine files in this
repository with one named tokeniser, so it provides a starting point you can
rerun rather than a universal conversion factor.

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

These measurements used tiktoken `o200k_base` on 4 August 2026, through
`docs/guide/measurements/token_density.py`. They do not give exact Claude or
Gemini counts; measure those routes separately.

Code in this sample used about 20–40% more tokens per character than prose.
The lockfile was denser still. That is useful when deciding whether to send a
whole generated file or select the relevant entries. Logs, repeated JSON
records and tool schemas deserve the same attention: their visible length
can understate their share of the token budget. Nine files and one tokeniser
are too small a sample to establish a rule for every corpus.

### A large avoidable cost: raw HTML

A fetched web page contains both readable content and the machinery used to
display it. Sending the raw response to a model can spend most of the budget
on that machinery before the model reaches the article.

Four pages were measured as retrieved and after a crude regular expression
removed tags and scripts:

| Page | Raw HTML tokens | Tags and scripts removed | Saved |
|---|---|---|---|
| Wikipedia article | 145,535 | 18,760 | 87% |
| arXiv abstract | 12,321 | 1,210 | 90% |
| vLLM docs page | 205,908 | 10,867 | 95% |
| BBC News index | 363,439 | 3,945 | 99% |

Removing HTML machinery saved 87–99% of tokens on these pages, with a median
of 92%. The remaining text still included navigation, footers and related
articles. A content [extractor](#g-extraction) tries to remove those too;
chapter 5 explains how to check that it also preserves the material you need.

Page type matters. The BBC entry is an index rather than an article, and it
accounts for about half the raw tokens in this small sample. A token-weighted
saving of 95% mostly reflects that page; excluding it gives 92%. Neither
figure establishes how much of a typical page is boilerplate, because the
script did not identify boilerplate and only four pages were measured. The
original HTML inputs were not retained, so these exact figures cannot be
reproduced. They illustrate why extraction is worth measuring on your own
sources.

### What is in there before you add anything

Tools occupy context even before they return a result. Their definitions tell
the model what each tool does and how to call it. A large catalogue can
therefore consume tens of thousands of tokens before a conversation begins:
Anthropic reported about 55,000 tokens for one five-connector setup.[^6]

Much of that text is [JSON Schema](#g-json-schema), the description of the
arguments a tool accepts. In an independent measurement of one server, 97%
of its tokens were in the input schema. Redesigning that schema reduced its
footprint from 17,161 tokens to 773.[^7] The size of a catalogue depends as
much on the shape of its definitions as on the number of tools.

In Anthropic's request layout, tool definitions precede the system prompt
and messages. As a session continues, messages and tool results can outgrow
that fixed overhead. One published session breakdown attributed about 28,000
tokens to fixed overhead and 185,000 to conversation and tool results.[^8]
The initial inventory and the growing history need separate attention.

### Available, loaded, invoked

A useful inventory distinguishes three states:

- **Available:** the tool exists and can be discovered. A short discovery
  entry may be present, while its full definition remains outside the window.
- **Loaded:** its definition is in the window and occupies space on each
  request carrying it, whether or not the tool is called.
- **Invoked:** the agent calls it, adding arguments and a result to the
  conversation as well.

Deferred loading makes this distinction practical. Anthropic's implementation
sends definitions to the service but keeps unused ones out of the model's
window. In its reported example, the loaded tool set fell from 77,000 tokens
to 8,700.[^6] Skills follow the same pattern: a name and short description
cost about 100 tokens while available, and full instructions load when the
skill is triggered.[^9] An independent count of the official skills found a
similar discovery footprint.[^10]

This is why counting calls alone misses a substantial cost. Two hundred
rarely used tools can still fill the window if all their definitions load on
every request.

### What you cannot see

Hosted APIs expose usage totals, but those totals do not explain the whole
request. You may receive input, output and cache counts, and on Anthropic a
reasoning-token count, without a breakdown separating system instructions,
tools, history and retrieved evidence.

Keep a local inventory of those categories, while allowing for limits in the
provider's reporting:

- Counting and billing can differ. Anthropic says its count may include
  automatically added system tokens for which the customer is not billed.[^3]
- A reasoning-token count does not necessarily expose the reasoning content.
- A cache miss usually appears as no cache reuse, without an error explaining
  which part of the prefix changed.
- Usage counters do not identify which evidence influenced the answer.

The last limitation matters when a request fits comfortably within the window
but the model still misses something you supplied. Understanding that failure
requires looking at what models can do with long inputs.

---

<span id="2-why-more-context-is-not-better"></span>

## 2. How more context changes an answer

Adding context gives a model more evidence, but also more material to sort
through. A document can contain the answer and still fail to help: the model
may overlook it, confuse it with a similar passage, or carry forward an
assumption from an earlier turn. These failures depend on the model, task and
kind of input. The advertised window size alone cannot predict them.

<span id="lost-in-the-middle-what-it-actually-said"></span>

### Where the answer sits

Imagine giving a model twenty documents, only one of which contains the
answer. Moving that document from the beginning to the middle changes no
facts, so differences in the answer reveal sensitivity to position.

That was the design of *Lost in the Middle*. In its GPT-3.5-Turbo experiment,
accuracy was about 75.5% with the relevant document first, about 53% around the
middle, and about 63% near the end: a [U-shaped curve](#g-u-shaped-curve). In
some settings, placing the answer in the middle produced a lower score than
giving the model no documents at all.[^11] A larger-window variant also did
not necessarily use the supplied material better than its smaller-window
counterpart.

Those were early models on inputs of roughly 2,000–6,000 tokens. The findings
explain why ordering became a concern, but the size of the effect cannot be
carried over to current models. A later controlled replication found nearly
flat performance across placements for ordinary text retrieval. Its results
were especially sensitive to which questions were sampled.[^109] Another
audit found end-to-middle drops on reasoning tasks, with smaller drops in
newer releases.[^12]

As of August 2026, those results had not been reconciled.
Position deserves attention for reasoning, tool output and multimodal input;
for a modest set of retrieved text passages, its effect may be small. Chapter
7 turns that distinction into an ordering strategy you can test.

<span id="context-rot-length-alone-degrades-performance"></span>

### Context rot: when length adds difficulty

A model can also become less reliable as an input grows even when the useful
information stays in place. This is often called *context rot*.

One way to see the problem is to remove almost all reasoning from the task.
Chroma asked models to reproduce lists of repeated words. Accuracy fell from
near-perfect on short lists to roughly 40–60% on lists of 10,000 words.[^13]
The operation remained simple; keeping track of a long input did not.

The same evaluation found that retrieval deteriorated faster when the
question and answer shared fewer words. Plausible distractors made it worse,
and their effects differed: adding one misleading passage could matter more
than adding several obvious irrelevancies. A focused prompt containing the
needed memory also outperformed that memory embedded in a much longer input.

Surrounding text had an effect of its own. Shuffling the filler to destroy its
coherence improved retrieval across the models tested. That suggests a useful
caution when assembling context: readable surrounding prose can still compete
with the evidence the question needs.

A separate study held a two-fact reasoning problem constant and padded it
with irrelevant text. Accuracy declined well below the models' window
limits.[^14] These experiments isolate aspects of length; they do not show
that every longer document will produce a worse answer.

### Effective length versus advertised length

*Effective context length* means the length at which the model can still do a
particular job reliably. The advertised length is how much input it accepts.
A useful test needs to establish the first for your task.

RULER extends simple retrieval tests with harder tasks and found that many
models did not sustain their short-input performance across their advertised
windows.[^15] NoLiMa removes the shared wording between question and answer,
so the model has to make an association rather than match a phrase. Most of
the long-window models it tested had fallen below half their short-context
baseline by 32K tokens.[^16]

One proposed explanation concerns training: even a long training window may
contain relatively few examples that require connecting distant positions.
Experiments remapping position offsets recovered some of the lost
performance.[^17] That is a possible mechanism, not a way to calculate a
universal usable fraction of a window. No traceable source was found for the
often-repeated estimate of 60–70% of advertised length.

<span id="the-counterweight"></span>

### Improvements depend on the model and task

Effective length has also improved substantially. Epoch AI tracked the input
length at which leading models maintained 80% accuracy, and found rapid gains
over its observation period.[^18] Older failures remain useful diagnostic
examples without defining a permanent limit.

A larger comparison of long-context retrieval found some models declining
after 32K or 64K tokens while others improved roughly monotonically to
100K.[^19] The failures differed too: some models increasingly refused for
copyright reasons, while others failed to follow instructions. These need
different remedies. Before shortening a prompt, inspect whether the model
missed evidence, refused to use it or answered the wrong question.

### Too many tools

Choosing a tool is another retrieval problem. As a catalogue grows, the model
must distinguish more names, descriptions and argument schemas. Similar tools
can be hard to tell apart even when the correct one is present.

LongFuncEval varied tool-definition length, tool-result length and
conversation length separately. All three affected performance, with large
differences between models.[^20] A production-catalogue study found that
shortlisting tools recovered part of the loss as the catalogue grew.[^21]
But even a shortlist guaranteed to include the correct tool left a gap of
about ten points. Finding the right candidate did not ensure the model would
choose it. Clear names and distinct descriptions therefore deserve attention
alongside the index.

The point at which this became troublesome was specific to that catalogue.
It does not establish a maximum tool count for every agent. Likewise, a
preprint testing many simultaneous instructions found a limit to perfect
compliance, with refusal becoming the failure near capacity.[^22] Adding
instructions can make a request harder to satisfy even when there is ample
room for their tokens.

### Conversations decay

A conversation can spread one complete task across many turns. The model
starts acting before it has all the information, and later messages must
correct or extend decisions already made.

A study that split fully specified instructions across turns found a large
average performance drop across the models tested.[^23] Its decomposition
showed both reduced capability and a larger increase in unreliability. Putting
the same pieces back into a single turn largely restored performance. The
information had not changed; its conversational arrangement had.

For a long-running agent, this suggests keeping the current task and decisions
explicit. A growing transcript is not necessarily a clear account of what the
agent should do next. Chapter 10 looks at ways to maintain that account.

### Pruning can improve accuracy

Old tool results often accumulate after their immediate purpose has passed.
Removing some of them can leave the model with a clearer view of the current
work, provided the necessary facts survive.

Microsoft tested this in a 50-task expense-agent benchmark.[^24] Keeping only
recent tool call/response pairs improved accuracy; adding windowed
summarisation improved it further:

| Configuration | Accuracy | Tokens |
|---|---|---|
| Full context | 71.0% | 1,480,996 |
| Keep last 5 tool call/response pairs | 79.0% | 535,274 |
| Pruning plus windowed summarisation | 91.6% | 553,374 |

The combined approach used about 63% fewer tokens and improved accuracy by
20.6 percentage points. These are the authors' results on their own agent,
with further checks across task categories and another model, rather than an
independent replication. Input accounted for almost all token use in that
workload, which explains why pruning history mattered more than shortening
answers.

Tool history also has its own position effects. LongFuncEval found newer tool
results easier to use than older ones.[^20] That differs from the original
text-retrieval U-curve, another reason to evaluate the actual input type.

<span id="the-four-failure-taxonomy-and-how-much-to-trust-it"></span>

### A vocabulary for diagnosing failures

Drew Breunig's terms *poisoning*, *distraction*, *confusion* and *clash* offer a
way to describe what went wrong.[^25] An earlier error can be carried forward;
irrelevant material can draw attention away; similar tools can be confused;
and conflicting instructions can pull the model in different directions.
These descriptions help organise a failure log before choosing a remedy.

The terms come from practitioner writing, and their empirical backing differs.
The conversation, tool-selection and long-context studies above examine
several of these mechanisms. For propagation of an injected false fact,
described here as poisoning, the sources checked in August 2026 offered an anecdote rather
than a controlled measurement. Keep that distinction when using the taxonomy.

<span id="dead-claims"></span>

### What adding noise teaches us

Extra documents sometimes appear to help for reasons unrelated to their
content. An early study reported improved retrieval-augmented answers after
adding random documents.[^26] A replication reproduced the result under the
original configuration, then found it disappeared with normal prompting.
Short generation limits and formatting choices had disproportionately harmed
the small-context baseline.[^27]

For practice, the useful finding concerns plausible mistakes in the retrieved
set. Passages that resemble the question but supply the wrong answer can be
particularly harmful, and they occur in dense-retrieval results.[^28] This
connects context management to evaluation: a change should be tested through
the final answer, with the rest of the setup held constant.

---

## 3. Benchmarks

A long-context benchmark might ask a model to find a sentence, reconstruct a
file or reason across several documents. Those tasks exercise different
abilities. Before using a score to choose a model or pipeline, identify which
ability your application needs.

<span id="the-one-finding-that-matters-most"></span>

### Match the benchmark to the work

HELMET compared long-context evaluations across many models and found that
synthetic-task scores did not reliably predict downstream performance. Even
its own task categories correlated poorly with one another.[^29]

A model that leads on retrieval may therefore be a different choice from one
that leads on synthesis or state tracking. Use benchmark results to narrow a
shortlist, then test the tasks your system will actually perform.

### Needle-in-a-haystack is a floor test

A [needle-in-a-haystack test](#g-niah) hides one sentence in filler and asks the
model to find it.[^30] It checks a necessary ability: access to a fact buried
in a long input. By August 2026, frontier models' near-perfect
scores made the simple version a weak way to distinguish them.

The task changes when a question no longer shares wording with the hidden
answer. NoLiMa tests that association and finds much earlier declines.[^16]
Reasoning across retrieved facts is harder again: GPT-4.1's launch material
reported perfect needle retrieval to a million tokens, alongside 19% accuracy
on multi-hop graph traversal above 128K.[^31]

For an application that must use a document to reach a conclusion, test that
use directly. Finding one sentence is only the first step.

### What each benchmark is actually for

The following is a snapshot of the benchmark landscape in August 2026. The question in the middle column is more useful than an overall rank.

| Benchmark | What it helps test | Qualification |
|---|---|---|
| RULER | Effective length across several tasks | Broader than a simple needle test |
| NoLiMa | Retrieval without keyword overlap | Still separates models |
| HELMET | Performance across different task categories | Helps expose weak transfer between benchmarks |
| LongBench v2 | Hard reasoning over long documents | Unsaturated in August 2026 |
| LongProc | Long structured output | Tests output as well as input handling |
| LoCoDiff | Tracking evolving state through a history | Requires exact final-file reconstruction |
| τ-bench / τ²-bench | Repeated success on agent tasks | Reports reliability across attempts |
| Terminal-Bench | Agent accuracy alongside cost | Includes dollar costs |
| BEIR / MTEB | Retrieval and embedding quality | Widely used for optimisation |
| RAGTruth | Grounding in retrieved material | Tests the output's relation to its evidence |
| NIAH | Basic access to a fact in a long input | Simple versions are saturated |
| LongBench v1 | Earlier long-context tasks | Saturated and relatively short by the check date |
| LoCoMo | Conversational memory | Baseline and configuration issues; see chapter 10 |

LoCoDiff is a useful example of a task that needs more than fact retrieval. It
gives a model a git history and asks for the exact final file.[^32] There is
no filler: changes throughout the history may affect what must be reproduced.
In the reported evaluation, all models fell below 50% above 25,000 tokens,
despite much higher scores on long needle tests.

τ-bench adds a different requirement: consistency. Its `pass^k` metric counts
tasks completed successfully on *all* k independent attempts.[^33] A high
single-attempt score can conceal repeated failures that matter to users who
rely on the same workflow every day.

<span id="length-alone-degrades-performance-even-with-perfect-retrieval"></span>

### Test the reader as well as retrieval

Suppose the retriever finds every required passage. A long prompt can still
make the answer worse. One study isolated this by replacing irrelevant text
with whitespace, restricting attention and placing the evidence immediately
before the question. Performance still declined across the models tested.[^34]

These controls help separate length from distractor content and position.
They also explain why retrieval recall alone cannot validate a context
pipeline: you need to test the model that reads the assembled prompt.

<span id="contamination-cannot-be-audited-away"></span>

### Allow for benchmark contamination

A public benchmark may overlap with a model's training material. The model
can then benefit from remembering benchmark-specific content, which is a
different ability from solving a new task. SWE-bench experiments found higher
rates of file identification and function reproduction for benchmark material
than for comparison material outside it.[^35]

Detecting such overlap is itself unreliable. An evaluation of contamination
audits found that the tested methods often gave incorrect results, particularly
under distribution shift.[^36] Passing an audit cannot establish that a
benchmark was absent from training.

Held-out material with known provenance and refreshed tasks can reduce this
problem. EvoBrowseComp, for example, generates live-web questions from newer
material.[^37] For your own evaluation, retain a private set alongside public
benchmarks and record when its source material became available.

<span id="vendors-report-on-evals-they-own"></span>

### Record dataset versions and omitted tasks

Vendor launch evaluations are useful sources, but inspect what they test and
which version they use. OpenAI's MRCR and Graphwalks datasets were corrected
after their initial publication, making scores from different versions hard
to compare.[^31] Public datasets also leave outsiders without the vendor's
private held-out material.

An advertised capability may have no corresponding launch score. Google's
Gemini 3 launch material highlighted a million-token window while reporting
other kinds of benchmarks rather than a long-context benchmark.[^38] The
window size establishes capacity; a suitable evaluation still needs to
establish performance.

<span id="only-one-leaderboard-publishes-cost"></span>

### Compare the cost of a score

Two systems with similar accuracy can have very different operating costs.
Terminal-Bench was unusual among these leaderboards in publishing both.
Its 4 August 2026 snapshot illustrates why the cost belongs beside the score:[^39]

| System | Accuracy | Cost |
|---|---|---|
| Claude Code / Fable 5 | 83.8% ± 1.2% | $552.67 |
| Codex / GPT-5.5 | 83.1% ± 1.1% | $2,059.19 |

The reported accuracy intervals overlap, while the costs differ by about
3.7 times. For deployment, that can matter more than the small difference in
mean accuracy. Compare cost per correct answer and latency as well as quality.

<span id="what-nothing-measures"></span>

### Fill the gaps with your own evaluation

These benchmarks leave several concerns only partly measured or
unmeasured. If these matter to the application, include them in your own suite:

- **Source selection:** whether the agent finds an authoritative, primary
  source, rather than merely a page containing the expected answer.
- **Attribution:** whether a claim is supported by the cited passage.
  HELMET includes citation tasks, but their scores transfer poorly to other
  task categories.
- **Conflicting evidence:** whether the model notices and handles sources
  that disagree. RECON begins to test this, with low reported performance
  outside its oracle condition.[^40]
- **Freshness:** whether the system recognises that evidence has become stale.
- **Abstention:** whether it declines appropriately when evidence is missing,
  rather than receiving credit only for attempting an answer.
- **Untrusted content:** whether retrieved text can divert the agent from
  its task.
- **Operating cost and delay:** what each correct answer costs and how long
  the user waits, including retries and failures.

This gives acquisition a clearer objective. Finding a page is useful only if
it supplies evidence the system can safely and accurately use.

---

## 4. Acquisition

Acquisition decides what material the rest of the pipeline can work with. If
the necessary source never arrives, ranking and compression cannot recover
it. Start with the shape of the task: a question about one document, a search
across a known collection and an investigation on the live web need different
ways of gathering evidence.

<span id="long-context-versus-retrieval-what-the-comparison-actually-shows"></span>

### Choosing between whole documents and retrieval

For a question that compares sections or needs several reasoning steps,
sending the whole document preserves connections that chunk retrieval might
miss. For a narrow factual question, a few well-chosen passages may supply
all the evidence at much lower cost.

A comparison across nine datasets found that long context generally produced
better answers when given enough budget. Yet about 63% of queries produced
identical answers with long context and retrieval.[^42] That agreement makes
routing useful: reserve the expensive path for questions that need it.

The same paper's *Self-Route* method asks the model whether the retrieved
chunks suffice, then escalates to full context when they do not. It retained
near-long-context quality while reducing cost in the tested setups.[^42]
This is a concrete alternative to choosing one strategy for every query.

The point where retrieval becomes preferable depends on model and task. LaRA
found advantages for long context on reasoning and comparison, while retrieval
helped with hallucination detection and helped smaller models at long input
lengths.[^43] Its crossover near 128K tokens is a result of those experiments,
not a general routing threshold. Test whole-document and retrieval baselines
on the same questions, especially where the task needs evidence from distant
sections.

<span id="retrieval-fails-in-four-ways-and-three-are-query-problems"></span>

### Diagnose the query before changing the index

A retriever needs a query it can match to useful material. Manual error
analysis in the long-context comparison identified four ways that can fail:[^42]

- A multi-step question needs a chain of lookups that one retrieval call does
  not perform.
- A vague question gives too little detail to match reliably.
- A long, complex question mixes requirements the retriever fails to separate.
- An implicit question asks for something whose wording or meaning is not
  directly present in a passage.

These suggest different experiments. Query rewriting can clarify a vague
request; decomposition can turn a multi-step task into smaller searches.
Improving the representation may help when the connection to the answer is
implicit. The taxonomy comes from one paper, but it provides a useful way to
choose a change from observed failures instead of adding every retrieval
technique at once.

### How many sources is enough

A retriever can keep finding relevant documents after the reader has stopped
benefiting from them. In the original *Lost in the Middle* experiments,
increasing the retrieved set from 20 to 50 documents improved reader accuracy
only slightly while [recall](#g-recall) continued to rise.[^11]

The extra passages also create opportunities for confusion. In one evaluation,
over 60% of queries had at least one convincing but wrong passage among a
dense retriever's top ten results.[^28] A close match can require more careful
discrimination than an obviously irrelevant document.

Anthropic found twenty passages worked best in its contextual-retrieval
setup; practitioner starting points after reranking often use five to ten.[^44]
Those are settings to test, not a universal answer. Tune the admitted count,
usually called *k*, against answer quality and cost in your complete pipeline.

<span id="techniques-and-whether-they-pay-for-themselves"></span>

### Choose techniques by the failure they address

The following techniques change different parts of the search. Their value
depends on what happens after they return candidates.

| Technique | What it changes | What to test |
|---|---|---|
| Hybrid dense + BM25, with rank fusion | Combines semantic and exact-term matches | Whether each route finds useful candidates the other misses |
| Cross-encoder reranking | Reads the query with each shortlisted passage | Whether it removes retrieval failures at the final k; chapter 6 |
| Contextual chunk headers | Restores document context before indexing | Corpus search versus within-document search; chapter 5 |
| Query rewriting | Makes a vague query more explicit | Whether the rewrite addresses observed query failures |
| Query decomposition | Splits a multi-step question | Whether the separate searches recover the required chain |
| Multi-query / RAG-Fusion | Broadens the candidate pool | Whether extra candidates survive reranking and the token budget |
| HyDE[^119] | Generates a hypothetical document to search by | Whether it helps the retriever being used; its original gains were over unsupervised retrievers |
| Step-back prompting[^120] | Searches through a more general question | Whether gains from the original older-model setup transfer |
| Prompted iterative retrieval | Lets the model search again after reading | Answer quality, retrieval calls and token use against a good one-shot search |
| RL-trained search policy | Trains the decisions about when and how to search | Whether training is feasible and improves your search tasks |
| GraphRAG | Uses graph-derived structure for retrieval and synthesis | Grounded answer quality as well as preference scores and indexing cost |
| Multi-agent research | Divides investigation across separate contexts | Whether parallel reading earns its extra token and coordination cost |

<span id="what-hybrid-actually-needs-to-contain"></span>

### What hybrid search combines

A literal error message and a loosely worded description of a fault call for
different kinds of matching. Hybrid search combines them so the first stage
can find both.

Full-text search, including BM25,[^122] retains exact terms that embeddings
can blur: error strings, flag names and host names. A literal match can be
especially useful when someone pastes the error they received. Embeddings
help with paraphrase: someone asking “restore hangs after manifest load” may
need an answer describing a “checkpoint stalled on the NFS mount”, despite
having little wording in common.

Term rarity, or inverse document frequency,[^118] helps distinguish specific
words from conversational filler. “Sounds good, thanks!” provides little
search signal even if its embedding lies near many queries. Recency can be
another signal when older answers describe infrastructure that has changed.
A date should inform relevance alongside the content, rather than act as an
unexamined cut-off.

The result lists need a shared ranking. Reciprocal rank fusion assigns each
document `weight / (k + rank)` in each list where it appears, then adds those
contributions. The original method used `k = 60`.[^116] A document with
moderate ranks in several lists can then beat one found near the top of just
one list. Working with ranks also avoids having to make BM25 scores and
embedding similarities share a numerical scale.

Recency weighting is a practitioner design choice for handling stale answers;
the cited material does not quantify its benefit. The same care applies to
the more elaborate approaches in the table.

Multi-query fusion, for example, found more relevant candidates in an
enterprise deployment but reduced Hit@10, the rate of finding a relevant
result in the final ten.[^45] The reranker and token budget constrained what
could reach the reader. Broader retrieval added work without improving that
final set. Include the downstream reader when evaluating any technique that
widens recall.

Iterative retrieval lets a model refine a search after seeing the first
results. In a direct comparison, a well-configured one-shot search slightly
outperformed iteration, while iteration made substantially more retrieval
calls. The iterative approach's advantage was token efficiency in that
setup.[^46] Search-R1's larger reported gains came from training a search
policy with reinforcement learning.[^47] Prompting an existing model to use a
search tool repeatedly does not reproduce that training intervention.

GraphRAG's reported benefits need another distinction. Its evaluations favoured
answers for comprehensiveness, diversity and usefulness on open-ended tasks;
faithfulness was similar to baseline retrieval.[^48] Those preferences may
matter to a product, but they do not establish improved factual accuracy.
Indexing cost also matters: Microsoft's LazyGraphRAG work reports a large
reduction against full GraphRAG.[^49] As of August 2026, no independent ground-truth comparison was found establishing that full GraphRAG beats a
well-tuned hybrid pipeline.

### How much do two search providers agree?

A second search provider may find sources the first misses. To explore that,
the authors sent 24 queries across six types—factual, fresh, technical,
commercial, vague and analytical—to three providers on 4 August 2026. Each
request asked for ten search results. Four of the 72 calls failed and were
excluded, rather than treated as empty result sets.

The comparison uses hosts rather than exact URLs. That is a coarse view of
source diversity: two hosts can repeat one source, while two pages on one
host can be independent.

| Provider | Mean results | Mean distinct hosts | Host concentration |
|---|---|---|---|
| Exa | 10.0 | 8.3 | 0.18 |
| Tavily | 9.0 | 7.8 | 0.16 |
| Firecrawl | 10.0 | 9.0 | 0.13 |

A concentration of 1 would mean every result came from one host. The lower
values here indicate that each provider spread results across hosts. To judge
what a second provider adds, look at overlap between the pairs:

| Pair | Share of hosts in common |
|---|---|
| Tavily and Firecrawl | 0.62 |
| Exa and Firecrawl | 0.21 |
| Exa and Tavily | 0.18 |

Any one provider returned about eight or nine distinct hosts per query; the
combined set averaged 15.5. Exa contributed a larger proportion of unique
hosts in this sample, while Tavily and Firecrawl overlapped more. This suggests
measuring provider combinations, rather than assuming any second provider will
add the same coverage.

Host variety also says little about source quality. Among the most frequent
hosts were Reddit, YouTube, arXiv, Medium and the vLLM documentation site.
A retrieval pipeline still needs to decide which kinds of evidence are
appropriate for the task.

These results describe one English-language run over a small query set, not a
stable ranking of providers. The script is
`docs/guide/measurements/source_overlap.py`; rerun it on your own queries and
inspect what the additional sources contribute to answers.

### Live web acquisition

Search results help choose what to read. Fetching every candidate can expand
the input dramatically: the provider examples used here put an average page
around 2,500 tokens, a large documentation page around 25,000 and a research
PDF around 125,000.[^50] Ten full PDFs can exceed a million-token window.
Search first, then fetch the pages whose contents are needed.

Billing reinforces that distinction. At the 4 August 2026 check, Anthropic
charged per search query and billed fetched content as input tokens; other
providers used combinations of query charges, page charges, credits and
output-token minimums.[^51] The cheapest search call is not necessarily the
cheapest route to an answer if it leads to much more reading.

Fetch tools can also impose constraints on acquisition. The platform tools
described here respect `robots.txt` and require a URL already present in
context, which makes search-then-fetch part of the workflow. Publisher access
can carry a separate price: Cloudflare's pay-per-crawl uses HTTP 402 and
cryptographic crawler signatures to let publishers allow, charge or block
access.[^52] Acquisition therefore needs to account for access as well as
search relevance.

---

## 5. Extraction and chunking

Once a document arrives, two steps determine what can be retrieved from it.
Extraction separates useful content from the page around it. Chunking divides
that content into pieces small enough to match and select. A failure in either
step can remove evidence before the answer model ever sees it.

<span id="extraction-is-where-the-tokens-are"></span>

### Extract the body and check what survives

Chapter 1's four-page measurement showed how much raw HTML can cost. An
extractor goes further than stripping tags: it tries to identify the body and
leave out navigation, adverts and footers. That saves space, but an extractor
can also omit a table, paragraph or whole page that the task needs.

Zyte's article-extraction benchmark compares extracted content with an
article-body reference using overlapping four-word sequences.[^53]
*Precision* indicates how much returned material matches the body; *recall*
indicates how much of the body survives. Both matter when the output will
become model context.

| Extractor | F1 | Precision | Recall |
|---|---|---|---|
| `rs_trafilatura` (Rust) | 0.970 | 0.951 | 0.990 |
| `go_trafilatura` | 0.960 | 0.940 | 0.980 |
| trafilatura (Python) | 0.958 | 0.938 | 0.978 |
| Readability (JS) | 0.947 | 0.914 | 0.982 |
| readability-lxml | 0.922 | 0.913 | 0.931 |
| `justext` | 0.804 | 0.858 | 0.756 |
| BeautifulSoup (plain) | 0.665 | 0.499 | 0.994 |
| `html2text` | 0.662 | 0.499 | 0.983 |
| `htmd` (Rust) | 0.184 | 0.102 | 0.970 |

The converters near the bottom keep most body text but also return much of
the surrounding page. That can be desirable for conversion, while being a
poor fit for compact article context. The scores compare word sequences, so
they cannot directly establish a token-cost multiplier.

This benchmark covered 181 pages and was archived in June 2026. Treat it as a
comparison on those pages, then inspect extraction on your own sources. In
particular, distinguish Readability implementations and evaluation runs: the
JavaScript entry above has high recall; lower figures elsewhere cannot be
applied to it indiscriminately. MarkItDown is a converter whose HTML path
wraps markdownify, rather than a body extractor. None of these tools renders
JavaScript, leaving another source of missing content.

Check page survival as well as scores on pages that survive. A separate study
found that extractors with similar benchmark scores retained different sets
of pages. Combining their outputs increased token yield without a benchmark
regression.[^54] A clean-looking result is not enough if the extraction stage
silently discards documents.

<span id="chunking-matters-less-than-you-have-been-told"></span>

### Start with simple chunks

Small chunks make it easier to match a specific passage. Larger chunks retain
more of its surrounding explanation. Overlap preserves text around boundaries
but also returns repetition. These are the trade-offs a chunking strategy
needs to manage.

In Chroma's evaluation, a simple recursive splitter was competitive with more
elaborate semantic approaches.[^55] The LLM-guided splitter found slightly
more relevant text, but returned substantially more irrelevant text with it:

| Strategy | Recall | Precision |
|---|---|---|
| Recursive splitter, 200 tokens, no overlap | 88.1% | 7.0% |
| Cluster semantic chunker, 200 tokens | 87.3% | 8.0% |
| LLM-guided semantic chunker | 91.9% | 3.9% |

For a starting configuration, a recursive splitter at 200–400 tokens with no
overlap is worth testing. Change it when failure inspection shows that
boundaries or missing surroundings are losing answers. This is one company's
evaluation on five corpora, not proof that one size suits every document.

[Parent-document retrieval](#g-parent-document) separates matching from
reading: search small chunks, then give the model the larger section each
winning chunk came from. That preserves precise matches while restoring some
context. No published measurement establishing its benefit was found;
it is a design pattern to evaluate through the reader.

<span id="contextual-retrieval-the-best-documented-vendor-upgrade"></span>

### Restore the context a chunk lost

“The company” and “this quarter” mean little once their paragraph is detached
from the document. Contextual retrieval addresses this by having a model write
a short situating header for each chunk at index time. The header, typically
50–100 tokens, accompanies the text used for search.

Anthropic measured retrieval failures as it added contextual embeddings,
contextual BM25 and reranking.[^44] This is the vendor's evaluation of its
own technique, with no independent reproduction found as of August 2026:

| Configuration | Top-20 retrieval failure rate |
|---|---|
| Baseline | 5.7% |
| + contextual embeddings | 3.7% |
| + contextual BM25 | 2.9% |
| + reranking | 1.9% |

The progression matters because it shows where each step helped. Contextual
headers reduced missing evidence; reranking removed further failures after
both search routes were present. Anthropic reported an indexing cost of about
$1.02 per million document tokens, made possible by caching the shared document
prefix during header generation.

The same header can make sibling chunks harder to distinguish. A chunking
study found improvements for search across a corpus but degradation when
searching within one document.[^56] Document-level context helps tell documents
apart; repeated across one document's chunks, it can blur their differences.
Choose the technique with the search scope in mind.

<span id="two-steps-further-both-unmeasured"></span>

### Other ways to prepare an index

Some systems take contextual preparation further by rewriting each source into
a common structure. A support thread might become a record containing the
question someone would search for, a summary, the resolution and the systems
involved. The index embeds that record. The aim is to make a code comment,
incident record and wiki passage more comparable when they address the same
problem.

Another approach filters material before embedding it, using signals such as
rare terms, minimum length, reactions or links. This can keep conversational
filler out of the candidate pool and runs once per source item rather than on
every query. It also makes an early selection decision that needs checking
for lost evidence. The production example in the appendix uses both patterns;
neither comes with published quality measurements in the sources cited here.[^115]

Late chunking preserves surrounding context in a different way. It embeds the
whole document at token level, then pools those representations into chunks.
Each chunk's representation has therefore been influenced by its neighbours,
without a separate header-generation call for every chunk.[^57]

Jina's evaluation of its own technique found gains that varied by dataset and
grew with document length. A head-to-head comparison put contextual retrieval
slightly ahead but at much higher cost.[^58] Both approaches are worth
understanding as ways to retain context during indexing; their benefit still
depends on the documents and questions being matched.

---

## 6. Ranking and reranking

Retrieval and ranking are different jobs. Retrieval asks “which hundred of
these million documents might be relevant?” Ranking asks “which five of these
hundred actually are?” The first must search cheaply over a large collection.
The second can spend more work on each candidate because there are far fewer
of them.

### Why a second pass helps

First-stage retrieval compares pre-computed representations. A passage's
[embedding](#g-embedding) is created before the question is known, so it cannot
be shaped by that question.

A [cross-encoder](#g-cross-encoder) reads the question and one candidate passage
together, then scores the pair. That allows more detailed matching. It is too
expensive to apply to every document in a large corpus, but affordable for a
shortlist of fifty candidates.

This division of work explains the common pattern: retrieve many candidates,
rerank them, then admit a smaller set to the answer model.

<span id="the-measured-gain"></span>

### What the second pass can recover

The contextual-retrieval experiment in chapter 5 shows a reranker adding value
after two search routes were already in place. In Anthropic's own evaluation,
reranking reduced top-20 retrieval failure from 2.9% to 1.9%, removing about a
third of the failures left by the earlier stages.[^44]

That is a useful measure for a reranker: how often does its final shortlist
contain evidence the reader needs? A ranking score alone does not tell you
whether a missed answer has been recovered.

<span id="bigger-rerankers-stop-helping-quickly"></span>

### Choose model size against the gain

A larger reranker can cost substantially more without improving the shortlist
much. The published MS MARCO cross-encoder family illustrates diminishing
returns.[^97] Here, nDCG@10 rewards placing relevant documents near the top,
while throughput indicates the processing cost:

| Model | nDCG@10 (TREC DL19) | Documents per second |
|---|---|---|
| TinyBERT-L2 | 69.84 | 9,000 |
| MiniLM-L6 | 74.30 | 1,800 |
| MiniLM-L12 | 74.31 | 960 |

Moving to six layers improved the score but cut throughput by a factor of
five. Moving to twelve layers nearly halved throughput again for little score
change. If your queries resemble this evaluation, the six-layer model is a
reasonable starting point; measure elsewhere before transferring the result.

Newer rerankers offer capabilities beyond size, including a 32,000-token
context for whole documents and instructions defining relevance for a query.
A vendor evaluation found instruction following contributed substantially to
the gain.[^98] An instruction can express which kind of evidence the task
requires, something a generic similarity score may not capture.

<span id="do-not-use-a-frontier-model-as-your-reranker"></span>

### Dedicated rerankers and general models

A general-purpose model can be prompted to order passages, but it brings the
cost of generation to a task that a cross-encoder performs in a batched scoring
pass. Listwise LLM rerankers may also need several sliding-window passes to
process all candidates.

One vendor's comparison found its cross-encoder ahead of frontier models in
quality, latency and cost.[^99] The direction had independent support in the
cited material, while the vendor-configured magnitudes should be treated
cautiously. The same comparison found LLM reranking more useful when the
first-stage retrieval was poor. In that situation, test improvements to the
candidate pool alongside changing the reranker.

<span id="it-is-cheap-relative-to-what-it-saves"></span>

### Work out whether reranking pays

Chapter 8 works through an example where scoring 20,000 candidate tokens costs
about $0.001 and selecting 2,000 of them saves $0.09 in uncached frontier-model
input. That leaves ample room for the reranker's cost. The calculation changes
when the saved tokens would have been read from a warm cache or sent to a
cheap model: reranking can then cost more than the input it removes.

Latency needs its own measurement. One third-party comparison reported a
substantial gap between hosted and self-hosted rerankers, but did not state
enough about hardware, batching or concurrency to isolate the cause.[^100]
Measure end-to-end latency under your expected load before deciding whether
self-hosting earns its operational cost.

### The options, roughly ordered by cost

| Approach | How it scores | Where it fits |
|---|---|---|
| Bi-encoder, or plain embeddings | Compares pre-computed vectors | A fast first stage, with limited query-specific detail |
| Late interaction, such as ColBERT | Keeps token vectors and matches query terms to them | More detailed retrieval at additional index cost |
| Cross-encoder | Reads the query and passage together | A second pass over a shortlist |
| LLM reranker | Generates an ordering of passages | A more expensive option to compare with a dedicated scorer |

Late interaction retains term-level detail that a single document vector
compresses away. A controlled comparison using the same backbone found only
about one nDCG@10 point between the two approaches.[^101] Compression and
on-disk token embeddings reduce its storage burden, but for English text it
still needs to justify its cost against dense retrieval plus a cross-encoder.

For visual documents, the design offers a different benefit. Late interaction
over page images can search PDFs, slides, scans and tables without a separate
OCR and layout-parsing pipeline.[^102] That makes it a relevant option when
converting the page to text is itself a source of lost evidence.

<span id="rank-for-attribution-not-similarity-if-you-need-citations"></span>

### Rank for the support an answer needs

A passage can resemble a question without supporting its answer. In a legal
question-answering benchmark, semantic similarity was a poor predictor of
which paragraphs the model cited; similarity ranking surfaced those paragraphs
worse than random selection.[^103]

That finding is specific to the benchmark, but the distinction applies to
citation-bearing products: evaluate whether selected passages support the
answer, alongside whether they resemble the query. Otherwise a relevance gain
can leave attribution unchanged or worse.

<span id="put-the-context-back-after-you-rank-not-before"></span>

### Restore surrounding text after ranking

A chunk may omit the heading that names a software version, the preconditions
above it or the caveat below. Once the winning chunks are chosen, retrieving
their neighbouring sections can give the answer model a more complete passage.

Doing this after reranking keeps the scorer's inputs focused. Expanding first
would change what the reranker compares, because each candidate would include
additional surrounding material. This is parent-document retrieval applied at
the end of selection: match and rank small chunks, then restore the larger
section. Measure whether expansion helps the reader and account for the extra
tokens before final packing.

<span id="the-one-thing-to-measure"></span>

### Measure the final shortlist

Alongside ranking scores, measure retrieval failure at the k you actually
admit: how often is the needed answer absent from what the model finally sees?
This exposes a failure the rest of the pipeline cannot repair by reading more
carefully. Then measure the reader's answer too, because presence alone does
not guarantee successful use.

---

## 7. Selection and ordering

The ranked list is a set of candidates. The prompt is a bounded collection of
evidence. Turning one into the other means deciding what fits, what repeats
material already selected and what must remain together for the answer to be
understandable.

<span id="the-budget-is-shared-and-most-of-it-is-already-spent"></span>

### Work with the remaining budget

Retrieved evidence shares the window with instructions, tools, memory,
conversation history and space reserved for output. In the session example
from chapter 1, conversation and tool results had grown far larger than the
fixed overhead.[^8] A budget that worked at the start of the session may be too
large several turns later.

Count those categories before packing evidence. Deferred tool loading can
recover substantial room in a tool-heavy setup; pruning history may matter
more later in a long-running agent. Which change helps most depends on the
inventory, rather than on a fixed percentage assigned to retrieval.

### Scope before you select

A compiler engineer and an infrastructure operator may search the same
organisation's collection while needing different default sources. Scoping
the query to the relevant project or source bundle reduces the candidate pool
before retrieval starts. Material outside that scope cannot distract the
reranker or reader.

The trade-off is cross-domain recall. A question may genuinely need material
from another team's sources, so make scope visible and changeable rather than
an unexplained permanent restriction. This is a practical filter to test,
especially in collections full of similar terminology from different projects.

<span id="how-many-to-admit"></span>

### Tune how much reaches the reader

The studies in chapters 2 and 4 point to three useful considerations: reader
accuracy can level off while retrieval recall continues to climb; convincing
near-misses can be harmful; and removing older tool results can sometimes
improve answers.[^11][^28][^24]

Together they suggest starting with a broad candidate search and a selective
final prompt. Five to ten reranked chunks is a practitioner starting point;
Anthropic found twenty best for its own setup.[^44] Vary the admitted count
and token budget against the final task score. An improving recall curve does
not by itself justify a larger prompt.

<span id="greedy-top-k-is-probably-the-wrong-algorithm"></span>

### Select a useful set, not just high-scoring items

If the five highest-ranked passages repeat the same fact, selecting all five
can leave no room for a lower-ranked passage containing another fact the
answer needs. Per-item relevance and set usefulness are different objectives.

Maximal marginal relevance, or MMR, addresses repetition by balancing a
candidate's relevance against its similarity to items already selected.[^121]
More recent work frames the problem as selection under a token budget: each
candidate has a cost, and the value of adding it depends on the current
set.[^104][^105][^106]

This resembles a knapsack problem: choose useful items that fit within a
capacity. If the objective has diminishing returns—a *submodular* objective—
particular algorithms can provide an approximation guarantee. The clinical-text
study's budget-aware algorithm guarantees roughly 63% of the optimum under
its stated assumptions.[^104] That mathematical guarantee applies to its
objective and algorithm, not to answer accuracy or every greedy packer.

A *facility-location* objective rewards a set for representing the candidate
pool well. MMR penalises repetition; facility location also asks how well
unselected candidates are represented by the chosen ones. In one comparison,
that distinction improved answer accuracy despite slightly lower evidence
recall:[^105]

| Method | Evidence recall | End-to-end accuracy |
|---|---|---|
| Top-k | 0.933 | 50.0% |
| MMR | 0.895 | 44.0% |
| Facility location | 0.909 | 52.0% |

These results do not establish a universal winning packer. They show why the
reader's score belongs in the comparison. Another study checked whether the
gold answer survived as a contiguous span in the packed context. That
predicted exact-match accuracy better than document recall, with substantial
variation even among questions for which every gold document had been
retrieved.[^106] Retrieving a document and preserving its answer are separate
steps.

The three studies had not been independently replicated as of August 2026.
Include simple baselines: under a tight budget, taking the beginning of a
document can work well when the corpus puts key facts first, and poorly when
it does not.[^104]

### Removing redundancy

Exact duplicates are a straightforward place to recover space. Compare the
bytes of candidate chunks and keep one copy of each repeated passage. A study
found very different savings across its corpora:[^107]

| Corpus type | Context removed by exact deduplication |
|---|---|
| Clean academic | 0.16% |
| Enterprise | 24.03% |
| Conversational | 80.34% |

The study reported no quality regression across four model vendors. The large
conversational saving describes that corpus, rather than conversation in
general; the near-zero academic saving is equally useful when estimating
whether the step will matter.

After exact matching, fingerprint methods such as MinHash or SimHash can find
near-duplicates in scraped or conversational text. Embedding similarity can
also find paraphrases, at higher processing cost.[^108] Add that complexity
when inspection shows useful remaining redundancy, and check that near-match
removal does not discard distinct facts.

<span id="in-what-order--and-the-u-curve-may-not-survive"></span>

### Order for the input type and task

The original *Lost in the Middle* result suggests placing important evidence
at the edges. As chapter 2 explained, later work did not consistently reproduce
that pattern for text retrieval.[^109] Small sets of test questions were a
major source of unstable conclusions.

In the controlled replication, ordering made little difference with five to
ten text passages. With fifty to a hundred passages on multi-hop questions,
putting the best evidence last gave a small gain on one dataset. The other
dataset showed no ordering sensitivity. These are useful starting hypotheses
for your own prompt, rather than a rule for all retrieval.

Other input types behaved differently. Visual question answering showed a
substantial advantage for placing the answer-bearing image first.[^110]
Tool-result experiments favoured newer results over older ones.[^20] And
Chroma's filler experiments suggest the content surrounding evidence can
matter as well as its position.[^13]

For a short list of text chunks, first test whether order makes a material
quality difference. If it does not, stable ordering may offer a caching benefit.
For long multi-hop or visual inputs, compare placing the best evidence at the
beginning and at the end.

### Order for the cache

Caching can reuse the beginning of a prompt when it matches a previous
request. Reordering the same evidence may therefore change processing cost and
latency even if answer quality stays similar.

One study maintained a prefix tree of recently served evidence sequences and
placed reusable prefixes first. This reduced median time to first token by
20–33%, with no measured answer-quality loss.[^111] It operated at the prompt
layer, without changes to the serving system.

The result had not been independently replicated, but it gives a reason to
measure reuse before repeatedly reordering a small evidence set for marginal
relevance gains. Chapter 9 explains the cache mechanics and costs.

<span id="budget-evidence-first-then-fit-everything-else-round-it"></span>

### Reserve room for the evidence you need

Instructions and coordination messages can fill the window until little
evidence remains. In one fixed-window experiment, performance fell sharply
when only a few hundred evidence tokens survived. When coordination tokens
were added outside that fixed budget, preserving the evidence, the tested
commercial models stayed correct even at a very high coordination ratio.[^112]

In that setup, the loss came from evidence being displaced. It suggests
reserving enough room for the task's required material and then fitting
coordination around it. Tool schemas are worth inspecting early: a separate
study recovered substantial accuracy by compressing schemas under a tight
budget.[^113]

How far other context can be cut needs a local measurement. One study retained
near-baseline success at 75% of the original context, with a sharper decline
between 50% and 35% retention.[^114] Those are results from one unreplicated
setup, not safe and unsafe percentages for every agent. Inspect what was
removed at each step, especially facts and instructions needed later.

<span id="what-is-not-known"></span>

### What still needs local measurement

The cited work does not supply a validated allocation formula for
instructions, tools, memory, evidence, history and output. It also leaves
output reservation largely unmeasured: there is no established rule for how
much space to hold back for the answer.

Other gaps remain around why text and visual ordering effects differ, and how
source overlap changes useful evidence coverage. The provider comparison in
chapter 4 is only 24 queries in one run. Treat these as parts of the pipeline
to instrument, rather than filling unknown quantities with fixed percentages.

---

## 8. Compression

Compression reduces material you have already chosen to send. It can make a
large, changing input affordable, but it also creates another opportunity to
lose a fact, qualification or citation. Before paying for that transformation,
check whether extraction, selection and caching can solve the budget problem.

<span id="the-headline-number-does-not-survive-checking"></span>

### Match the compression claim to the material

Repeated examples in a few-shot prompt contain scaffolding a compressor can
remove many times. A web page with a single crucial detail presents a different
problem. The ratio achieved on one does not establish what can be safely
removed from the other.

LLMLingua's widely quoted “up to 20×” result came from a mixture including
few-shot reasoning prompts with repetitive structure.[^59] Its successor,
LLMLingua-2, reported 2×–5× compression.[^60] The retrieval-oriented
LongLLMLingua reported improved accuracy with around four times fewer tokens
on NaturalQuestions.[^61] Each result describes its own data and setup;
together they give a range of possibilities to test, rather than one expected
saving for retrieved documents.

The implementation's maintenance also belongs in the decision. At the 4 August
2026 check, LLMLingua remained available under MIT but had little recent commit
activity. The tooling appendix retains that dated snapshot.

### The families, briefly

Compression methods differ in what they preserve and what access they need:

| Family | How it works | Requires access to answer-model weights? |
|---|---|---|
| Token-level, such as LLMLingua | Drops tokens scored as low-information, using perplexity or a trained classifier | No |
| Extractive, such as RECOMP | Keeps selected sentences or passages | No |
| Abstractive | Uses a model to summarise the material | No |
| Soft or learned, such as gist tokens, ICAE and xRAG | Encodes material into internal model states | Yes |

Text-based approaches can sit in front of a hosted API. Methods that inject
compressed internal states require access below that API boundary, so they are
not interchangeable deployment options.

RECOMP also allows its compressor to return an empty string when retrieved
documents are irrelevant.[^62] This combines compression with an admission
decision: some material should contribute no context at all. Measure that
choice separately from how tightly useful material can be compressed.

### The arithmetic

Suppose a search returns fifty chunks of 400 tokens each: 20,000 candidate
tokens. Selecting five leaves 2,000, saving 18,000 tokens in the answer call.
Whether the selection step pays depends on the rate those removed tokens
would otherwise have cost.

The example below uses prices checked on 4 August 2026. It compares the cost
of processing the candidates with the value of the input removed. The LLM
compression rows count its input cost only, so its output adds further cost.

| Approach | Cost of the step | Value of tokens saved | Saving / step cost |
|---|---|---|---|
| Rerank ($0.05/M) → Opus 5 uncached input ($5.00/M) | $0.0010 | $0.090 | 90:1 |
| Rerank → Opus 5 cached read ($0.50/M) | $0.0010 | $0.009 | 9:1 |
| Rerank → Haiku 4.5 cached read ($0.10/M) | $0.0010 | $0.0018 | 1.8:1 |
| Rerank → cheap model, cached ($0.02/M) | $0.0010 | $0.00036 | 0.36:1 |
| LLM compression (Haiku input $1.00/M) → Opus 5 uncached | $0.020 | $0.090 | 4.5:1 |
| LLM compression → Opus 5 cached read | $0.020 | $0.009 | 0.45:1 |

A ratio below one means the processing step costs more than the input tokens
it saves. Reranking has ample room to pay against uncached frontier input in
this example, but less against inexpensive cached reads. LLM compression costs
about twenty times as much as reranking before its output is counted. A large
reduction in token count can therefore leave the bill higher.

### Compression and caching interact

A cache can reuse a stable prompt prefix without processing its tokens again.
Query-aware compression creates different text when the question or selected
documents change, reducing that opportunity for reuse. A stable compressed
result can be reused, but a new summarisation pass on every request needs to
earn its cost against the alternative of caching the original material.

Compare the complete routes: compression cost plus the reduced answer input,
and the writes, reads and misses of the uncompressed cache. The observed hit
rate matters. One study found compression overtaking caching at around a sixfold
reduction, with a cache-hit plateau near 0.83 on the API it tested.[^124] That
is one provider and one recent study, not a general crossover threshold.

Latency follows the same dependency. A compression call must finish before
the answer model can start reading its result. On a cold cache, reduced
prefill may recover that delay. On a warm cache, the original prompt's
processing might already have been reusable. Measure the serial compression
call as part of end-to-end latency.

<span id="measure-the-right-thing"></span>

### Check the details that survive

A compression ratio describes the size change. Task quality depends on what
remains: the needed facts, their qualifications and the evidence supporting
them. Measure downstream accuracy, claim recall and citation support alongside
the ratio.

An Anthropic cookbook example makes the distinction concrete. A research agent
read eight documents, then compacted its context. All three checked high-level
facts survived, while none of three checked appendix-table details did.[^63]
A summary can preserve the subject of the documents yet lose the numbers a
later question needs. This was one small demonstration, so it motivates a
fidelity check rather than establishing a general loss rate.

<span id="when-to-compress-at-all"></span>

### Decide when compression earns a place

A practical sequence is to reduce avoidable input before transforming useful
input:

1. Fetch material selectively, using search results to choose what needs
   a full read.
2. Extract the body from raw pages, checking for lost content.
3. Rerank and select passages against the task and token budget.
4. Cache the stable part where reuse makes it worthwhile.
5. Test compression for the remaining large, redundant material, especially
   when it changes per request and will go to an expensive model.

Each step needs an answer-quality check. Selection can omit evidence just as
summarisation can. The advantage of this sequence is that the compressor only
processes material still worth sending, and its savings are compared with a
realistic caching alternative.

---

## 9. Caching

An agent often sends the same instructions, tools and earlier messages on
successive requests. Prompt caching reuses the work of reading that repeated
prefix. When much of the prompt stays stable, the read discount can make a
large difference to cost without removing any content.

<span id="how-it-works-in-one-paragraph"></span>

### How prompt caching works

While reading a prompt, a model computes intermediate results for its tokens:
the [KV cache](#g-kv-cache). Prompt caching retains reusable work on the server
for later requests with a matching prefix. Providers differ in whether they
charge for writes, reads or storage, and in how long the cache remains usable.

Cached tokens still occupy the context window. Caching reduces processing and
billing for repeated material; a 50,000-token cached block still takes 50,000
tokens of room alongside the new question and answer.

### The numbers, checked 4 August 2026

The table records the providers' pricing and caching documentation at the
original check date.[^123] Read the columns together: a low read price may
come with a write premium, a minimum prefix size or storage charges.

| Provider | Mechanism | TTL | Minimum | Write | Read |
|---|---|---|---|---|---|
| Anthropic | Explicit breakpoints, max 4 | 5 min or 1 hour | 512–4,096 tokens by model | 1.25× (5 min), 2× (1 hour) | 0.1× |
| OpenAI (GPT-5.6+) | Automatic, optional explicit | 30 min only | 1,024, strict | 1.25× | ~0.1× |
| OpenAI (earlier) | Automatic | 5–10 min idle, max 1 h | 1,024–2,048 | free | varies; gpt-4o only 50% |
| Google Gemini | Implicit by default | not documented | 2,048–4,096 | free | ~0.1× |
| Google explicit | Cache objects | settable | as above | free per token | plus hourly storage |

TTL is the *time to live*: the period during which an entry remains eligible
for reuse. With Anthropic's five-minute cache, one write and one read cost
`1.25 + 0.1 = 1.35` times the base input price, compared with `2.0` for two
uncached requests. The one-hour cache needs two reads to recover its higher
write premium.

Google's explicit caching adds hourly storage. At the recorded rate for Gemini
3.1 Pro Preview, holding 200,000 tokens for an hour cost $0.90 before any read.
That makes the expected number and timing of requests part of the decision;
retaining a rarely reused cache can outweigh its read savings.[^123]

Minimum prefix sizes also vary by model. Anthropic processes requests below
the minimum without caching and returns no error.[^64] Check the relevant
model's threshold and inspect the cache counters instead of assuming that a
cache request succeeded. The prices and limits are a dated snapshot; check them before using the
calculation for a deployment.

<span id="the-tension-with-everything-else-in-this-guide"></span>

### Build a stable prefix

A cache matches an exact request prefix. Changing content before a breakpoint
prevents reuse at or after that change, even if the new wording means the same
thing.[^123] Arrange stable material before volatile material:

```
tools (frozen)  →  system prompt (no timestamps)  →  cached history
                →  [BREAKPOINT]  →  retrieved evidence, this turn's question
```

Retrieved evidence usually changes with the query, so it belongs after the
last stable breakpoint unless you have observed reuse. Repeated jobs can be
different: recurring evidence sequences may form reusable prefixes, as in the
ordering study in chapter 7. Measure that reuse before deciding how to arrange
the retrieved blocks.

This is a real trade-off with relevance ordering. Reordering the same passages
might slightly improve reading on one task while invalidating an otherwise
useful prefix. Compare both answer quality and cache behaviour.

### What mis-ordering costs

Consider a 50,000-token evidence block on the recorded Opus 5 prices. The same
content has three costs depending on whether the server can reuse it:

| Request state | Cost for the 50,000-token block |
|---|---|
| Warm cache read | $0.025 |
| Uncached input after the breakpoint | $0.25 |
| Five-minute cache write | $0.3125 |

A changed prefix that forces a write on every request costs more than ordinary
uncached input, because each request pays the write premium. In this example,
that is about $288 more per thousand requests than warm reads, and 25% more
than uncached input. For a deployment estimate, include the initial write,
later reads, expiry and the measured hit rate.

<span id="what-silently-invalidates-a-prefix"></span>

### Find why a prefix changed

Many cache misses begin with small changes unrelated to the main task. Inspect
the prefix for timestamps, request IDs, UUIDs, per-user data, conditional
sections and tool sets that vary between requests. Also check whether JSON
serialisation or iteration over an unordered collection changes the order of
otherwise identical content.

Two provider-specific behaviours deserve separate checks:

- Anthropic's breakpoints look back at most twenty content blocks. If a turn
  adds thirty tool calls, the next request can miss the earlier reusable
  prefix. Intermediate breakpoints, placed about every fifteen blocks in the
  documented pattern, help within the four-breakpoint limit.
- A cache entry becomes available only after the first response begins.
  Launching identical requests simultaneously can therefore make each pay
  for fresh processing. When the workflow permits it, starting one request
  and waiting for its first token lets later requests reuse the entry.

Anthropic's diagnostics beta reports the earliest divergence with labels such
as `system_changed`, `tools_changed`, `messages_changed` and
`model_changed`.[^65] That helps distinguish a content change from a model or
tool configuration change.

<span id="what-you-cannot-touch"></span>

### What requires control of the server

Research on KV-cache eviction, attention sinks, compression and quantisation
works below the hosted API boundary. StreamingLLM, H2O and SnapKV require
control of the inference implementation.[^66] A caller of a hosted API can
arrange prompts and use the exposed cache features, but cannot substitute
those internal algorithms.

For self-hosting, the August 2026 tooling check found automatic prefix caching
enabled by default in vLLM and SGLang's RadixAttention providing configurable
eviction. Compare performance against a well-configured server. Large speedup
multipliers from early KV-cache papers used older models and baselines and do
not predict the gain over those newer configurations.

---

## 10. Agent memory

A long-running agent needs to remember decisions, recover earlier evidence
and avoid rereading everything on each turn. The transcript alone is a costly
way to do that: it grows continuously and contains both useful state and the
intermediate work that produced it.

<span id="where-the-tokens-actually-go"></span>

### Separate fixed overhead from growing history

In the session breakdown used earlier, listed instructions, tools, memory
files and skills totalled about 22,600 tokens, while messages occupied about
185,400.[^8] The source's broader grouping put fixed overhead around 28,000;
under either grouping, conversation and tool results dominated.

This gives you two maintenance jobs. Deferred loading reduces definitions that
would otherwise be present before work begins. Managing history controls the
tool results and messages that accumulate during work. Solving the first does
not stop the second from growing.

<span id="the-four-verbs"></span>

### Four operations for memory

LangChain organises context management around four verbs:[^67]

- **Write:** store information outside the window, in files, memory records
  or scratchpads.
- **Select:** bring back the parts needed for the current task.
- **Compress:** summarise or trim material that remains in context.
- **Isolate:** give separate work its own window.

These operations can be combined. An agent might write a progress record,
select the relevant source files on resumption and use a separate context for
a bounded research task. The design question is what each operation must
preserve for the next step to succeed.

### Compaction, and what it loses

[Compaction](#g-compaction) replaces a long conversation with a summary. It
creates room to continue, but the summary becomes a dependency: information
omitted from it may no longer be available to the model.

Anthropic's server-side implementation, as documented in August 2026,
triggered by default at 150,000 input tokens, with a minimum trigger of 50,000.
Its documentation states that raw history is permanently discarded after
compaction.[^68] The small fidelity example in chapter 8—high-level facts
preserved, appendix details lost—shows the kind of omission to test for.[^63]

Claude Code's documented sequence clears older tool outputs before resorting
to summarisation. Clearing is mechanical and requires no inference; compaction
adds a model pass. That ordering can remove expendable bulk before asking a
summary to preserve the remaining state.

Survival also depends on where instructions came from. In August 2026,
Claude Code re-injected the system prompt and project-root instructions from
disk. Path-scoped rules and nested instruction files were unavailable until a
matching file was read again. Invoked skills were re-injected with per-skill
and total caps of 5,000 and 25,000 tokens respectively, truncated from the end.
These details make the placement and reload mechanism for critical instructions
worth checking.

For work across sessions, keep explicit handoff artefacts such as a progress
file, task list and git history. Anthropic's long-running-agent guidance found
compaction insufficient by itself for preserving instructions across
sessions.[^69] A durable record lets the next session read the current state
directly instead of reconstructing it from a compressed transcript.

### Sub-agents

A sub-agent can read widely in its own window and return a short result to the
main agent. The main context then carries the conclusion instead of every
intermediate tool response.[^70] One documented Claude Code example read
three files and returned 420 tokens, with those file reads staying outside the
main window.

The handoff is selective by design. Give the sub-agent a bounded task and
consider which details the parent will need from its answer. A short report
saves context only if it retains enough information for the next decision.

<span id="the-multi-agent-argument-fairly"></span>

### Match isolation to the work

Independent reading tasks are easier to split than changes with shared
dependencies. Several agents can investigate different sources and report
back; agents editing connected parts of a system also need to agree on the
decisions their edits imply.

Anthropic's internal research evaluation found a substantial gain from an
orchestrator with parallel sub-agents, at substantially higher token use.[^71]
Its stated fit was work with heavy parallel reading, information beyond one
window and many complex tools. It explicitly cautioned about tasks needing
identical shared context, many dependencies and much coding work.

Cognition described the coordination problem through agents building
incompatible halves of the same game.[^72] Its later guidance favoured
additional agents contributing analysis while writes remained single-threaded,
including review from a fresh context.[^73] These accounts support a practical
starting point: isolate read-heavy work with clear boundaries and keep a
single owner for interdependent writes.

Architecture is only one explanation for a measured gain. Anthropic's own
analysis found token use explained much of the performance variation in one
benchmark. A separate comparison found single agents matching or beating
multi-agent systems under matched thinking-token budgets.[^74] Compare
systems at similar budgets before attributing the difference to delegation.

Failure analysis reinforces the importance of the surrounding design. A study
of multi-agent traces found substantial failures attributable to specification
and system-design issues.[^75] More agents create more handoffs whose
requirements need to be clear.

### Progressive disclosure

Skills, deferred tools and just-in-time retrieval share one pattern: keep a
short pointer in the window and load full content when it becomes relevant.
An available skill might cost about a hundred tokens; loading all its
instructions in advance would consume their full size on every request.[^9]

Progressive disclosure shifts the question from “what might ever be useful?”
to “what does this step need?” It works best when the pointer gives the model
enough information to recognise when to load the detail.

### Keep tools narrow and let the caller orchestrate

A narrow tool runs a query, applies light scoring or returns evidence through
stable inputs and outputs. The caller decides which tool to use next and how
to combine results.[^117] Keeping those decisions above the tools allows the
same primitives to support different workflows.

The production example in the appendix uses one retrieval layer for both an
agent that chooses its own sequence and a fixed plan-execute-synthesise web
pipeline.[^115] A model call hidden inside every tool invocation would add
cost and delay whether that reasoning was needed or not.

Narrow tools can create a large catalogue, bringing back chapter 2's selection
problem. Deferred loading helps reconcile the two goals: provide specific
capabilities, while presenting a relevant subset of their definitions at each
step. Clear names and descriptions still matter once that subset is loaded.

<span id="memory-benchmarks-are-not-trustworthy-yet"></span>

### Evaluate memory against simple storage

A memory layer earns its place by recovering useful information at acceptable
cost. Compare it with the full conversation and with simple file storage
before comparing it only with other memory products.

On LoCoMo, the full-context baseline in one commercial memory paper scored
about 73%, above the system's roughly 68%.[^76] Letta separately reported 74%
using conversation histories stored in a file.[^77] These results show that
the benchmark allowed simple approaches to perform well; they do not settle
how memory will behave on your longer or more selective tasks.

Configuration also changed reported rankings. A vendor dispute produced
substantially different scores for the same system after changes to the
harness and inclusion of a disputed category.[^78] A controlled study found
that changing only the embedding model could reverse the conclusion about
which memory approach performed better.[^79]

These results make LoCoMo a weak discriminator between memory approaches.
LongMemEval offers harder tasks, and MemoryAgentBench includes selective
forgetting. Whatever suite you choose, hold the surrounding pipeline fixed,
include simple baselines and measure the outcomes your own agent needs:
recovering a past decision, retaining a detail or leaving obsolete information
out of the next prompt.

---

## 11. Proving a change helped

Suppose a reranker reduces the prompt by half and the next answer looks better.
That is a promising example, but it leaves several possibilities open. The
reranker may have selected better evidence, the model may have had a good run,
or the question may simply have been easy. An evaluation needs to separate
those explanations well enough to support the decision you are making.

Start by naming that decision: whether to adopt the reranker, how much quality
loss a saving can justify, or whether a memory strategy retains needed details.
Then choose comparisons and measurements that can answer it.

<span id="always-include-the-boring-baseline"></span>

### Include the simplest useful baseline

A complex component can look good against its peers while adding little to a
simple pipeline. Chapter 10's memory results illustrate this: full context and
plain file storage performed well against specialised memory systems on
LoCoMo.[^76][^77]

For retrieval, compare with no retrieval and with a simple single-provider
top-k pipeline. For memory, include the full transcript where it fits and
simple storage with retrieval. Keep costs beside quality. A component that
matches a baseline more cheaply may still be useful; one that adds cost needs
a corresponding benefit.

<span id="how-many-test-cases-you-actually-need"></span>

### Plan enough test cases for the decision

An evaluation score estimates performance on a wider population of tasks. A
small sample can move several points simply because different questions were
included. For a pass/fail score, the standard error is
`√(s̄(1−s̄)/n)`, where `s̄` is the observed pass fraction and `n` the number
of independent items. That uncertainty can be as large as the improvement you
hope to measure.[^88]

Plan around the smallest effect you would act on. The figures below illustrate
the scale of the problem; they depend on the task variation, test design and
assumptions of their sources.

| Decision | Illustrative sample requirement | Basis |
|---|---|---|
| Detect a 3-point change in item-level scores | About 1,000 items | Power analysis[^88] |
| Detect a 10-point change in agent pass rates | About 120–200 tasks | Analysis of an agent evaluation[^89] |
| Obtain a stable majority verdict from an LLM judge | About 11 repeated trials | Measured judge variability[^90] |

Repeating a task measures how consistently the system handles that task.
Adding tasks measures how performance varies across the work you care about.
These address different uncertainty. The cited analyses found diminishing
returns from repeats, with task variation especially important for agents.[^88][^89]
When a suite is too small to detect the intended effect, additional distinct
tasks may help more than another round over the same few questions.

Lowering temperature is not a substitute for a better experiment. It changes
the output distribution and can introduce bias or move variation into parts
that repeats cannot reduce.[^88] Evaluate the settings the system will use.

Questions drawn from the same source are also related. If ten questions all
come from one document, treating them as ten independent observations can
understate uncertainty. Group, or *cluster*, the error calculation by the
shared document, repository or other source. One worked comparison found the
naive error estimate more than three times too small.[^88]

<span id="change-one-thing--and-the-model-is-more-than-the-model-name"></span>

### Hold the surrounding system constant

Changing an embedder alongside a memory method makes it difficult to tell
which caused the gain. One controlled study found that the embedder alone
could move accuracy enough to reverse the ranking of memory approaches.[^79]
Vary one component in each comparison and retain the configuration of the rest.

“The same model” needs more detail than a display name:

- Temperature zero does not guarantee identical outputs. An inference study
  found different completions of the same prompt because numerical behaviour
  varied with server batching.[^91]
- Hardware, quantisation and inference backends can shift benchmark
  scores.[^92]
- Prompt formatting can change performance substantially, including model
  rankings.[^93]

Fix the model identifier, prompt template, decoding settings, reasoning effort
and evaluator version. Where you control the server, fix its backend and
hardware class too. Where the provider hides those details, record the limits
of your control and interleave configurations in time so changing load or
service behaviour is less likely to favour one arm.

<span id="one-run-tells-you-nothing"></span>

### Measure consistency across runs

Two configurations can have similar average accuracy but different patterns
of failure. One may usually succeed on each task; another may alternate
between success and a serious mistake. Users experience those repeated
attempts, not just the mean across a suite.

τ-bench's `pass^k` counts success on all k attempts and makes that distinction
visible.[^33] The multi-turn study in chapter 2 also found that growing
unreliability contributed substantially to the loss from splitting a task
across turns.[^23]

Repeat enough items to estimate run-to-run variation, report the spread and
inspect differences smaller than that variation cautiously. Combine this with
a sufficiently varied task set; repeated attempts cannot supply missing task
coverage.

<span id="a-strong-published-context-ablation-found-nothing"></span>

### Learn from a context change that did not help

Standing context files are meant to supply repository knowledge an agent
would otherwise need to discover. Whether they help depends partly on whether
missing knowledge is causing the failures.

A study comparing `AGENTS.md`-style instructions across repositories and two
agents found no correctness benefit from the tested context strategies.[^89]
It used paired, task-aware statistical comparisons and inspected near-miss
tasks. Failure analysis suggested that agents were struggling with
implementation rather than lacking the repository knowledge the files
supplied. Adding that knowledge did not address the observed failure.

The study also makes a useful distinction between a null finding and evidence
of equivalence. With its small task set, the minimum effect it could reliably
detect remained large. A modest benefit or harm could therefore go unnoticed.
Its power analysis estimated 120–200 tasks to detect a ten-point effect.[^89]

For your own change, inspect whether it reached the intended stage and whether
that stage was the source of the problem. If an extractor never saw the pages
that failed, an extraction change cannot explain their outcome. If a context
file supplied facts the agent already knew, a null result says little about
cases where those facts are missing. Report what the experiment could detect,
including the uncertainty around an apparent lack of difference.

<span id="measure-whether-the-evidence-was-there-at-all"></span>

### Separate missing evidence from failed use

When an answer is wrong, first ask whether the supplied context contained
enough information to answer. If it did not, acquisition or selection needs
attention. If it did, the reader or prompt may need attention. A final accuracy
score combines both failures.

Google's *Sufficient Context* work labels question-context pairs according to
whether the material is enough to answer definitively.[^94] Its automated
rater was accurate enough in its own evaluation to explore that distinction,
though it remains a vendor-evaluated instrument rather than a perfect label.

Two results show why the distinction matters. Models sometimes answered
correctly despite insufficient supplied context, so a correct answer did not
prove successful retrieval. Conversely, one model hallucinated much more with
insufficient context than with no context.[^94] Partial evidence can encourage
an answer it cannot support.

Include a sufficiency check and a path for recognising gaps. “Some related
material was found” should not be treated as equivalent to “the evidence
needed for this answer is present”.

<span id="do-not-trust-a-single-benchmark"></span>

### Use benchmarks as proxies, then check the outcome

A benchmark is useful when success on it predicts success at your task.
HELMET's weak correlations across long-context tasks show why that relationship
needs checking.[^29] A retrieval test, a synthesis test and a state-tracking
test need not rank systems alike.

Choose the closest available tasks, then compare their scores with the real
outcome on a held-out sample. If an improved retrieval metric leaves answer
quality unchanged, inspect the link between the two before continuing to
optimise the metric.

<span id="the-judge-is-a-component-not-an-oracle"></span>

### Evaluate the judge as part of the pipeline

Many context evaluations use a model to score another model's answer. That
makes the judge a component with its own errors and configuration, much like
the retriever or reader. The memory-benchmark dispute in chapter 10 illustrates
how harness choices can change reported results.[^78]

A judge may also answer a narrower question than you intend. An answer can
faithfully follow a retrieved document while that document is wrong, stale or
inappropriate. Grounding in the supplied text does not validate the source.

Automatic fact checkers have measurable error rates. In the aggregate
leaderboard discussed here, performance was much weaker on long-form expert
answers than across the overall set.[^82] Use those tools with calibration
against the kinds of claims and sources your system produces.

<span id="automatic-metrics-are-validated-for-comparing-systems-not-judging-answers"></span>

### Distinguish system comparisons from individual verdicts

A metric may rank whole systems well while often misjudging individual
answers. Averaging across many examples can cancel errors that remain serious
when one score determines whether to release one answer.

AutoAIS, a standard automatic attribution metric, reported strong correlation
with human judgement at system level, alongside much lower and more variable
agreement on individual examples.[^81] That supports using it to compare
configurations over a suite, with suitable calibration. It does not establish
that a threshold on one answer is a reliable production gate.

Choose the use of a metric at the level for which it has evidence. If you need
to act on individual outputs, validate that decision separately, including
which kinds of errors the metric misses.

<span id="entailment-scoring-can-be-cheap-enough-to-run-on-everything"></span>

### Use inexpensive scoring where it is valid

A fine-tuned entailment checker can judge whether a claim follows from a
passage at far lower cost than a frontier model. MiniCheck's comparison scored
about 13,000 claim-document pairs for $0.24, against $107 for GPT-4 at
comparable balanced accuracy.[^82]

That cost difference can make it practical to score every response in both
arms of an experiment. It does not remove the need to validate the checker or
make every other metric equally cheap. Treat scoring cost and scoring quality
as separate parts of the evaluation design.

<span id="faithfulness-alone-rewards-evasion"></span>

### Measure faithfulness alongside coverage

A system that answers less can make fewer unsupported claims. If the only
metric is faithfulness, a change that causes evasive or incomplete answers can
look like an improvement.

Vectara's hallucination leaderboard illustrates the need to inspect answer
rates alongside low hallucination rates.[^83] The vendor supplies the detector,
whose own errors also limit the precision of absolute scores.

Pair faithfulness—whether claims follow from the evidence—with coverage—
whether the answer contains the facts needed for the task. An appropriate
abstention when evidence is missing is different from omitting facts the
system could have supported. Inspect both so a shorter answer does not earn
credit merely for making fewer checkable claims.

<span id="citations-resolve-far-better-than-they-support"></span>

### Check what a citation supports

A working link, a relevant page and a supporting passage are three different
checks. A page may discuss the right subject while supplying no evidence for
the sentence that cites it.

An evaluation of research reports across fourteen models measured these
separately:[^84]

| Citation check | Reported result |
|---|---|
| Link resolves | Over 94% for frontier models |
| Linked page is relevant | Over 80% |
| Linked page supports the claim | 39–77% |

Link validity alone would therefore give a much more favourable picture than
claim support. The same work found support accuracy falling as tool calls
increased. More retrieval can expand coverage while making attribution harder.

A deep-research benchmark also found a trade-off between precision per
citation and the number of effective citations.[^85] Report both support and
coverage, and trace claims to passages where possible. A larger bibliography
is not itself a measure of a better-supported answer.

<span id="a-bias-that-penalises-synthesis"></span>

### Check for bias against synthesis

An answer that combines distant passages or paraphrases them heavily can be
harder for an automatic checker to recognise as supported. An evaluation of
factuality metrics found both kinds of bias.[^86]

This matters directly to context optimisation. A change that enables broader
synthesis may score worse even when its conclusions are supported. Calibrate
the metric on a human-annotated sample containing the kinds of paraphrase and
cross-document reasoning you want to enable, rather than assuming performance
on local quotation transfers.

<span id="prose-detectors-do-not-transfer-to-tool-output"></span>

### Match detectors to the source format

Code, tool results and structured records differ from the prose used to train
many grounding detectors. In one study, a prose-trained detector performed
poorly on code-agent sources, while a detector fine-tuned on that material
performed substantially better.[^87]

That single study does not establish how every detector transfers. It does
show why source format belongs in validation: test the checker on code and
tool output before using its scores to decide whether a context change helped
an agent working with those inputs.

<span id="metrics-and-what-each-one-misses"></span>

### Choose metrics with their omissions in view

No single metric covers the journey from selecting a source to producing a
useful, supported answer. Use the table to locate the question each one asks:

| Metric | Measures | Leaves open |
|---|---|---|
| Recall@k | Whether required material appears in the selected k items | Whether it survives packing and is used |
| Precision@k | What share of selected material is useful | Whether that material is sufficient |
| nDCG@10 | Whether relevant items rank near the top | Token cost and how the reader responds to order |
| Sufficient context | Whether the context contains enough to answer | Whether the model answers correctly |
| Retrieval failure rate at k | How often needed evidence is absent from the final set | Whether present evidence is interpreted correctly |
| Faithfulness / groundedness | Whether the answer follows from the supplied text | Whether the source is correct, current or sufficient |
| Claim recall | Whether required facts survive | Answer form and unsupported additional claims |
| Citation support | Whether a cited passage supports its claim | Source quality and later changes to the page |
| Answer accuracy | Whether the answer is right | How it was reached, its cost and its reliability |

Choose the unit deliberately. Document recall counts a source as present even
if the useful words were cut from it. An applied study found word-level recall
correlated much better with human answer scores than document-level recall.[^95]
The same metric name can conceal a substantial difference in what is counted.

Validate a proposed metric across at least two systems as well. Correlation
within one system may mainly reflect which questions are easy, rather than
whether the metric distinguishes better and worse systems.[^95]

<span id="measure-cost-because-most-leaderboards-omit-it"></span>

### Put cost and latency beside quality

The Terminal-Bench example in chapter 3 showed similar accuracy at a 3.7-fold
cost difference.[^39] Without cost, the practical difference between those
configurations is hidden.

Report accuracy alongside cost per correct answer and latency, including
failed attempts and retries. A context change may be worth adopting because it
preserves quality while reducing spend or delay. A quality gain may also be
too expensive for the intended workload. Both decisions need the complete
route's costs.

<span id="contamination-is-not-auditable"></span>

### Protect the value of held-out tasks

Public tasks may overlap with training material, and contamination detectors
cannot reliably establish that they do not.[^35][^36] Supplement public
benchmarks with private tasks, material known to post-date a training cut-off
where that date is available, or refreshed evaluations.

Keep the held-out set for the final decision rather than repeatedly tuning
against it. Record the provenance of its inputs and replace it when repeated
use has turned it into part of the development process.

<span id="a-checklist"></span>

### A practical evaluation checklist

The checks below follow the experiment from design to deployment. Their scale
should match the decision: an exploratory result can guide the next test, but
should not carry the certainty of a well-powered comparison.

1. Write down the decision rule first: the minimum useful effect, primary
   metric and statistical power needed.
2. Size the task set from that effect. If it is too small, report the
   uncertainty and detectable-effect limit rather than claiming equivalence.
3. Keep always-pass and always-fail tasks in headline performance. Any
   separate comparative analysis should be declared before seeing results.
4. Inspect a few intended interventions and near-misses. Check that the change
   reaches the relevant stage and addresses the failure you mean to test.
5. Fix and record model, serving configuration where available, decoding,
   reasoning effort, evaluator version and prompt template.
6. Include a do-nothing baseline and a simple alternative.
7. Vary one component per comparison.
8. Interleave configurations so time, service updates and load do not align
   with one arm.
9. Use repeats to estimate variability. Four to six is a starting point from
   the cited analysis; adding tasks may be more valuable than further repeats.
   Keep the intended temperature setting.
10. Analyse paired per-item differences and cluster errors by the shared source.
11. Report `pass^k` when repeated success matters to the workflow.
12. Score faithfulness and coverage together, including appropriate abstention.
13. Score every response where the chosen validated metric is affordable;
    otherwise record the sampling design and its uncertainty.
14. Check citation support separately from link resolution and page relevance.
15. Calibrate automatic metrics against human-labelled examples from your data.
    Size that sample for class prevalence and tolerated uncertainty; report
    sensitivity, specificity and confidence intervals.
16. Record cost and latency for each configuration.
17. Record missing evidence as well as the evidence that scored.
18. Retain retries, refusals and failures in the evaluation record.
19. Keep raw provider responses where retention is permitted, so the run can be
    inspected and replayed. Record any retention limits and the gaps they leave.
20. Check the evaluator against the real outcome on held-out data; validate new
    metrics across at least two systems.
21. Supplement public benchmarks with private tasks and reserve the holdout for
    the final decision.
22. Preserve the comparison systems' configurations as well as your own.
23. Follow deployment with an online test designed to remain valid under
    repeated checking. Repeatedly applying a fixed-horizon significance test
    to an accumulating dashboard can create false positives.[^96]

---

<span id="12-what-nobody-knows-yet"></span>

## 12. Where local measurement still matters

The techniques in this guide give you ways to inspect and change a context
pipeline. They do not supply a universal token budget, tool limit or retention
percentage. Some useful questions remain open, and several common rules of
thumb go beyond the measurements behind them.

<span id="claims-that-are-repeated-and-unsupported"></span>

### Treat rules of thumb as hypotheses

A few examples recur across the chapters:

- **Effective window size:** no traceable basis was found for a universal
  60–70% of advertised length. Effective length depends on the task and model.
- **Token density:** code was 20–40% denser than prose in the nine-file sample,
  which cannot establish a ratio for other repositories or tokenisers.
- **HTML overhead:** stripping machinery removed 87–99% of tokens from four
  pages. That is a different measurement from the unsupported claim that a
  typical page is 80% boilerplate text.
- **Helpful random documents:** a replication traced the reported benefit to
  prompting and generation constraints, rather than a transferable advantage
  from noise.[^27]
- **False-fact propagation:** the poisoning terminology is useful, but the
  August 2026 source check did not establish a controlled propagation result
  behind the anecdote discussed in chapter 2.[^25]
- **Tool-count limits:** the reported point where one catalogue's selection
  performance deteriorated was specific to that catalogue.[^21] Tool names,
  schemas, ambiguity and the model all affect the problem.

Use such numbers to choose an initial experiment, then measure the relevant
quantity in your system. Their value is in helping frame the question, not in
removing the need to ask it locally.

<span id="things-you-will-have-to-measure-yourself"></span>

### Measure the parts of source use that scores omit

A final answer score leaves much of acquisition unexplained. Your evaluation
may need to establish whether the chosen source is authoritative, whether a
citation supports the exact claim and whether the evidence is still current.
These are distinct from finding related text.

The same applies to gaps and conflicts. A system needs a way to recognise that
it lacks enough reliable evidence, and to handle two sources that disagree.
The benchmarks discussed here cover these unevenly: RECON begins to examine source
conflict,[^40] while chapter 4's provider comparison measures host overlap
without settling which source should prevail.

Retrieved web content also arrives as [untrusted input](untrusted-context.md).
Ordinary relevance
scores do not establish that following instructions embedded in that content
is appropriate. Include the source-selection, attribution, freshness,
abstention and input-handling decisions that matter to your application,
rather than assuming answer accuracy covers them.

<span id="things-that-might-be-true-and-need-checking"></span>

### Results worth testing in other settings

Several findings suggest useful experiments without yet supporting broad
policies.

One corpus-scale study found plain keyword search overtaking an agentic
searcher above roughly ten million corpus tokens, at much lower query-token
cost.[^41] That is one team's corpus construction; replication would help
establish whether the result concerns search policy, corpus scale or the
particular task design.

The tool-catalogue study left about ten points of selection loss even with a
perfect shortlister.[^21] If that pattern transfers, part of the remedy lies in
clearer descriptions and disambiguation after retrieval. Test whether the
correct tool is missing from the shortlist or present but not chosen.

Long-context degradation also varied substantially across models and failure
types.[^19] A policy fitted to one model needs checking when the reader changes.
And the small compaction example preserved broad facts while losing table
details,[^63] leaving open how to measure fidelity across different tasks and
longer runs.

<span id="how-to-read-anything-in-this-area"></span>

### Carry the method into the next experiment

When a new technique appears, connect its reported result to the decision in
front of you:

1. Identify the material and task it was tested on. Repeated examples and
   retrieved web pages have different opportunities for compression.
2. Keep the model and date with the result. An older finding can explain a
   failure mode without predicting its size on a newer model.
3. Inspect the baseline. Full context, simple file storage or one search call
   may be the comparison that matters.
4. Check who configured the experiment and whether its details are available,
   especially in vendor comparisons.
5. Check whether the benchmark still separates systems and whether it predicts
   the outcome you need.
6. Read follow-up work for changes in setup, narrower results and replication.

Then make one bounded change, preserve the evidence needed to explain it and
compare the complete answer path. A useful context policy is one you can
connect to better answers, lower cost or more reliable behaviour on the work
it is meant to support.

---

## Glossary

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
more input, even when the extra input adds no reasoning requirements.

<span id="g-cross-encoder"></span>**Cross-encoder** — a model that reads the
question and one candidate passage together and scores how well they match.
Used over a shortlist because scoring every query-passage pair across a
whole collection would be expensive.

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
piece of text, arranged so that similar texts have nearby representations.

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

**MCP (Model Context Protocol)** — a standard for connecting tools and other
resources to an agent. Tool definitions occupy context when loaded.

**MMR (maximal marginal relevance)** — a selection rule that balances
relevance to the query against similarity to items already selected.

**nDCG at k** — a ranking score that gives more credit when relevant results
appear near the top of the first k positions.

**Oracle** — an idealised condition supplied with information a real system
would have to discover, used to separate that discovery step from later work.

<span id="g-niah"></span>**Needle in a haystack** — a test where one specific
fact is hidden inside a large body of unrelated text and the model is asked to
find it.

<span id="g-parent-document"></span>**Parent-document retrieval** — matching on
small pieces because they match precisely, then handing the model the larger
section each small piece came from.

**pass^k** — the fraction of tasks a system gets right on every one of k
separate attempts, rather than on its best attempt.

**Prefill** — the phase in which a model processes input before generating
output. Reusable cached work can reduce the processing needed.

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

Cerebras' account of its internal knowledge base brings the earlier techniques
together in one pipeline.[^115] It serves questions across data-centre
operations, chip design, hardware, training, inference and cloud. People,
automations and agents use the same underlying sources.

The sequence shows where each decision happens:

```
sources          Slack, wiki, code, incidents, team databases
   |             one connector per source, one row shape
distillation     LLM rewrites each item into a canonical record       -> ch 5
   |             filtered by term rarity, length, reactions           -> ch 5
index            a single embeddings table, any source queryable
   |
scope            default project narrows the corpus                  -> ch 7
   |
retrieve         six lists in parallel: lexical, semantic, per-source -> ch 4
   |
fuse             reciprocal rank fusion, k=60, combines the lists     -> ch 4
   |
rerank           small model scores merged candidates, keep 10        -> ch 6
   |
expand           pull neighbouring sections into the selected text   -> ch 6
   |
synthesise       answer with citations
```

Filtering runs before indexing, and project scoping narrows the pool before
retrieval. Both reduce the work passed to later stages. Expansion happens
after ranking, so the ranker compares focused chunks while the answer model
receives more complete passages.

The same primitives support two ways of coordinating the work. Agents call
tools and choose their own sequence; the web interface follows a fixed
plan-execute-synthesise pipeline. This is chapter 10's separation between
retrieval operations and the caller's decisions.

### What it cannot tell you

The account reports use at about 15,000 questions a day three months after
launch, but gives no retrieval-quality scores, simpler-baseline comparison or
evaluation method. That usage describes adoption. It cannot identify how much
the rewriting, filters, parallel searches or reranker each contribute.

Use the account to understand how the stages can fit together. To decide
whether the same complexity belongs in another system, compare individual
stages against a simpler pipeline as chapter 11 describes.

## Appendix: tooling, checked 4 August 2026

This snapshot records GitHub and package-registry checks made on 4 August
2026. Maintenance status can change, so check the current project before
adopting it. The distinctions below—converter versus extractor, reference
implementation versus maintained package, legacy server versus replacement—
remain useful questions to ask.

### How to check whether a project is alive

A recent `pushed_at` timestamp can reflect a README edit or dependency bot.
Commit participation gives a longer view: GitHub's
`repos/OWNER/REPO/stats/participation` endpoint returns weekly commit counts.
Read those alongside the README and release history to understand where active
development is happening.

| Project | Commits, 52 weeks | Last 13 weeks | Last 4 weeks |
|---|---|---|---|
| cognee | 6,252 | 1,665 | 383 |
| mem0 | 837 | 384 | 103 |
| graphiti | 394 | 110 | 51 |
| langmem | 52 | 24 | 7 |
| letta | 2,542 | 6 | 2 |

Letta illustrates why the time window matters. Its high annual count concealed
much lower recent activity, and its README identified the repository as the
legacy server and directed users elsewhere. The organisation's active
repositories concerned a TypeScript coding agent. A package install and a
project name alone would not reveal that change of focus.

### Picks by stage

These are candidates and cautions from the dated snapshot, to be considered
alongside the relevant chapter's evaluation:

| Stage | Candidates | What to inspect |
|---|---|---|
| Extraction | trafilatura (Apache-2.0); Readability where its output fits the pages | MarkItDown converts HTML but does not remove body-external boilerplate. Check recall for the exact extractor and source types; chapter 5 |
| Chunking | Recursive splitting at 200–400 tokens; Chonkie for late chunking; semchunk for token-exact splits | `semantic-chunkers` had no commit or release since June 2025. Jina's reference implementation was unchanged since December 2024, while the technique remained available through an embeddings API flag |
| Compression | LLMLingua (MIT), after measuring the need | One commit since October 2025; measure the ratio and fidelity on your own text |
| Memory | Graphiti, Mem0, Cognee | Compare with full context and file storage; include the dependency footprint and maintenance findings below |
| Retrieval frameworks | Haystack 3.0, LangChain, LlamaIndex, txtai, DSPy | R2R had no commit since November 2025; Verba and Cognita were archived |
| Serving | vLLM, SGLang | vLLM's source enabled prefix caching by default despite prose describing opt-in; SGLang exposed eviction controls. Archived HuggingFace TGI directed users to vLLM, SGLang or llama.cpp |
| Evaluation | Inspect AI for clustered errors, bootstrap intervals and epoch reducers; promptfoo for declarative comparisons and CI; DeepEval for metrics; Langfuse or Phoenix for traces | Confirm the statistical methods and maintenance you need rather than choosing by the size of the metric catalogue |

Dependency counts help estimate the installation footprint of a memory system,
though they do not measure its quality. The same snapshot recorded:

| Package | Transitive dependencies | Other recorded detail |
|---|---|---|
| Graphiti | 27 | Smaller dependency set among the compared memory packages |
| Mem0 | 33 | — |
| Cognee | 129 | 26 MB package footprint |
| Letta | 247 | Maintainers labelled the server legacy |
| LangChain | 36 | Three direct dependencies |

LangMem's recent commits were dependency-bot changes. Among evaluation tools,
ARES had no commit since March 2025; OpenAI Evals' last release was May 2024
and its README pointed to a hosted product; Deepchecks' last LLM release was
December 2024. RAGAS had moved organisations and had not released since early
2026. These are maintenance observations in August 2026, not claims about
their status today.

For extraction, the earlier notes also recorded Readability recall of 0.729 in
one benchmark run, distinct from the JavaScript result in chapter 5. The
figures need their implementation and run attached before comparison.
`html2text` was GPL-3.0 with little activity; its extraction score appears in
chapter 5.

Read licence files as well as repository badges. The snapshot found Morphik
under BSL 1.1, `context-mode` under Elastic License 2.0, Dify and holaOS under
modified Apache-2.0 terms with multi-tenancy restrictions, and OpenViking and
chunkr under AGPL-3.0. Other repositories marked `NOASSERTION` by GitHub
contained Apache-2.0 or MIT licences. The badge alone did not explain the terms.

Stars indicate attention rather than maintenance or fitness for the task.
Compare activity over time, the maintainers' own status notes and the component's
behaviour in your pipeline before relying on popularity.

---

## About this guide

The authors build software that decides which content an AI agent may take
in and keeps a record of what it took in, including a plugin for Anthropic's
Claude Code. Anthropic accounts for about one citation in six; where an
Anthropic post evaluates an Anthropic technique, the text marks it as a
vendor evaluating itself. Anthropic's API documentation is treated as
authoritative for the behaviour of its own products.

Three measurements are the authors' own: token density across nine repository
files, HTML stripping on four pages and search-provider overlap across 24
queries. Each is labelled with its sample size and limitations where it appears.

## Sources

Source checks date from 4 August 2026. Recorded URL checks returned HTTP 200
except where a reference notes an exception, including the Cerebras article
in reference 115. A resolving URL establishes availability; the quoted figures
were also checked against source content rather than automated summaries.
Several vendor results come from internal evaluations with unpublished task
composition, identified where that affects interpretation.

The three original measurements have different scopes:

- Token density used tiktoken `o200k_base` across nine files in this repository.
  The measurement script and repository files are available to rerun.
- HTML stripping used the same tokeniser on four fetched pages, before and
  after removing tags and scripts. The four original HTML inputs were not
  retained, so their published figures cannot be reproduced exactly. Neither
  token measurement transfers exactly to Claude or Gemini tokenisers.
- Provider overlap used 24 English-language queries across six categories,
  sent once to three search providers. Four of 72 calls failed and were
  excluded. The script is `docs/guide/measurements/source_overlap.py`; a new
  run samples changing search results rather than reproducing the snapshot.

In the provider sample, Exa contributed 61% unique hosts and the other two
providers about 26% each. Across 295 distinct hosts, the most frequent were
`reddit.com` (30 appearances), `youtube.com` (24), `arxiv.org` (19),
`medium.com` (19) and `docs.vllm.ai` (19). These describe host coverage, not
independent evidence or source quality.

### References

[^1]: Anthropic, *Context windows*, `platform.claude.com`, fetched 4 August
    2026.
[^2]: Vaswani et al., *Attention Is All You Need*, arXiv:1706.03762, June
    2017, Table 1.
[^3]: Anthropic, *Token counting*, fetched 4 August 2026.
[^4]: Anthropic `claude-api` reference material, bundled 2026.
[^6]: Anthropic, *Advanced tool use*, 24 November 2025.
    Reported connector footprints: GitHub, 35 tools and about 26,000 tokens;
    Slack, 11 tools and about 21,000; five connectors, about 55,000.
    Anthropic also reported seeing 134,000 tokens of definitions. Deferred
    loading reduced one tool set from 77,000 to 8,700 tokens (reported as 85%).
[^7]: `github.com/zhang-liz/mcp-token-benchmark`, 8 July 2026.
[^8]: JD Hodges, published `/context` breakdown, 22 March 2026.
[^9]: Anthropic, *Agent Skills overview*, fetched 4 August 2026.
[^10]: Aurimas Griciūnas, *Agent Skills progressive disclosure*, 11 March
    2026.
    Seventeen official skills: median discovery footprint about 80 tokens;
    approximately 1,700 tokens for the full set.
[^11]: Liu et al., *Lost in the Middle*, TACL, February 2024.
    GPT-3.5-Turbo: about 75.5% with the gold document first, 53% around
    position 10 and 63% at position 20; closed-book 56.1%, gold-only oracle
    88.3%. Claude-1.3 and Claude-1.3-100K: closed-book 48.3% and 48.2%, oracle
    76.1% and 76.4%. Inputs were about 2,000–6,000 tokens. Increasing retrieved
    documents from 20 to 50 gained about 1.5% and 1% for GPT-3.5-Turbo and
    Claude-1.3 respectively.
[^12]: Zhang et al., *Positional Failures in Long-Context LLMs*,
    arXiv:2605.23170, 22 May 2026.
[^13]: Hong, Troynikov and Huber, *Context Rot*, Chroma, 14 July 2025.
    Eighteen models. Repeated-word copying fell from near-100% at 25 words
    to roughly 40–60% at 10,000. A focused ~300-token memory prompt beat the
    same content in ~113,000 tokens by roughly 15–40 points by model family.
[^14]: Levy et al., *Same Task, More Tokens*, arXiv:2402.14848, ACL 2024.
    Average accuracy fell from 0.92 to 0.68, with degradation beginning
    around 3,000 tokens in this setup.
[^15]: Hsieh et al., *RULER*, arXiv:2404.06654, April 2024.
    Only about half the tested models claiming windows of at least 32K
    maintained satisfactory performance at 32K despite strong needle scores.
[^16]: Modarressi et al., *NoLiMa*, arXiv:2502.05167, February 2025.
    The source notes record both 10 of 12 long-window models and 11 of 13
    models falling below half their short-context baseline at 32K. The
    denominators remain unreconciled; the prose therefore gives no exact
    fraction.
[^17]: An et al., *Why Does the Effective Context Length of LLMs Fall
    Short?*, arXiv:2410.18745, October 2024.
    Effective length was near half the training length in the reported
    setup; position-offset remapping recovered more than 10 points on RULER.
[^18]: Epoch AI, *Context windows*, 25 June 2025.
    Tracked 123 models: advertised windows grew about 30× annually; the
    input length at which the best models achieved 80% accuracy improved
    over 250× in nine months.
[^19]: Leng et al., *Long Context RAG Performance of LLMs*,
    arXiv:2411.03538, November 2024.
    About 2,000 experiments. Llama 3.1 405B declined after 32K and
    GPT-4-0125 after 64K; o1-mini, GPT-4o, Claude 3.5 Sonnet and Claude 3 Opus
    improved roughly monotonically to 100K. Claude 3.5 copyright refusals
    rose from 3.7% to 49.5% between 16K and 64K; DBRX instruction-following
    failures rose from 5.2% to 50.4%.
[^20]: Kate et al., *LongFuncEval*, arXiv:2505.10570, April 2025.
    Growing tool definitions from 8K to 120K degraded Mistral-large by
    94%, Llama-3.1-70B by 72% and GPT-4o by 13.8%. Longer tool responses
    degraded Mistral-large by 91% and GPT-4o by 7%; longer conversations cost
    13–40%. Tool-result accuracy at position 8 exceeded position 1 by 5–75%.
[^21]: Gillespie and Perry, *Scaling Enterprise Agent Routing*,
    arXiv:2606.17519, 16 June 2026.
    Catalogue of 110 agents and 584 tools, evaluated on GPT-5.4, GPT-5.1
    and Claude Sonnet 4.5. Implicit-query routing F1 fell 58.2%→42.1% from
    51 to 584 tools; recall 55%→37%, precision 68%→60%. Oracle shortlisting
    still fell 79.0%→68.8%. Embedding shortlisting to about 20 candidates
    recovered 10–11 points; the elbow at 40–60 agents was catalogue-specific.
[^22]: Eliav, *Prompt Design at Scale*, arXiv:2607.19257, 21 July 2026.
    Single-author preprint.
    Perfect-response rates reached zero by 80 simultaneous instructions
    for every model, format and placement tested; no invented answer was
    recorded in 5,760 probes. Refusal was the failure near capacity.
[^23]: Laban et al., *LLMs Get Lost In Multi-Turn Conversation*,
    arXiv:2505.06120, May 2025.
    Over 200,000 simulated conversations, 15 models: average performance
    drop 39%; o3 98.1→64.1. Decomposition reported about 16% lower aptitude
    and 112% greater unreliability.
[^24]: Lodha et al., *Less Context, Better Agents*, arXiv:2606.10209, 8 June
    2026.
    Input accounted for 99.75–99.87% of token use; individual tool
    responses were 500–3,000 tokens, with accumulated histories reaching
    50,000–150,000+. Further checks were by the same authors across three
    task categories and Claude Sonnet 4.5, not independent replication.
[^25]: Drew Breunig, *How Long Contexts Fail*, 22 June 2025.
    The poisoning illustration was a Gemini agent playing Pokémon. The
    August 2026 source check did not find it in current versions of the Gemini 2.5
    technical report, or find a controlled propagation experiment.
[^26]: Cuconasu et al., *The Power of Noise*, arXiv:2401.14887, SIGIR 2024.
    Original reported improvement from irrelevant documents: up to 35%.
[^27]: Mazuryk et al., *The Powerless Noise*, arXiv:2607.03615, SIGIR 2026.
    Under normal settings the effect was −0.23% to −1.30%. The original
    setup used a 15-token generation limit, no chat template and a forced
    no-answer instruction. Truncated or malformed generations accounted for
    53.6–73.6% of its errors.
[^28]: Amiraz et al., *The Distracting Effect*, arXiv:2505.06914, 2025.
[^29]: Yen, Gao, Chen et al., *HELMET*, arXiv:2410.02694, ICLR 2025. 59 models,
    seven categories. <https://princeton-nlp.github.io/HELMET/>
    No synthetic long-context task reached average correlation above
    0.8 with downstream tasks; reported RULER correlations were below 0.85.
[^30]: Greg Kamradt, *LLMTest_NeedleInAHaystack*, MIT.
    <https://github.com/gkamradt/LLMTest_NeedleInAHaystack>
[^31]: OpenAI, GPT-4.1 launch material, 14 April 2025. MRCR 57.2% at 128K,
    46.3% at 1M; Graphwalks BFS 61.7% under 128K falling to 19.0% above.
    MRCR was corrected in December 2025 and Graphwalks in February 2026;
    the recorded corrections were unannounced.
[^32]: LoCoDiff, Mentat AI / AbanteAI, 8 May 2025.
    <https://abanteai.github.io/LoCoDiff-bench/>
    Reported best score 79%; all models below 50% above 25,000 tokens;
    largest prompt 97,500 tokens.
[^33]: Yao et al., *τ-bench*, arXiv:2406.12045, June 2024; and *τ²-bench*,
    arXiv:2506.07982, June 2025.
    GPT-4o retail-task success was below 50% on one attempt and below
    25% on all eight attempts.
[^34]: Du, Tian, Peng et al., *Context Length Alone Hurts LLM Performance
    Despite Perfect Retrieval*, arXiv:2510.05381, EMNLP 2025 Findings.
    Reported performance declines ranged from 13.9% to 85% across five
    models under the study’s controls.
[^35]: *The SWE-Bench Illusion*, arXiv:2506.12286, June 2025, final December
    2025.
    Buggy file identification from issue text: up to 76% within the
    benchmark versus 53% outside; function reproduction 35% versus 18%.
[^36]: Zarzecki, Dubiński and Cygert, *The Reliability Gap in Benchmark
    Auditing*, arXiv:2606.03305, 2 June 2026.
    Three detection methods across 25 models; 201 of 335 evaluations
    produced correct outcomes. Distribution shift and low statistical power
    limited detection.
[^37]: Wang et al., *EvoBrowseComp*, arXiv:2606.13120, 11 June 2026.
    Automated pipeline regenerating 800 live-web questions.
[^38]: Google, Gemini 3 launch material, 18 November 2025.
    Launch material reported LMArena, GPQA Diamond, MathArena Apex and
    MMMU-Pro, with no long-context benchmark in the sources cited here.
[^39]: Terminal-Bench 2.1 leaderboard, fetched 4 August 2026.
    <https://www.tbench.ai/leaderboard/terminal-bench/2.1>
[^40]: Arya, *RECON*, arXiv:2607.16716, 18 July 2026.
    Best non-oracle system reported 22.4%.
[^41]: Wang, Xu et al., *BM25 Wins at Scale*, arXiv:2607.26497, 29 July 2026.
    One team, one corpus construction; unreplicated.
[^42]: Li, Li, Zhang, Mei and Bendersky, *RAG or Long-Context LLMs?*,
    arXiv:2407.16833, EMNLP 2024 industry track. Source of the Self-Route
    results and the four-way failure taxonomy.
    Across nine datasets, long-context answer-quality gains were 7.6 points
    on Gemini-1.5-Pro, 13.1 on GPT-4o and 3.6 on GPT-3.5-Turbo; about 63%
    of queries produced identical answers. Self-Route used 38.6% of the
    tokens on Gemini-1.5-Pro (reported 65% cost cut) and 61% on GPT-4o
    (39% cost cut), at near-long-context quality. Over half of queries were
    answerable from retrieval alone.
[^43]: *LaRA*, arXiv:2502.09977, ICML 2025. 2,326 test cases, 11 models.
    At 32K, long context led by about 2.4%; at 128K, retrieval led open
    models by about 3.7%, while GPT-4o and Claude 3.5 Sonnet still favoured
    long context. Models of 3B–12B gained 6.5–38.1% from retrieval at 128K.
    Long context led reasoning by ~9% and comparison by 14–15%; retrieval
    led hallucination detection by 10–22%.
[^44]: Anthropic, *Introducing Contextual Retrieval*, 19 September 2024.
    Top-20 failure chain: 5.7% baseline, 3.7% contextual embeddings,
    2.9% with contextual BM25, 1.9% with reranking; relative reductions
    35%, 49% and 67%. Reported best k was 20. Header generation used
    50–100 tokens per chunk and cost about $1.02 per million document
    tokens with prefix caching. Vendor evaluation; no independent
    reproduction found as of August 2026.
[^45]: Industry RAG-Fusion deployment study, arXiv:2603.02153, 2 March 2026.
    Hit@10 fell from 0.51 to 0.48 despite increased retrieval recall.
[^46]: *Fishing for Answers*, arXiv:2509.04820, September 2025.
    One-shot retrieval with a wider search and chunk filter scored 91.0%,
    versus 90.0% for iterative retrieval; iteration made 59–97% more
    retrieval calls. Combining the two underperformed either alone.
[^47]: Jin et al., *Search-R1*, arXiv:2503.09516, March 2025.
    Reported gains of +41% and +20% over the evaluated baselines came
    from reinforcement learning on the search policy.
[^48]: Edge et al., *GraphRAG*, arXiv:2404.16130, April 2024. Preference-based
    evaluation; faithfulness at parity with baseline retrieval.
[^49]: Microsoft Research, *LazyGraphRAG*, 25 November 2024.
    Reported full GraphRAG indexing cost was 1,000× LazyGraphRAG’s.
[^50]: Anthropic, *Web fetch tool*, fetched 4 August 2026.
[^51]: Provider pricing pages for Anthropic, Brave, Exa, Firecrawl and Jina,
    all fetched 4 August 2026.
    Snapshot: Anthropic $10 per 1,000 searches, fetching charged through
    input tokens; Brave $5 per 1,000 searches; Exa $7 per 1,000 searches
    plus $1 per 1,000 pages per content type; Firecrawl one credit per
    page; Jina Reader output tokens with a 10,000-token minimum per search.
[^52]: Cloudflare, *Introducing pay-per-crawl*, 1 July 2025, modified 15 July
    2026.
[^53]: Zyte, `scrapinghub/article-extraction-benchmark`. 181 pages, four-gram
    shingle scoring; expanded to 28 extractors 1 March 2026, archived 24 June
    2026.
[^54]: Li et al., *Beyond a Single Extractor*, arXiv:2602.19548, 23 February
    2026.
    Combining extractors raised token yield by up to 71% with no
    benchmark regression in the reported setup.
[^55]: Smith and Troynikov, *Evaluating Chunking Strategies for Retrieval*,
    Chroma, 3 July 2024.
    Five corpora, 328,208 tokens, 472 queries, token-level scoring.
    About 9% recall spread between best and worst strategies. LLM-guided
    splitting gained 3.8 recall points over the recursive baseline and
    roughly halved precision; reducing overlap improved the overlap-with-
    ground-truth measure. Vendor evaluation, unreplicated.
[^56]: Chunking taxonomy paper, arXiv:2602.16974, SIGIR 2026, 19 February
    2026.
[^57]: Günther, Mohr, Williams, Wang and Xiao, *Late Chunking*,
    arXiv:2409.04701, September 2024, revised July 2025.
    Vendor’s own evaluation: nDCG@10 64.20→66.10 on SciFact,
    23.46→29.98 on NFCorpus and unchanged on Quora.
[^58]: *Reconstructing Context*, arXiv:2504.19754, 28 April 2025.
[^59]: Jiang et al., *LLMLingua*, arXiv:2310.05736, 9 October 2023. "Up to 20x
    compression with little performance loss", measured on GSM8K, BBH,
    ShareGPT and an arXiv set.
    Repository snapshot, checked 4 August 2026: MIT, 6,522 stars and one
    commit since October 2025.
[^60]: Pan et al., *LLMLingua-2*, arXiv:2403.12968, 19 March 2024. Claims
    2×–5×.
[^61]: Jiang et al., *LongLLMLingua*, arXiv:2310.06839, ACL 2024.
    Reported up to +21.4% accuracy at around four times fewer tokens on
    NaturalQuestions.
[^62]: Xu, Shi and Choi, *RECOMP*, arXiv:2310.04408, 6 October 2023.
    Reported retained size as low as 6% with minimal performance loss in
    the evaluated setups; extractive and abstractive variants, including
    empty-string output for irrelevant retrieved material.
[^63]: Isabella He, Anthropic cookbook, context-engineering tools, 20 March
    2026. Compaction fidelity: 3 of 3 high-level facts kept, 0 of 3
    appendix-table details.
    One research-agent demonstration: eight documents, about 329,000
    tokens. The six checked facts are a small sample from one run.
[^64]: Anthropic, *Prompt caching*, fetched 4 August 2026.
    Recorded model minima: Opus 5, 512 tokens; Opus 4.8, 1,024;
    Opus 4.7, 2,048; Opus 4.6, 4,096. Below-minimum requests process
    without caching and return no error.
[^65]: Anthropic, *Cache diagnostics*, beta `cache-diagnosis-2026-04-07`.
[^66]: Xiao et al., *StreamingLLM*, arXiv:2309.17453; Zhang et al., *H2O*,
    arXiv:2306.14048; Li et al., *SnapKV*, arXiv:2404.14469. Headline
    multipliers are against 2023-era baselines.
    Headline speedups of 22.2× and 29× used OPT and LLaMA-1/2 baselines.
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
    Internal research evaluation: reported +90.2% over a single agent.
    Multi-agent token use about 15× chat, single-agent about 4×. Token use
    explained 80% of performance variance in one benchmark. Vendor’s own
    evaluation; budget and task scope qualify the architecture claim.
[^72]: Walden Yan, Cognition, *Don't Build Multi-Agents*, 12 June 2025.
[^73]: Walden Yan, Cognition, *Multi-Agents: What's Actually Working*, 22
    April 2026.
    Reported reviewer average: two bugs per pull request, 58% severe;
    Cognition’s own deployment report.
[^74]: Tran and Kiela, *Single-Agent LLMs Outperform Multi-Agent Systems Under
    Equal Thinking Token Budgets*, arXiv:2604.02460, 2 April 2026.
[^75]: Cemri et al., *Why Do Multi-Agent LLM Systems Fail?*, arXiv:2503.13657,
    NeurIPS 2025. 1,600+ annotated traces, 14 failure modes.
    Specification and system-design issues accounted for about 42%
    of failures in the annotated traces.
[^76]: Chhikara et al., *Mem0*, arXiv:2504.19413, April 2025 — the
    full-context baseline figure is in the paper's own results table. Critique:
    Zep, *Is Mem0 Really SOTA in Agent Memory?*, 6 May 2025. Zep is an
    interested party.
    LoCoMo full-context baseline about 73%, Mem0 about 68%.
[^77]: Letta, *Benchmarking AI agent memory*, 12 August 2025. 74.0% on LoCoMo
    from a plain filesystem.
    The compared graph-based system scored 68.5%.
[^78]: `getzep/zep-papers` issue #5, Mem0 reply, 8 May 2025.
    Recorded competing results include 65.99%, a rerun at 75.14%, and
    58.44% after removing an adversarial category both parties disputed.
    Another quoted configuration reached 84%; these configurations do not
    form a single controlled comparison.
[^79]: Kuan Wang, *MemDelta: Controlled Baselines and Hidden Confounds in Agent
    Memory Evaluation*, arXiv:2606.29914, 29 June 2026.
    Changing only the embedder moved accuracy by 6.2 percentage points;
    one embedding choice reversed the ranking of memory approaches.
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
    Compared examples: 90.2% citation accuracy with about 31 effective
    citations, versus 81.4% with about 111.
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
    Worked examples: on a 164-item set, roughly three percentage points
    of standard error made 83.6% versus 86.7% hard to distinguish. Repeats
    K=2 reduced total variance by about a third, K=4 by half, with a
    two-thirds ceiling in the analysed setup. On 198 items, moving from
    one to ten repeats lowered the minimum detectable effect from 13.2%
    to 7.5%. One lower-temperature example tripled minimum variance.
[^89]: Prakhar Khatri, *Do Context Files Help Coding Agents?*,
    arXiv:2607.27250, July 2026. 291 runs, three repositories, two agents,
    three strategies; no correctness benefit; manipulation-validity probe;
    120–200 tasks needed for a 10-point effect; difficulty not portable across
    agents. Data and code at
    <https://github.com/codeprakhar25/context-files-coding-agents>.
    Three Python repositories, 15–17 merged pull-request tasks each,
    two agents, three context strategies and three repeats, 291 runs.
    Methods included within-task permutation tests (10,000+ iterations),
    task-clustered bootstrap equivalence testing, paired tests with
    multiple-comparison correction and Monte Carlo power simulation.
    Reported pass rates: 53.3%→55.6% (p=1.00) for one agent;
    58.8% / 56.9% / 52.9% (p=0.66) for the other. Thirty-six near-miss
    probe cells produced no conversion to a pass. At 17 tasks × 3 repeats,
    minimum detectable effect remained above 30 points. Task-difficulty
    rank correlation across agents was 0.75, with about 40% of tasks at
    floor or ceiling for one but not the other.
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
    Reported cross-encoder gains: 12.6% over GPT-5 and 13.4% over
    Gemini 2.5 Pro in nDCG@10, 36× and 48× faster, and 25–60× cheaper.
    Vendor configured the competitors; independent evidence supported
    the direction, not these magnitudes.
[^100]: Particula, reranker latency comparison, 22 May 2026. Methodology
    underspecified (batch size, hardware, concurrency unstated) and a separate
    source reports 392 ms for one of the same models. Ratio indicative, not
    the absolutes.
    Reported hosted latency 595–603 ms versus 188 ms self-hosted.
    The missing controls prevent attributing the gap solely to networking.
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
    Answer-span survival correlated 0.39–0.55 with exact match, versus
    0.31 for document recall. A 4.6× exact-match gap remained among
    questions where all gold documents had been retrieved.
[^107]: Byte-exact deduplication in retrieval-augmented generation,
    arXiv:2605.09611, 10 May 2026. The three-regime result, zero quality
    regression across four vendors.
[^108]: Cross-attention calibrated deduplication, arXiv:2607.24332, 27 July
    2026; and H3D, arXiv:2607.08382, 9 July 2026, benchmarking MinHash,
    SimHash, Winnowing, FuzzyHash and FlyHash.
    Embedding-similarity deduplication was roughly seven times slower
    than alternatives for about 10% additional removal in the cited work.
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
    Fixed window 4,096 tokens; evidence fell to a few hundred tokens
    at the performance drop.
[^113]: Tool-schema compression, arXiv:2605.26165, 24 May 2026. +20.5
    percentage points exact-match at 8,000 tokens against 2.6% uncompressed.
[^114]: *Control Under Compression*, arXiv:2608.01056, 2 August 2026. 92.7%
    success at 75% retained context against a 93.8% full-context baseline.
    The study reported sharp divergence between 50% and 35% retained
    context; these are not validated general retention thresholds.
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
    Google explicit storage at the check: $1.00 per million tokens per
    hour for Flash and $4.50 for Gemini 3.1 Pro Preview; 200,000 tokens
    held one hour on the latter costs $0.90 before reads.
[^124]: Yan Song, *Cache-Aware Prompt Compression: A Two-Tier Cost Model for
    LLM API Caching*, arXiv:2607.15516, 17 July 2026. One author, one provider
    API; the measured crossover is not a general threshold.
