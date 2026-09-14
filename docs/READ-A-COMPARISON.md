---
title: Read the commerce comparison in five minutes
---

# Read the commerce comparison in five minutes

This walkthrough reads the committed commerce comparison, `demo/output/commerce/`,
from a clean shell: thirteen runs over one obviously fictional retail bundle
(`demo/commerce/README.md`). It needs this checkout, a Rust toolchain and
the standard shell tools used below, and no credentials, gateway or
network. Every command here was run, in this order, while the document was
written. [`docs/READ-A-RUN.md`](READ-A-RUN.md) is the single-run primer; this document reads
runs *against each other*. Terms like run, plan, manifest and admission are
defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

The claim under test is the mixed-supply claim: that combining source
classes — structured product data, the brand's own content, third-party
editorial — beats any class alone. The suites were declared, and their
rubrics and as-of dates sealed, before any plan ran, and the result is
committed as produced. Mixing does not win every question; the comparison
shows where mixing wins and where it costs.

## 0. One experiment, four supply conditions

Each of the three questions (availability, compatibility, recommendation)
exists as four suites: the class alone (`product-*`, `brand-*`,
`editorial-*`) and all classes together (`mixed-*`), run by pointing
`COMMONMEASURE_INTERNAL_CORPUS` at the class directory or the mixed root
(the `commerce-examples` recipe in the `justfile`). Within one question the
conditions differ in nothing but their name:

```sh
diff demo/jobs/commerce/product-recommendation.json demo/jobs/commerce/mixed-recommendation.json
```

Two lines: `suite_version` and `label`. Same job id, same prompt, same
objective (coverage weighted 1.0, freshness 0.5), same rubric, same as-of,
same citation requirement, same token budget. The supply is the only
variable, which is the discipline [`docs/contracts/experiment.md`](contracts/experiment.md) requires.
Each run's manifest
seals its suite, so all thirteen runs are distinct experiments by hash:

```sh
python3 -c "
import json, glob
hashes = [json.load(open(path))['hash'] for path in sorted(glob.glob('demo/output/commerce/*/manifest.json'))]
print(len(hashes), 'runs,', len(set(hashes)), 'distinct manifest hashes')
"
```

## 1. The comparison, read from the records

Coverage and freshness are read from each plan's evaluation records; the
objective is arithmetic on the weights sealed in each run's own manifest —
the same arithmetic the router's selection line publishes inside each run
(`s:/selection`), applied across runs that share a question, a rubric and an
as-of date:

```sh
python3 - <<'PY'
import json, glob
for path in sorted(glob.glob("demo/output/commerce/*/")):
    run = path.rstrip("/")
    summary = json.load(open(f"{run}/summary.json"))
    manifest = json.load(open(f"{run}/manifest.json"))
    objective = manifest["manifest"]["job"]["objective"]
    plan = next(p for p in summary["plans"] if p["provider"] != "none")
    coverage = plan["evaluation"]["coverage"]["fraction"]
    freshness = plan["evaluation"]["freshness"]["fraction"]
    score = objective["coverage"] * coverage + objective["freshness"] * freshness
    print(f'{run.split("/")[-1]:27} coverage {coverage:.2f}  freshness {freshness:.2f}  objective {score:.2f}')
PY
```

Grouped by question, the committed runs read:

| question | product | brand | editorial | mixed |
|---|---|---|---|---|
| availability | **1.50** | 0.40 | 0.40 | 1.45 |
| compatibility | **1.50** | 1.07 | 0.40 | 1.40 |
| recommendation | 1.17 | 0.40 | 0.73 | **1.45** |

**Mixing wins the recommendation question and loses the other two.** The
recommendation rubric needs the editorial pick *and* the product facts, so
no single class covers it: product data measures 2/3, editorial 1/3, mixed
3/3. Availability and compatibility are covered by the product-data class
alone at freshness 1.00, so the mixed window's 2025-dated documents (the
stale brand page in both windows, and the outdated review in the
compatibility window, each within the rubric's blind spot and outside the
180-day freshness horizon) cost 0.10 and 0.20 of freshness and add nothing
the objective scores. That trade-off is what the
mixed-supply claim says: mixing helps where the classes complement each
other, and adding documents to a class that covers the rubric with fresh
documents does not improve it.

Every figure above is a plan's own evaluation record: the coverage record
names the matched phrase, part and byte span per rubric item, and the
freshness record names each part's declared date, age and provenance.
Resolve any of them as [`docs/READ-A-RUN.md`](READ-A-RUN.md) does, e.g.:

```sh
python3 -c "import json; d=json.load(open('demo/output/commerce/mixed-recommendation/summary.json')); \
print(json.dumps(d['plans'][1]['evaluation']['coverage']['items'][0], indent=2))"
```

## 2. The governed condition

The thirteenth run is `governed-recommendation`: the same recommendation
job over the same mixed corpus, under source policy. The whole difference
between it and the ungoverned mixed run is declared in the suite:

```sh
diff demo/jobs/commerce/mixed-recommendation.json demo/jobs/commerce/governed-recommendation.json
```

A different job id, `policy_mode` moving from `observe` to `strict`, and
five `required_licence` constraints naming the bundle's declared licence
references. The third-party class holds one document those constraints
cannot accept: the GadgetGrove capture, for which nobody granted a licence
and whose manifest entry deliberately declares none. One source record per
run shows the difference:

```sh
python3 - <<'PY'
import json
for run in ("mixed-recommendation", "governed-recommendation"):
    summary = json.load(open(f"demo/output/commerce/{run}/summary.json"))
    plan = next(p for p in summary["plans"] if p["provider"] != "none")
    source = next(s for s in plan["sources"] if "gadgetgrove" in s["url"])
    print(run, "->", json.dumps(
        {key: source[key] for key in ("licence", "admitted", "admission_reason")}))
PY
```

The licence state, unknown, is identical in both records and is carried in
the evidence either way; only the ruling differs. In the ungoverned run the
document is admitted with its unknown state on the record. In the governed
run it is refused at
admission: the plan's `policy_decisions` carry the refusal with the reason
("No supplier declared a licence this job accepts" — unknown is not
permitted, [`docs/FAIL-POLICY.md`](FAIL-POLICY.md)), the window assembles nine parts instead
of ten with the refused document in none of them, and inference runs only
over what admission let cross:

```sh
python3 - <<'PY'
import json
summary = json.load(open("demo/output/commerce/governed-recommendation/summary.json"))
plan = next(p for p in summary["plans"] if p["provider"] != "none")
print(json.dumps(plan["policy_decisions"], indent=2))
window = plan["evaluation"]["coverage"]["inputs"]
print(len(window), "window parts;",
      "gadgetgrove admitted:", any("gadgetgrove" in part["reference"] for part in window))
PY
```

The refusal costs the governed run no coverage (3/3 without the capture)
and 0.01 of freshness: the refused capture is dated 18 July 2026, fresh
under the sealed as-of, so the fraction drops from 0.90 over ten parts to
0.89 over nine, which is 0.006 of objective. That cost is this rubric's
result, read from the record; it is not a general claim about what
governance costs.

## 3. The dossier and the console

The full account of any run is its dossier — header, sealed identity,
sources with licence states, refusals with reasons, window composition,
evaluation records, selection arithmetic — every claim ending in a citation
into the artefacts:

```sh
cargo run -q -p commonmeasure-cli -- inspect demo/output/commerce/governed-recommendation
```

The dossier carries the refusal, the licence states and the admission
reasons; the console does not render run directories.

## 4. Check the seal yourself

Each figure above can be recomputed from the files. The sealed provider
response (here the internal adapter's own query response, the one document
its envelopes are derived from) hashes to the acquisition record's claim:

```sh
sha256sum demo/output/commerce/mixed-recommendation/responses/internal-only.json
python3 -c "
import json
summary = json.load(open('demo/output/commerce/mixed-recommendation/summary.json'))
plan = next(p for p in summary['plans'] if p['provider'] != 'none')
print(plan['acquisition']['response_hash'])
"
```

Regeneration (`just gateway-up`, then `COMMONMEASURE_INFERENCE_ENDPOINT=... just
commerce-examples`) re-answers with a live gateway: answers are not
byte-stable and `file://` URLs are absolute to the generating checkout,
but the admission decisions, window composition, evaluation
records and the comparison they carry re-derive, and the manifests keep
naming the same sealed experiments. If anything disagrees — a summary and
manifest carrying different hashes, an evidence log that does not hash to
the summary's claim — the dossier reports a `MISMATCH`, and the
directory does not hold one coherent run ([`docs/READ-A-RUN.md`](READ-A-RUN.md) §If
something disagrees).
