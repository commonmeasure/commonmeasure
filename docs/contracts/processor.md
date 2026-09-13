---
title: Processor contract
---

# Processor contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

A **processor** is a replaceable unit of work around a ContextJob
([`docs/GLOSSARY.md`](../GLOSSARY.md) has the one-line version). A processor never receives
authority it did not declare. Each declares stage, inputs, outputs,
permissions, failure policy, determinism and evidence type.

Three things in this system are easy to confuse and are not the same. The
distinction is defined once, in this section.

- A **processor** is a stage *around* a job. It is installed, not chosen;
  sealed into the manifest as part of experiment identity; and it runs on
  every plan. It never competes and is never ranked, and its evidence is
  about the run's own machinery.
- A **skill** is a *candidate* — one of several supply routes the objective
  ranks, invoked to produce a result for the job
  ([`docs/contracts/provider.md`](provider.md) §Skill supply). Its evidence is about what
  was supplied, not about the machinery. A skill's identity is a third
  party's declaration on the operator's disk, which is why it is observed per
  run rather than sealed into the manifest as a processor's configuration
  digest is.
- A **harness plugin** is how a host reaches the runtime; the Claude Code
  plugin in `plugin/` is one ([`docs/contracts/host-integration.md`](host-integration.md)). A skill
  is supply that the runtime reaches.

The last pair is named explicitly because harness marketplaces distribute
skills inside plugins, so in ordinary use one word covers both: the
`commonmeasure` plugin is a plugin; a skill bundle installed under
`~/.claude/skills` is a skill.

This document governs the processor. The processor is the extension point
that optimisation, verification, provenance and grounding addons plug into.

## Stages

- `discover` — produce candidate sources or supply routes;
- `admit` — check source, licence, provenance, safety and budget before context
  crosses;
- `transform` — deduplicate, rank, select, order or compress context;
- `infer` — execute a ModelPlan through a gateway;
- `verify` — attach claim, citation, provenance or policy evaluations;
- `attest` — sign or independently corroborate an artefact;
- `report` — build a purpose-limited projection for an authorised receiver.

## Manifest

Every processor declares:

- stable name, version and implementation identity. For an in-process
  processor the identity is the crate and crate version it was compiled from
  plus a **configuration digest**: SHA-256 over the canonical rule text that
  determines its behaviour. Running code cannot attest to a digest of its own
  machine code; the rule digest is what a reader needs to re-derive an
  invocation, and behaviour cannot change without the rule digest changing.
  Sidecar and remote processors, which arrive as separate artefacts, will
  declare an artefact digest as well;
- supported stage and capability name;
- network, filesystem, model, credential and content permissions;
- whether it can see raw content, prompts or responses;
- deterministic, seeded or non-deterministic execution;
- resource limits: an in-process processor has no supervisor to enforce a
  timeout, so its manifest says so and does not declare a limit nothing
  enforces. Supervised sidecars declare enforced timeouts;
- fail-open or fail-closed behaviour by policy mode;
- the evidence format its invocations take
  (`contextops-processor-invocation/v1`). No schema file exists for it; the
  invocation shape is defined in code at `crates/commonmeasure-runtime/src/processor.rs`
  (`INVOCATION_VERSION`). Health-check formats will be declared with the
  first sidecar: an in-process processor's health is the process's health.

## Evidence

Every invocation produces an append-only record containing:

- processor identity and configuration hash;
- input and output artefact hashes;
- start/end timestamps and latency; cost and further resource use where the
  processor incurs any (the in-process deterministic processors incur none
  and record none: an absent cost field means the processor cannot spend,
  not that a spend went unmeasured);
- decision, score or abstention with method;
- changed, removed or selected source spans where applicable;
- error, retry and fallback chain where the invocation had one (in-process
  processors neither retry nor fall back; a failure surfaces as the
  fail-by-policy-mode behaviour their manifest declares, recorded in `gaps`);
- assurance basis and blind spots.

Scores are namespaced to the processor. The runtime does not put a reranker
score, an NLI probability, a provenance validation and a source-authority
assessment on one scale.

## Process boundary

The Rust runtime supports three implementations behind the same contract:

1. in-process Rust library for small, trusted deterministic components;
2. supervised local sidecar over stdio or loopback for Python/Go/model-heavy
   components;
3. remote service run by an authorised provider (a provenance-stamping
   vendor, for example).

Sidecars receive one scoped invocation, not unrestricted evidence access. Remote
processors receive only fields permitted by egress policy.

## Status

The in-process implementation exists: `crates/commonmeasure-runtime/src/processor.rs`
and eight processors behind it, seven deterministic and one, the fidelity
judge, a model exchange through the configured gateway. The two detectors
run in every batch run and mediated session; the optimiser in every batch
run; the HTML text extractor on every mediated fetch that received a body;
the governor
runs only in batch runs whose suite declares `governance`; the fidelity
verifier runs on every batch answer, the fidelity judge only in batch runs
whose suite declares `fidelity_judge`, and the output provenance labeller
only in batch runs whose suite declares `output_provenance`.

- **`pii-detector` (admit).** Pattern rules over a source's text before it can
  reach a model. In `strict` a finding refuses the crossing; in `observe` and
  `prefer` it is recorded identically and carried. The finding's mode
  discipline is `commonmeasure_runtime::policy::Ruling`, the same switch every other
  policy uses. Findings are categories and byte offsets only; the matched text
  never enters the record.
- **`injection-screen` (admit).** Pattern rules over a source's text for known
  indirect-prompt-injection phrasings — instruction override, role
  reassignment, system-prompt exfiltration, tool directives — before that text
  can reach a model. Same mode discipline as the PII detector, run at the same
  admit point: `strict` refuses, `observe` and `prefer` record and carry.
  Findings name the rule matched and byte offsets only; the matched text never
  enters the record. Its scope is bounded: it is deterministic screening that
  names what it matched, a case-folded substring test and not a
  comprehension-grade defence. A source matching no rule is recorded as
  clean under these rules, not as safe. The rules state the most the screen
  can claim, and they are the canonical text the configuration digest covers.
- **`support-governor` (admit).** The suite's sealed governance rule set
  ([`docs/contracts/run-output.md`](run-output.md) §Manifest) evaluated against each source's
  operator-declared metadata: an unentitled tier, a deprecated path in force
  at the suite's `as_of`, a path no rule speaks for, or a source with no
  declared metadata each refuse in `strict` and are recorded identically in
  `observe` and `prefer`, through the same `Ruling` switch. It runs only when
  the suite declares `governance`; an ungoverned suite records no invocation.
  The rule set's identity and content are in the manifest, not in this
  processor's configuration digest, so changing a rule changes the
  experiment's hash and leaves the processor identity unchanged.
- **`html-text-extractor` (transform).** The readable text of a mediated
  fetch. A response whose content type is `text/html` or
  `application/xhtml+xml` is decoded as UTF-8 and reduced to its text:
  comments and the `head`, `script`, `style`, `template`, `noscript` and
  `svg` elements are dropped, the text of every other element is kept, each
  block element starts a line, character references are decoded and runs of
  whitespace collapse. Any other body is decoded and delivered unextracted.
  It runs before the admit screens, so the screens rule on the text the agent
  would read and a finding's offsets index that text. Its record's input is
  the hash of the bytes received and its output the hash of the text
  delivered, the two hashes the crossing carries as `retrieved_hash` and
  `content_hash`; a reader re-derives the output by applying the rules the
  configuration digest pins to the bytes. It removes markup, not boilerplate:
  navigation, footers and related-content lists stay in the text.
- **`context-optimiser` (transform).** Whole-source and repeated-phrase
  deduplication in retrieval order, no ranking and no learned compression. Its
  invocation record is re-derivable: excise the recorded spans from the sealed
  source text and the hash of what remains is the recorded output hash. The
  original envelope stays addressable.

- **`fidelity-verifier` (verify).** The answer segmented into claims (the
  sentences of its non-citation lines) and each claim judged against the
  grounding record and the window's retained text: `supported` where the
  claim contains a supported citation's quote or a run of at least six of
  its consecutive words occurs in one part, the span recorded with the
  part's content hash; `contradicted` where the claim contains the quote of
  a citation the grounding evaluator judged contradicted; `unsupported`
  otherwise. Supported means a word sequence entered the window, not that
  the claim is true, and a paraphrase is invisible. The record carries the
  claims' byte offsets into the answer, so `commonmeasure inspect` rechecks
  each claim against the answer it prints.
- **`fidelity-judge` (verify, opt-in).** One exchange through the configured
  gateway under the suite's model plan, over the same window the answer
  model received: a verdict per claim (`supported`, `contradicted`,
  `unsupported`), parsed deterministically, with a claim the reply does not
  cover recorded as `unavailable`. The record is `declared`, not observed,
  and carries the judge's agreement with the verifier: claims compared,
  claims agreed, the fraction (null where nothing was compared) and a
  matrix of judge verdicts under each verifier verdict. The exchange's
  charge in currency is unknown, not zero. No gateway, an empty window or
  an answer with no claims records an abstention with the reason and a gap.
- **`output-provenance` (attest).** Builds one C2PA manifest per answered
  plan from that plan's sealed window, signs it with the certificate the
  operator names in `COMMONMEASURE_PROVENANCE_CERTIFICATE` and
  `COMMONMEASURE_PROVENANCE_KEY`, embeds it in the answer under C2PA 2.4
  Annex A.8 and reads it back before recording. Each part of the window is
  an ingredient named by the SHA-256 of the text that entered and the grade
  its crossing was recorded at; a `c2pa.created` action carries the
  trained-algorithmic-media source type; an `ai.commonmeasure.source-record`
  assertion carries the run id, the plan id, the run manifest hash, the
  answer's hash and the same source list; a `cawg.training-mining` assertion
  carries the suite's declaration verbatim; a `cawg.identity` assertion is
  signed with the same certificate under the role `cawg.producer`. No
  prompt, context or answer text enters the manifest. The plan's `answer`
  stays the model's bytes; the labelled text and the raw manifest store are
  published as `provenance/<plan>.txt` and `provenance/<plan>.c2pa`
  ([`docs/contracts/run-output.md`](run-output.md) §Directory), and `commonmeasure provenance
  <file>` reads a labelled text back with no run directory. The signed bytes
  are not deterministic (a fresh manifest label, a randomised signature);
  the assertions are a pure function of the record, and the test re-derives
  them. Without a signing identity the invocation abstains with a gap naming
  both variables, the manifest that would have been signed stays in the
  record, and the answer stands unlabelled. No trust list is configured, so
  a well-formed signature reads as valid but untrusted, and the record says
  so (`demo/provenance/README.md`).

Scores and findings are namespaced inside each invocation record, as above.
Sidecar (supervised Python over stdio) and remote implementations are
planned and have no implementation; learned rerankers, compressors and
checkers are built that way, never in-process. The fidelity judge is not
one of them: its model runs behind the gateway, and the in-process code
builds the exchange and reads the reply.
