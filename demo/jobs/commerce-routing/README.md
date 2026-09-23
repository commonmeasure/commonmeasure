# The routing experiment's questions and pre-registered split

This directory is the routing experiment's job population: the
questions on which a routing rule will be fitted and then tested on
questions it did not see. Registered before any run of any suite in this directory; `split.json`
carries the registration date and digest. Terms like
suite, run, admission and manifest are defined in `docs/GLOSSARY.md`.

Everything here follows the commerce comparison's discipline (`demo/commerce/README.md`):
the corpus is obviously fictional, and each question's rubric and as-of
date are declared here, once, sealed into every run's manifest. The commerce comparison's
clause applies here: the results are reported whatever they are, and the
suites are not tuned until mixing wins.

## The population

Twelve questions over the extended Fictive Retail corpus
(`demo/commerce/corpus/`), four per class:

| class | question ids |
|---|---|
| availability | `brewlink-hub-availability`, `g4-price-today`, `milk-wand-availability`, `presto200-filter-availability` |
| compatibility | `accessory-carryover`, `app-without-hub`, `g4-out-of-box`, `g4-presto200` |
| recommendation | `grinder-choice`, `milk-accessory-choice`, `narrow-counter-setup`, `presto200-replacement` |

Each question exists as four suites: the class alone (`product-*`,
`brand-*`, `editorial-*`) and all classes together (`mixed-*`). The suites
are run by pointing `COMMONMEASURE_INTERNAL_CORPUS` at the class directory or
the mixed root, as the commerce comparison did. Within one question the four
suites differ only in `suite_version` and `label`: same job id, same
prompt, same objective (coverage 1.0, freshness 0.5), same rubric, same
as-of (2026-08-13, 180-day horizon), same citation requirement, same
token budget. The supply condition is the only variable.

The three questions the commerce comparison ran and committed
(`demo/jobs/commerce/`) are not in this population: their results are
published, so fitting on them would contaminate the split.

## The pre-registered split

`split.json` in this directory is the machine-readable registration. The
split rule is mechanical so that nobody chose which set each question went to:
within each class, question ids sort alphabetically; the first and third
are the fitting set, the second and fourth the holdout set.

| class | fitting | holdout |
|---|---|---|
| availability | `brewlink-hub-availability`, `milk-wand-availability` | `g4-price-today`, `presto200-filter-availability` |
| compatibility | `accessory-carryover`, `g4-out-of-box` | `app-without-hub`, `g4-presto200` |
| recommendation | `grinder-choice`, `narrow-counter-setup` | `milk-accessory-choice`, `presto200-replacement` |

The rule to be fitted reads only the fitting runs' measured records; the
holdout suites are run only after the rule is frozen as a versioned
artefact, and the comparison is published whatever its result. The split,
the rubrics and the as-of dates do not change after this registration;
changing any of them is a different experiment by hash.

## The frozen rule — `rule.json`

Frozen after the 24 fitting runs and before any holdout run. The rule is
`commerce-routing-rule/1`, digest
`sha256:94214704a6b0642c324a9c81eac191fca15d52422e5db415d5e729da2b2f2d67`
over its own rule text: route availability and compatibility jobs to the
product-data corpus, recommendation jobs to the mixed corpus. The
derivation is deterministic and complete: per class, the supply
condition with the strictly highest mean weighted objective (coverage
1.0, freshness 0.5, the weights sealed in every fitting manifest) over
that class's fitting questions; a tie would have left the class unrouted,
and none occurred. `rule.json` carries the full derivation: every fitting
run cited by manifest hash with its measured coverage and freshness
fractions, so a reader can redo the arithmetic from this directory alone.
`crates/commonmeasure-runtime/tests/routing_rule.rs` re-derives the rule from those
committed records in the offline gate, so an edited route or a reworded
rule text fails a test. No holdout
record contributed to the rule.

## The outcome

The holdout runs are committed and the rule lost: mean objective
1.364 against the fixed default's 1.408, with per-run measured selection
best at 1.458. The result is published under the anti-tuning clause
above. The recorded runs are not in the public repository, because their
records carry the recording machine's paths.

## What is deliberately out

As in the holdout experiment: no LLM judge (quality keeps its abstention), no
exchange rate between fractions and cost or latency, no live spend, and
no claim beyond this rubric family on this holdout.
