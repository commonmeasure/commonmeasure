---
domain: edge
audience: contributor
---

# The specialist bundle

A synthetic vendor bundle for the specialist comparison: the documentation
corpus of **Fictive Systems**, a fictional enterprise-software vendor, built
so that retrieval over a vendor corpus has something real to disagree about.
Everything here is synthetic: the vendor, the product ("Orchestrator"), its
release lines, integration paths, entitlement tiers and every fact in every
document were invented for this bundle. No real vendor's product or
documentation is imitated, and no real customer configuration appears. Terms
like admission, manifest, suite and corpus are defined in `docs/GLOSSARY.md`.

## The corpus — `corpus/`

Eight markdown documents served by the existing internal `query` adapter
(point `COMMONMEASURE_INTERNAL_CORPUS` at `demo/specialist/corpus`). Beside
them, `corpus.json` declares what only the corpus owner can declare: the
licence and each document's **effective date** (the `dates` map, measured by
the freshness evaluator against a suite's `as_of`).

The corpus is deliberately shaped to give the comparison conditions
something real to disagree about:

- `connecting-data-sources-3x.md` and `connecting-data-sources-4x.md`
  **contradict each other across release lines**: 3.x's supported
  connectivity path (the Flux Connector) is retired on 4.x in favour of the
  Stream Gateway, and details such as credential rotation differ. Retrieval
  can answer a 4.x question from the 3.x document.
- `flux-connector-4x-migration.md` documents the **deprecated** 4.x path,
  the restraint target: an answer about 4.x connectivity should not teach it.
- `bulk-export-operations.md` is a **Premier-tier only** document in the
  vendor's own terms.
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

## The comparison suites — `demo/jobs/specialist/`

Two runnable conditions over one shared prompt set, one suite per
(condition, prompt) pair, all served from this corpus by the internal
adapter:

| condition | suite shape |
|---|---|
| `whole-*` | `policy_mode: observe`, `result_limit: 10` — the entire bundle admitted |
| `retrieval-*` | `policy_mode: observe`, `result_limit: 3` — query selection only |

The four prompts are identical across conditions: a deprecated-path request
(`*-restraint`), a request whose correct answer changed between release
lines (`*-currency`), a Premier-tier request (`*-refusal`), and an ordinary
supported request (`*-baseline`). Every suite declares the same per-prompt
`coverage_rubric` and `as_of`, so coverage and freshness are measured on the
same terms and the conditions differ only in the result limit.

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

## What is not here

The comparison design's fourth condition, the provider's conventional
support journey, cannot be reproduced in this lab and is recorded as
**unavailable**; it is not simulated.
