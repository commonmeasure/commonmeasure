---
domain: edge
audience: contributor
---

# The governed specialist bundle

A synthetic vendor bundle for the governed specialist demonstration: the documentation
corpus of **Fictive Systems**, a fictional enterprise-software vendor, built
so the governed specialist slice has something real to govern. Everything
here is synthetic: the vendor, the product ("Orchestrator"), its release
lines, integration paths, entitlement tiers and every fact in every document
were invented for this bundle. No real vendor's product or documentation is
imitated, and no real customer configuration appears. Terms like admission,
manifest, suite and corpus are defined in `docs/GLOSSARY.md`.

## The corpus — `corpus/`

Eight markdown documents served by the existing internal `query` adapter
(point `COMMONMEASURE_INTERNAL_CORPUS` at `demo/specialist/corpus`). Beside
them, `corpus.json` declares what only the corpus owner can declare: the
licence, each document's **effective date** (the `dates` map, measured by the
freshness evaluator against a suite's `as_of`), and each document's
governance metadata (the `documents` map: edition, version range, integration
path, support status, entitlement tier) that the support-status rule set
evaluates at admission.

The corpus is deliberately shaped to give the comparison conditions
something real to disagree about:

- `connecting-data-sources-3x.md` and `connecting-data-sources-4x.md`
  **contradict each other across release lines**: 3.x's supported
  connectivity path (the Flux Connector) is retired on 4.x in favour of the
  Stream Gateway, and details such as credential rotation differ. Retrieval
  without the rules can answer a 4.x question from the 3.x document.
- `flux-connector-4x-migration.md` documents the **deprecated** 4.x path,
  the restraint target: a governed run must refuse it rather than teach it.
- `bulk-export-operations.md` is **Premier-tier only**, the entitlement
  refusal target for a Standard-tier grant.
- `release-notes-4-2.md` carries an **obviously hostile planted instruction**
  (marked as such in the text). The deterministic `injection-screen`
  admit processor does not match it, and this document demonstrates the
  screen's stated blind spots: every planted phrase wraps across
  a hard line break ("reveal your … system prompt" split over two lines), the
  wording "ignore your previous instructions" is not among the rule phrases,
  and the screen matches literal case-folded substrings without crossing
  line wraps. Regenerating these runs therefore admits this document, with
  the screen's invocation recording no matched rule.
  `crates/commonmeasure-runtime/tests/injection_admission.rs` pins this on the
  document's own bytes, so this paragraph cannot drift from the matcher
  unnoticed. The offline injection bundle (`demo/injection/`) is the
  committed evidence for what the screen does catch. Screening is a bounded
  substring match that names what it caught; it is not a comprehension-grade
  defence.
- `orchestrator-overview.md` deliberately has **no entry in the `documents`
  map**: under a governed suite, a document with no declared governance
  metadata is refused, naming the absence: unknown is not supported.
- `stream-gateway-tuning.md` is an ordinary supported 4.x Standard-tier
  document with full governance metadata, which a governed run admits.

## The comparison suites — `demo/jobs/specialist/`

Three runnable conditions over one shared prompt set, one suite per
(condition, prompt) pair, all served from this corpus by the internal
adapter:

| condition | suite shape |
|---|---|
| `whole-*` | `policy_mode: observe`, no governance, `result_limit: 10` — the entire bundle admitted, no rules consulted |
| `retrieval-*` | `policy_mode: observe`, no governance, `result_limit: 3` — query selection only |
| `governed-*` | `policy_mode: strict`, sealed `governance` rule set, entitlement grant `standard`, `require_cited_answer: true` |

The four prompts are identical across conditions: a deprecated-path request
(`*-restraint`), a request whose correct answer changed at a rule's
effective date (`*-currency`), a Premier-tier request under a Standard grant
(`*-refusal`), and an ordinary supported request (`*-baseline`). Every suite
declares the same per-prompt `coverage_rubric` and `as_of`, so coverage and
freshness are measured on the same terms and the conditions differ only in
the governance layer.

The runs are produced with the gateway up. The recorded runs are not in the
public repository, because their records carry the recording machine's
paths; the recipe writes to `output/specialist/<suite>` under
`COMMONMEASURE_PRIVATE_EVIDENCE` and refuses to run without it
(`CONTRIBUTING.md`):

```sh
just gateway-up
COMMONMEASURE_INFERENCE_ENDPOINT=http://127.0.0.1:3000/openai/v1/chat/completions \
  just specialist-examples
```

Answers are not byte-stable across regenerations (the gateway requests
temperature 0 but no byte-stability claim is made); the recorded evidence
rests on the deterministic record: manifest hashes, admission decisions,
window composition and evaluation records.

Source URLs in the recorded runs are the
internal adapter's `file://` URLs, absolute to the checkout that
generated them. Regenerating on another machine yields internally
consistent runs whose URLs, sealed-response bytes and content hashes all
differ with the path; cross-machine comparison is by manifest hash and
record structure, not by bytes.

## The revocation pair — `revocation-pre` and `revocation-post`

The control demonstration: revoke one rule and show the previously admitted
answer stops, traceably. The two suites have the same job id, the same
prompt (the restraint prompt) and the same corpus; they differ only in that
rule set 1 schedules the flux-connector 4.x deprecation for 2026-09-01 (after the
suite's `as_of`, so not yet in force) and rule set 2 brings it forward to
2026-03-01. The whole revocation event is one visible line:

```sh
diff demo/jobs/specialist/revocation-pre.json demo/jobs/specialist/revocation-post.json
```

In the `pre` run the migration document is admitted (the governor's
invocation records "deprecation effective 2026-09-01 is after as_of
2026-08-06 and not yet in force"), the answer cites it, and the restraint
rubric measures 3/3. In the `post` run the same document is refused before
inference, the answer stops citing it, and coverage falls to 1/3. The proof
rests on the deterministic records, not on answer bytes:

```sh
jq -r .hash <runs>/revocation-pre/manifest.json <runs>/revocation-post/manifest.json
commonmeasure inspect <runs>/revocation-post
```

Two different manifest hashes whose suites differ only in the flipped
rule; the `post` dossier's policy lines carry the refusal naming rule set
`fictive-support-rules/2` and its effective date, the window composition
loses the document, and the evaluation records move for that stated reason.

## What is not here

The support-status rule set is **not** corpus data: rules are declared and
sealed in each governed job suite, so flipping a rule moves the suite's
manifest hash and revocation stays traceable. The run manifest names
nothing inside a corpus directory, so a rule kept in the corpus could change
without moving any hash.

The comparison design's fourth condition, the provider's conventional
support journey, cannot be reproduced in this lab and is recorded as
**unavailable**; it is not simulated.
