---
title: Read the routing experiment in five minutes
---

# Read the routing experiment in five minutes

This walkthrough reads the committed routing experiment from a clean shell: 48 runs over the extended
Fictive Retail bundle (`demo/commerce/README.md`), a frozen routing rule,
and a holdout result published as a loss. It needs this checkout, a Rust
toolchain and the standard shell tools used below, and no credentials,
gateway or network. Every command here was run, in this
order, while the document was written. [`docs/READ-A-RUN.md`](READ-A-RUN.md) reads one run;
[`docs/READ-A-COMPARISON.md`](READ-A-COMPARISON.md) reads runs against each other; this document
reads a *prediction* against runs the predictor never saw. Terms like
run, suite, admission and manifest are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

The claim under test is the acceptance gate's routing line (`ROADMAP.md`
§Acceptance gate): that a routing rule
fitted on one set of jobs improves an operator-owned objective on a
holdout set. The result: **on this holdout the frozen rule loses**, with
mean objective 1.364 against the fixed default's 1.408. The loss is published under the anti-tuning clause
declared at registration: the rubrics and the split were declared once,
before any run, and the result is published whether the rule wins or loses.

## 0. The experiment's shape, and why it can be trusted

Twelve fictional commerce questions in three classes (availability,
compatibility, recommendation), each asked under four supply conditions —
one source class alone, or all classes mixed. The split into six fitting
and six holdout questions is mechanical and was registered before any run
(`demo/jobs/commerce-routing/split.json`, rule stated inside). The file
records its own registration: the `registered` date and
`registration_digest`, the SHA-256 over the population, the split rule and
the split as registered, which any reader can recompute:

```sh
python3 -c "
import json, hashlib
s = json.load(open('demo/jobs/commerce-routing/split.json'))
body = json.dumps({k: s[k] for k in ('population', 'split_rule', 'split')}, sort_keys=True, separators=(',', ':'))
print(hashlib.sha256(body.encode()).hexdigest() == s['registration_digest'])
"
```

Within one question
the four condition suites differ in nothing but their name:

```sh
diff demo/jobs/commerce-routing/product-g4-presto200.json demo/jobs/commerce-routing/mixed-g4-presto200.json
```

Two fields: `suite_version` and `label`. Same prompt, same rubric, same
as-of date (2026-08-13, 180-day horizon), same objective weights
(coverage 1.0, freshness 0.5). The supply is the only variable. All 48
committed runs are distinct experiments by hash:

```sh
python3 -c "
import json, glob
hashes = [json.load(open(path))['hash'] for path in sorted(glob.glob('demo/output/commerce-routing/*/manifest.json'))]
print(len(hashes), 'runs,', len(set(hashes)), 'distinct manifest hashes')
"
```

## 1. The rule, re-derived from the fitting runs

The frozen rule (`demo/jobs/commerce-routing/rule.json`,
`commerce-routing-rule/1`) was fitted only from the six fitting questions'
measured records: per class, the supply condition with
the strictly highest mean weighted objective. Re-derive it:

```sh
python3 - <<'PY'
import json, hashlib
split = json.load(open("demo/jobs/commerce-routing/split.json"))["split"]
rule = json.load(open("demo/jobs/commerce-routing/rule.json"))
for klass, sets in sorted(split.items()):
    means = {}
    for condition in ["product", "brand", "editorial", "mixed"]:
        scores = []
        for question in sorted(sets["fitting"]):
            run = f"demo/output/commerce-routing/{condition}-{question}"
            summary = json.load(open(f"{run}/summary.json"))
            weights = json.load(open(f"{run}/manifest.json"))["manifest"]["job"]["objective"]
            plan = next(p for p in summary["plans"] if p["provider"] != "none")
            scores.append(weights["coverage"] * plan["evaluation"]["coverage"]["fraction"]
                          + weights["freshness"] * plan["evaluation"]["freshness"]["fraction"])
        means[condition] = sum(scores) / len(scores)
    best = max(means, key=means.get)
    frozen = rule["routes"][klass]["condition"]
    print(f"{klass:15} argmax {best:8} frozen {frozen:8} agree {best == frozen}  "
          + " ".join(f"{c}={v:.3f}" for c, v in means.items()))
digest = "sha256:" + hashlib.sha256(rule["rule_text"].encode()).hexdigest()
print("digest recomputes:", digest == rule["rule_text_digest"])
PY
```

Availability and compatibility route to product data (mean 1.500 against
mixed's 1.425), recommendation to mixed (1.425 against editorial's
1.108); the rule text's digest recomputes. The offline test gate pins
this (`crates/commonmeasure-runtime/tests/routing_rule.rs`): editing a route or a
figure makes a test fail.

## 2. The holdout, read from the records

The frozen rule is applied to the six questions it never saw and compared
with two comparators. The **fixed default** is the mixed corpus, the
use-everything condition an operator without a rule runs (declared at
comparison time, not in the registration; all four conditions are
committed per question, so any other default re-derives from the same
records). **Per-run selection** is the per-question best over the four
measured conditions, which costs running every condition.

```sh
python3 - <<'PY'
import json
split = json.load(open("demo/jobs/commerce-routing/split.json"))["split"]
rule = json.load(open("demo/jobs/commerce-routing/rule.json"))
def objective(condition, question):
    run = f"demo/output/commerce-routing/{condition}-{question}"
    summary = json.load(open(f"{run}/summary.json"))
    weights = json.load(open(f"{run}/manifest.json"))["manifest"]["job"]["objective"]
    plan = next(p for p in summary["plans"] if p["provider"] != "none")
    return (weights["coverage"] * plan["evaluation"]["coverage"]["fraction"]
            + weights["freshness"] * plan["evaluation"]["freshness"]["fraction"])
totals = {"rule": [], "default": [], "selection": []}
for klass, sets in sorted(split.items()):
    for question in sorted(sets["holdout"]):
        scores = {c: objective(c, question) for c in ["product", "brand", "editorial", "mixed"]}
        route = rule["routes"][klass]["condition"]
        totals["rule"].append(scores[route])
        totals["default"].append(scores["mixed"])
        totals["selection"].append(max(scores.values()))
        print(f"{question:32} rule({route}) {scores[route]:.2f}  default {scores['mixed']:.2f}  best {max(scores.values()):.2f}")
for name, values in totals.items():
    print(f"mean {name:10} {sum(values)/len(values):.4f}")
PY
```

The rule wins three questions by 0.10 (its product route measures full
coverage at full freshness, where mixing admits older pages the rubric
never needs), ties the two whose route *is* the default, and loses
`g4-presto200` by 0.57. The means: **rule 1.364, default 1.408, per-run
selection 1.458**. The acceptance line is failed on this holdout, by the
sealed weights and measured fractions above.

## 3. Where the loss comes from

The losing question's rubric needs one fact from each source class: the
retailer's compatibility verdict, a design fact stated only on a brand
page, a workflow note only editorial carries. The committed records show
what each window held:

```sh
python3 - <<'PY'
import json
for condition in ["product", "mixed"]:
    summary = json.load(open(f"demo/output/commerce-routing/{condition}-g4-presto200/summary.json"))
    plan = next(p for p in summary["plans"] if p["provider"] != "none")
    print(condition, "window:", plan["source_count"], "sources")
    for item in plan["evaluation"]["coverage"]["items"]:
        where = (item["matched"] or {}).get("reference", "").split("corpus/")[-1]
        print(f'  {item["item"]:16} covered={item["covered"]}  {where}')
PY
```

The product route covers 1 of 3 (objective 0.83); the mixed window
covers 3 of 3 (1.40), each match recorded with the document and span
that carries it. One within-class outlier outweighs three 0.10 wins:
a constant per-class route is fragile where a class's questions are not
alike. This favours per-run measurement over static rules, and per-run
selection is the best line above.

## 4. The failure path

A rule is also a dependency: the route it names can be missing. The
failure path is held by a test that runs a genuine holdout suite, under
the condition the frozen rule routes its class to, against a corpus root
that does not exist:

```sh
cargo test -p commonmeasure-runtime --offline --test routing_failure_path
```

The published record is the outcome [`docs/FAIL-POLICY.md`](FAIL-POLICY.md) §5 requires:
the plan `unavailable`, a `provider_unavailable` gap naming the
unreadable root, no source admitted from anywhere, the token count and
coverage unmeasured rather than zero, no answer, and the sealed record on
disk for a reader.

## 5. What this shows, and what it does not

Everything above is deterministic measurement of admitted windows:
phrase coverage against pre-declared rubrics, and date arithmetic
against declared document dates. No model answer was produced or judged:
these runs carry an explicit inference gap, and answer quality keeps its
stated abstention throughout. The corpus, rubrics and questions are
authored and fictional, so the figures demonstrate the experiment's
machinery (pre-registration, sealed identities, a frozen prediction, a
published loss) and not a market fact about real content. The bounded
claim is the registration's: on this rubric family, on this holdout.
