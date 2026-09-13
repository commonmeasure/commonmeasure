# Fixtures

## cited-run-2026-08-04.json

The four live model answers, their evaluation windows, and the verdicts the
grounding evaluator (0.3.0 @ sha256:771836f4…) published about them, lifted
verbatim from the 4 August 2026 cited run — `demo/output/cited` at commit
4b617e5. That run is the only witness of two evaluator defects since fixed (the
SOURCE prefix collision between the citation directive and the window
header, and case-sensitive quote matching); `just cited-example` overwrites the run
directory and `responses/` is gitignored, so this fixture is the durable copy
(W0 of the same brief). Do not regenerate it: a rerun of a fixed evaluator
cannot reproduce the evidence.

Schema, per plan (`plans[]`):

- `id`, `provider` — the plan as `summary.json` names it. The fifth plan of
  the run, `tollbit-only`, produced no answer and is not part of the corpus.
- `answer` — the model's answer, byte for byte; `answer_sha256` matches the
  evaluation record's `answer` input hash.
- `window[]` — the parts that entered evaluation, in order, each with
  `source_ref`, `content_hash`, `text`: exactly the fields of
  `commonmeasure_inference::ContextPart`, so a test can deserialise them straight into
  an `EvaluationInput` or render them through the production window renderer.
  `text` is what the evaluator matched against: the recon capture's bounded
  retained lead (~120 chars plus a disclosed elision marker), with the
  context-optimiser's recorded spans excised where the run excised them.
  Every `sha256(text)` was re-verified against the recorded hash at lift time.
- `published_evaluation` — the verdicts as published: evaluator identity,
  `verdict_counts`, and the `citations` array verbatim, including its wrong
  reasons.

W2 witness: `tavily-only` citation 0 quotes `window[0].text` exactly except
for the case of the first word ("the AI Act Code of Practice…" vs the
source's "The …"). W1 witness: every citation line carries the window's
`SOURCE ` label (or the directive's `SOURCE_URL` placeholder) inside the URL
token, and every published verdict mis-resolved it.

The retained excerpts are deliberately bounded: the run itself sealed only
elided ~120-character leads, no fuller licensed text exists in the run, and
none is committed here.
