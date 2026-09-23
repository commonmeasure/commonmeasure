# The commerce demonstration bundle

A fictional retail bundle for the commerce demonstration. It follows the
same discipline as the governed specialist bundle (`demo/specialist/`): a
comparison a sceptic can replay, assets that are obviously
fictional, and rubrics declared before any plan runs. Everything here is
synthetic: the retailer (**Fictive Retail**), its brands (Aurelio, Northglade), the
publications (The Crema Courier, BrewPrint, GadgetGrove), every product and
every fact in every document were invented for this bundle. No real
retailer, brand, publication or product is imitated, and no real price,
stock position or review appears. Terms like admission, manifest, suite and
corpus are defined in `docs/GLOSSARY.md`.

## The corpus — `corpus/`

Sixteen markdown documents in three source classes, one class per
subdirectory, served by the internal `query` adapter:

- **`product-data/`** — structured product data from the retailer's own
  merchandising system: five catalogue listings and a compatibility matrix,
  all dated 1 August 2026. The freshness evaluator favours this class, and
  it is the class that holds prices, stock and the compatibility cut-over
  dates.
- **`brand-content/`** — the brands' own pages: an Aurelio launch page, a
  Northglade fitting guide and product page, an Aurelio BrewLink Hub page,
  and a deliberately stale Aurelio page from November 2025 that still
  presents the discontinued Presto 200 as the flagship. Brand content holds
  fitting detail; its older pages mislead on availability, price and what
  connecting to the app requires.
- **`third-party/`** — editorial and review pages: a syndicated
  small-kitchens roundup, a syndicated grinder guide and a syndicated steam
  tip review (the class that carries the recommendations), a syndicated
  2025 grinder review whose compatibility verdict was true when written and
  is wrong now, and a review page captured from GadgetGrove **with no
  licence**: nobody granted one, so the manifest deliberately declares
  none and its licence state is unknown.

The corpus holds sixteen documents. The comparison runs sealed a
nine-document subset by manifest hash;
the routing experiment (`demo/jobs/commerce-routing/`, with its questions
and pre-registered split) uses all sixteen. Re-running the
`commerce-examples` recipe reads the full corpus and is a different
experiment by hash, as a changed rubric would be. All sixteen follow the
same shape: the retailer's data holds current prices, stock and cut-over
dates (a July price cut, a discontinued milk wand and hub); brand pages were
accurate for their dates and mislead at the current date; only editorial
recommends.

Each class directory carries its own `corpus.json` so each class is a
runnable corpus alone; the mixed root's `corpus.json` declares the same
dates and licences class-prefixed. The two layers cannot drift unnoticed:
`crates/commonmeasure-supply/tests/internal_corpus.rs` pins their agreement, and a
declaration for a document the scan will never read is a load error.

Licences use the manifest's per-document `licences` map (this bundle is the
reason it exists): product data under `fictive-retail/product-data-licence-v1`,
brand pages under each brand's reference, syndicated editorial under each
publication's reference, and the GadgetGrove capture under no licence, because
an aggregated corpus that declared one licence corpus-wide would misdeclare
either the licensed classes or the unlicensed capture.

The corpus is deliberately shaped so the source classes disagree in the
ways real commerce content disagrees:

- **Availability moved recently.** The Presto 200 was discontinued on
  15 July 2026 (product data); the brand's November 2025 page still sells
  it. A buyer answered from brand content alone is told about a machine the
  retailer no longer stocks.
- **Compatibility flipped at a date.** The Northglade G4 works with the
  Presto 300's auto-dose tray only from 28 March 2026 (adapter ring DR-2 +
  grinder firmware 2.1). BrewPrint's September 2025 review says that it
  does not, which was accurate for its date.
- **Only editorial recommends.** The structured data has no opinion; the
  small-kitchens pick exists only in the syndicated roundup. Product data
  alone can say what is in stock and how wide it is; it cannot say which
  to choose.

Nothing hostile is planted here. The injection screen runs at
admission over every document in every committed run below, and each
invocation records no matched rule. This is deliberate, and the runs' own
evidence pins it. `demo/injection/` is the committed evidence for what the
screen catches, and `demo/specialist/` for its stated blind spots.

## The comparison suites — `demo/jobs/commerce/`

Three questions, each asked identically under four supply conditions: the
class alone (`product-*`, `brand-*`, `editorial-*`) and all classes
together (`mixed-*`), selected by pointing `COMMONMEASURE_INTERNAL_CORPUS` at
the class directory or the mixed root. Within one question the four suites are
identical apart from `suite_version` and `label`: same job id, same prompt,
same objective (coverage 1.0, freshness 0.5), same `coverage_rubric`, same
`as_of` (2026-08-08, 180-day horizon), same citation requirement. The
rubric and as-of were declared once, before any run, and sealed in each
manifest; the acceptance test is that the results are reported
whatever they are and the suite is not tuned until mixing wins.

| question | what makes it hard |
|---|---|
| `*-availability` | the correct answer changed on 15 July 2026; the stale brand page contradicts it |
| `*-compatibility` | the correct answer changed on 28 March 2026; the 2025 review contradicts it |
| `*-recommendation` | the rubric needs the editorial pick *and* product facts, so no single class carries it |

The thirteenth suite, `governed-recommendation.json`, is the governed
condition: the same recommendation job over the mixed corpus in `strict`
mode with `required_licence` constraints naming the five declared licence
references. Source policy then refuses the unlicensed GadgetGrove capture
at admission, before inference, with the refusal and the licence state in
the evidence; the ungoverned `mixed-recommendation` run admits the same
document with its unknown licence state recorded. The licence state is
carried in both runs and enforced only where the job declares the
requirement.

## The comparison runs

Produced with the gateway up (the recorded runs are not in the public
repository, because their records carry the recording machine's paths):

```sh
just gateway-up
COMMONMEASURE_INFERENCE_ENDPOINT=http://127.0.0.1:3000/openai/v1/chat/completions \
  just commerce-examples
```

Answers are not byte-stable across regenerations (the gateway requests
temperature 0 but no byte-stability claim is made); the committed evidence
rests on the deterministic record: manifest hashes, admission decisions,
window composition, evaluation records and the selection arithmetic. Source
URLs are the internal adapter's `file://` URLs, absolute to the
checkout that generated them; cross-machine comparison is by manifest hash
and record structure, not by bytes.


## What is not here

The rubric is suite input, sealed by each run's manifest, and not corpus
data, so changing an item is a different experiment by hash. This bundle
measures supply mix under the runtime's own evaluators and claims nothing
about any external answer engine.
