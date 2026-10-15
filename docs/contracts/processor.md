---
title: Processor contract
domain: extensions
audience: contributor
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
  A sidecar or remote processor, which arrives as a separate artefact,
  declares an artefact digest as well;
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
  (`INVOCATION_VERSION`). No health-check format is declared: an
  in-process processor's health is the process's health, and a sidecar
  declares one with its protocol.

## Evidence

Every invocation produces an append-only record containing:

- processor identity and configuration hash;
- input and output artefact hashes;
- start/end timestamps and latency; cost and further resource use where the
  processor incurs any (the in-process deterministic processors incur none
  and record none: an absent cost field means the processor cannot spend,
  not that a spend went unmeasured). The Encypher route records an explicit
  unknown processor charge in `detail.remote.cost` and a gap after any attempted
  request; acquisition-cost measurements do not include that charge;
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
and nine processors behind it, plus the SimpleQA benchmark's
`simpleqa-correctness` ([`docs/contracts/simpleqa-benchmark.md`](simpleqa-benchmark.md)).
Each declares its determinism in its manifest. Six declare `deterministic`:
the two detectors, the release verifier, the optimiser, the fidelity
verifier and the integrity checker.
Four declare `non_deterministic`: the HTML text extractor, because credential
validation depends on the verification time; the output provenance labeller,
because its signed bytes differ on each run; and the fidelity judge and
`simpleqa-correctness`, each a model exchange through the configured
gateway. None declares `seeded`. The two detectors
run in every batch run and mediated session; the optimiser in every batch
run; the HTML text extractor on every mediated fetch that received a body
other than a signed release; the release verifier on every mediated fetch
whose body was served as one;
the fidelity
verifier runs on every batch answer, the fidelity judge only in batch runs
whose suite declares `fidelity_judge`, and the output provenance labeller
only in batch runs whose suite declares `output_provenance`. The integrity
checker runs where a caller displays a reply against a verified release;
no command or tool calls it yet.

- **`pii-detector` (admit).** Pattern rules over a source's text before it can
  reach a model. A finding is recorded in the invocation and carried on the
  crossing as a breach, in every mode and whatever the source; it refuses
  nothing. Findings are categories and byte offsets only; the matched text
  never enters the record. A body the edge does not read (a PDF) gets no
  finding: the detector records that it did not rule, and that unknown
  verdict is a breach `strict` refuses.
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
  On a signed release it rules twice: once on the release text, and once,
  in a second record, on the source's strings delivered beside it (signer
  key, text hash, title, URL, licence, attribution, each term, display
  mode, release time, passage ids, marker labels and the passages they
  name). That record names each string as an input `<url>#<field>` with
  its hash, and each finding names its field.
- **`release-verifier` (admit).** A mediated fetch whose `Content-Type`
  names `application/x.signed-release+json` is read as a signed release in
  place of extraction: UTF-8 JSON with the source's `text` and a `sidecar`
  of passages by id, keep-intact markers, terms, display mode and
  attribution, signed with Ed25519 over the text's SHA-256 and the RFC 8785
  canonical JSON of the signed fields. The format and its media type are a
  working format that the trusted news demo's stand-in gatekeeper defined,
  not a standard and not Common Measure's to define; it is offered to the
  open standards bodies as a proposal. The media type takes precedence over
  the body's first bytes: a body served under it that starts like a PDF
  goes to this processor, not the file path. A body that does not read as a
  release (not that JSON, a field outside the format in the signed fields
  or a passage or marker, a duplicate passage id, a passage or marker
  outside the text, a marker naming no passage or lying outside the
  passage it names) is `unavailable` with a `capability_unavailable` gap in
  every mode, and is never delivered as plain text or as a file. A release
  that reads is verified under the key its sidecar names, with strict
  Ed25519 verification (a small-order key or R, or a non-canonical S,
  fails): `verified`, `failed` (another scheme, the text not matching its
  hash, a malformed key or signature, a signature that does not verify) or
  `missing` (no signature or no key). `failed` and `missing` are a breach
  under the shared mode discipline: `strict` refuses, `observe` and
  `prefer` record and carry. The record's input is the hash of the bytes
  received and its output the hash of the release text, which the crossing
  carries as `retrieved_hash` and `content_hash`; the PII detector and the
  injection screen then rule on that text. The record carries the state,
  the signer's key, the text's signed hash and the counts of passages and
  markers, never the text. Verified means the holder of the named key
  signed this text and these fields: no key is pinned or looked up for the
  source, so it does not establish who signed, and it says nothing about
  the accuracy of the terms or passages.
- **`html-text-extractor` version 2 (transform).** The readable text of a mediated
  fetch. A response whose content type is `text/html` or
  `application/xhtml+xml` is decoded as UTF-8 and reduced to its text:
  comments and the `head`, `script`, `style`, `template`, `noscript` and
  `svg` elements are dropped, the text of every other element is kept, each
  block element starts a line, character references are decoded and runs of
  whitespace collapse. Before extraction, the C2PA SDK verifies supported
  embedded credentials against the received body with any content coding
  removed. UTF-8 HTML supports Annex A.7 inline manifests; other UTF-8
  `text/*` media types support Annex A.8 wrappers. A recovered Annex A.8
  wrapper is removed after verification, including where its manifest is
  invalid or untrusted. Other non-HTML content is delivered as decoded.
  Unsupported or incomplete wrappers stay in the text. A PDF, and the other
  files `context_fetch` does not decode, never reach it
  ([host integration](host-integration.md#a-fetched-file)).
  A body the origin served under the gzip content coding is gunzipped
  first, and the input hash stays over the coded bytes it served; a body
  under any other coding never reaches it. It runs before the admit screens, so the screens rule on the text the agent
  would read and a finding's offsets index that text. Its record's input is
  the hash of the bytes received and its output the hash of the text
  delivered, the two hashes the crossing carries as `retrieved_hash` and
  `content_hash`; a reader re-derives the output by applying the rules the
  configuration digest pins to the bytes. It removes markup, not boilerplate:
  navigation, footers and related-content lists stay in the text.
  `detail.embedded_credential` retains the verification state, manifest-store
  SHA-256 reference, active manifest label, reading method, verifier version
  and SDK validation statuses beside those hashes. No trust list is configured:
  a valid signature and content binding produce `untrusted`; a failed check
  produces `invalid`. Unsupported media types, non-UTF-8 bodies and unsupported
  wrappers produce `unavailable` with a capability gap. Annex A.7 external
  links retain their reference with `unavailable`; the verifier makes no
  network request. `absent` means no supported credential was found in a
  supported text body. None of these states grants licensing authority or
  changes admission policy. Text transformation is deterministic; credential
  validation can depend on the verification time, so the processor declares
  `non_deterministic`. The invocation records its start and finish times.
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
- **`integrity-checker` (verify).** How a reply is displayed against a
  signed release the release verifier verified. The reply's items say what
  they are: `quote` (a passage id, optionally a sentence index),
  `from_source` (a sentence and the passage ids it cites) or `own` (the
  assistant's own sentence). Each sentence is put to four deterministic
  stages over the release text, each answering one question with its own
  verdict and probability: the fidelity verifier's six-word run
  (`supported`, recorded as evidence, admits nothing), the whole sentence
  verbatim (`supported`), an attribution lookup for quotations
  (`right_person_quoted`) and keep-intact containment
  (`cuts_kept_passage`). Nothing combines the answers. The display rule
  labels rather than withholds, with strictness from the release's signed
  `display_mode`: under `labelled` a sentence whole in the text is shown
  confirmed with its passages, and an own or unconfirmed sentence is shown
  labelled "not from the article"; under `strict` (and under any mode value
  other than `labelled`) only confirmed sentences and the source's words
  are shown. A sentence that cuts a kept span, or attributes a quotation to
  someone the text does not, is replaced by the source's words in both
  modes. A quote is rendered from the signed text, expanded to a kept span
  it cuts, or refused. The record carries, per item, each stage's raw
  answer, the outcome and the passage shown, and the eleven-count vector
  defined in [`display-counts.md`](display-counts.md); its inputs are the
  release text, the signed sidecar (canonical JSON) and the reply by hash,
  its output the display by hash. The record holds no text of the reply or
  the release: a sentence is recorded by its SHA-256 and byte length, the
  words shown by byte span into the release text, a quotation by its byte
  span in the sentence and its SHA-256, a speaker by SHA-256, and an
  identifier the reply supplied that the release does not sign by its
  SHA-256. Reasons name no sentence, quotation or speaker. Hashes are
  `sha256:<hex>`, as in the other records. The rendered text is returned
  to the caller beside the record, never in it. No
  model stage is installed: a paraphrase is never confirmed and nothing is
  withheld for contradicting the source, which the record states. A
  release whose signature failed or is missing is not displayed: the
  invocation abstains with an `evidence_missing` gap. The record belongs
  in the local evidence log and is not projected to any receiver;
  `commonmeasure inspect` prints it from a session file, citing the line
  and the pointer in that line's record. The mediated `context_show` tool
  invokes it ([host integration](host-integration.md) §Displaying a reply
  against a release) and records a `display_rendered` record after it. The
  checker does not screen the release's strings; the injection screen
  rules on them at the fetch.
- **`output-provenance` (attest).** Builds one C2PA manifest per answered
  plan from that plan's sealed window. The default local route signs it with the certificate the
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

### Independent output verification

`commonmeasure provenance <file>` reads an exported Annex A.8 text credential
without its originating run directory. Invalid credentials produce a JSON report
and a failing exit status. Absent or unreadable credentials are errors. A valid
but untrusted credential succeeds unless `--require-trusted` is set.

`--trust-anchors <pem>` supplies local PEM certificate trust anchors for both the
C2PA claim and CAWG identity assertions. `--require-trusted` requires that option
and fails unless the SDK's complete validation state is trusted. The report
retains raw SDK results, the exact bundle's SHA-256 digest, its scope, the SDK
version and verification time. Supplying a bundle does not assert membership of
an official C2PA trust list, establish factual accuracy or grant content rights. A missing,
empty or malformed bundle fails without falling back to default verification.

Verification makes no network calls, downloads no trust list and does not fetch
current revocation status. Operators supply and maintain their chosen anchors.
The reader uses fresh settings per call; the default read and local signing path have
no anchors. `--run <directory>` additionally compares the embedded source record
with the run summary, and a mismatch fails; a source reference that differs only
by its userinfo is not a mismatch. This is a local reader capability;
managed trust distribution and live host output signing remain planned.

### Optional Encypher signing

The batch adapter is **fixture-tested** against controlled HTTP responses carrying
real C2PA signatures. Live interoperability remains incomplete: the service must
accept standard ingredient assertions, return metadata readable by the local
verifier and preserve every requested training-use declaration. Provider retention
remains unverified. It is a specific adapter
inside the existing processor, not the generic external-processor protocol.

The job explicitly authorises disclosure with
`output_provenance.encypher.send_answer_and_source_record: true`. It must also
declare the existing `training_mining` preferences. The operator selects
`COMMONMEASURE_PROVENANCE_SIGNER=encypher`, supplies `ENCYPHER_API_KEY` or
`ENCYPHER_API_KEY_FILE` (exactly one), and sets
`COMMONMEASURE_PROVENANCE_TRUST_ANCHORS` to a local PEM trust bundle. The selected
remote configuration is sealed in the run's processor manifest with network
permission and its own configuration digest. Missing permission or configuration
produces an unavailable invocation without a request or local-signing fallback.

The adapter sends the answer, source references, hashes, acquisition grades, run
identifiers and declared output-use preferences to the fixed
`https://api.encypher.com/api/v1/sign` endpoint. It sends no prompt or source body
separately; the answer itself may contain excerpts. Each source reference is sent
with any userinfo (`user:key@`) removed, in the ingredient assertions and the
source record alike. The userinfo is cut from the text and every other byte is
kept as recorded; where it cannot be cut from the text, the URL parser's
serialisation without the userinfo is sent (`https:u:p@h.example/f` is sent as
`https://h.example/f`). A value that does not parse as a URL but has a
`scheme://user@host` authority loses its userinfo too. The signed credential
names the references as sent. The run's own record keeps them as recorded, and
`--run` verification does not count a reference's userinfo as a difference. This
changed the processor's version and configuration digest in 0.4.2. The answer, labelled-output and manifest-store references are not sent. It requests a full document
credential with plain-text embedding, no attribution indexing and no manifest
database persistence. Those options do not establish complete provider retention.
The answer is limited to 1 MB and the request to 2 MiB. One HTTP exchange uses
the transport's 30-second budget, excluding host DNS, and 32 MiB response ceiling;
the exact request hash supplies the required `Idempotency-Key` header. There are
no redirects or automatic retries. A lost reply may follow completed
signing, so the record does not assert that a transport failure means no charge.

Before publishing, local readback requires trusted validation, unchanged
NFC-normalised text, the exact source record and training declaration, matching
ingredients with their hashes, grades and `inputTo` relationships, and the
declared AI-generation action. Missing, duplicated or changed required assertions
fail. The provider is the claim signer; its certificate does not independently
establish the operator's identity. The answer remains unlabelled on failure.

The private invocation retains request/response hashes, attempted delivery, HTTP
status where received, the chosen document identifier, trust digest, validation
and explicit unknown charge. It does not store request/response bodies or API
keys in diagnostics. Successful text and raw credential bytes are published in
the existing provenance artefact paths. This creates no Content Telemetry egress
or Hub receipt permission. `demo/provenance/README.md`
contains configuration and the explicitly invoked synthetic live trial.

Scores and findings are namespaced inside each invocation record, as above.
The generic sidecar (supervised Python over stdio) and remote processor protocols
remain planned; learned rerankers, compressors and
checkers are built that way, never in-process. The fidelity judge is not
one of them: its model runs behind the gateway, and the in-process code
builds the exchange and reads the reply.
