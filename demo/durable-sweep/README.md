---
domain: edge
audience: contributor
---

# Local durable retrieval sweep

This example runs two synthetic questions in agency and supplier configurations
through the same unchanged Common Measure replay CLI. It needs no supplier or
model credentials. It tests a small SQLite queue around existing acquisition
and source records, not a hosted experiment service.

## Run

From the repository root, with Rust dependencies already fetched and `uv`
installed:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build -p commonmeasure-cli --locked --offline
uv run --no-project --offline python demo/durable-sweep/proof.py \
  --root target/sweep-agency --binary target/debug/commonmeasure --use-case agency
uv run --no-project --offline python demo/durable-sweep/proof.py \
  --root target/sweep-supplier --binary target/debug/commonmeasure --use-case supplier
uv run --no-project --offline python -m unittest discover -s demo/durable-sweep -v
```

Each root is a private local example namespace. Use fresh roots for independent
experiments. Repeating a command returns the same execution identity and does
not acquire again. `SWEEP_BINARY` selects the binary for tests; the default is
`target/debug/commonmeasure`. No installed Edge home or source policy is edited.

The wrapper creates synthetic Exa-shaped responses and a hash-checked replay
manifest. The production replay transport serves them on loopback and uses the
real adapter, parser, job constraints, admission and sealed evidence path.
The request's question does not change the fixed fixture's response. These are
synthetic workflow inputs, not recorded supplier performance or licensed content.
The runtime labels the replay transport `replay-tested`; that label does not
establish a real supplier integration.

Children receive an explicitly constructed environment with isolated homes and
no inference endpoint or credentials. The ordinary batch runtime attempts no
inference when it is unconfigured. Its answer-oriented plan can therefore be
`unavailable` despite successful retrieval. This example's `retrieved` outcome
means a verified acquisition with admitted sources, never a completed answer.
It does not weaken or change the ordinary run's status.

## State and evidence

`queue.sqlite` stores immutable definition JSON, its digest, a unique
project/slot execution identity, state, a fencing counter and held replay call
units. Transactions use SQLite `BEGIN IMMEDIATE` and `synchronous=FULL`.
Duplicate submissions with changed definitions are refused. Changed questions,
constraints or controls change the definition digest. A digest here is not a
complete hosted comparability/cohort contract.

Each execution directory holds synthetic fixtures, private suite inputs,
separate ordinary case run directories and `export.json`. The example export
has its own `contextops-local-sweep/v1` shape, validated by `validate_export`.
It is neither the comparison ZIP nor the SimpleQA benchmark format. Its
metrics-only allowlist excludes questions, answers, source text, URLs and
credentials. Cell references resolve locally to SHA-256-bound summaries, which
retain the runtime manifest and source evidence. Export access recomputes its
stored digest; tests also recompute source response and evidence hashes.
Producer hashes identify the local wrapper and binary files.

Selected cases remain in the denominator after refusal. Charges, supplier
latency and model/judge usage stay null with missing reasons. Returned bytes,
admitted whitespace-word estimates and provider model tokens remain different
stages. Loopback latency is not supplier latency. The fixed synthetic fixture
reports no price; money ceilings are refused. Held units bound the selected
replay workload only, not money, provider billing or fleet-wide allowances.
Completed work consumes those units for the lifetime of the example root.

## Interruption checks

The test suite exits real wrapper processes at these boundaries:

| Boundary | Recovery | Held units |
|---|---|---|
| `--crash before-send`: reservation saved, no child launched | `not_sent`; no automatic requeue | Released |
| `--crash after-child`: real replay acquired and published a case, before queue indexing | `outcome_unknown`; no automatic resend | Retained |
| `--crash after-evidence`: export saved and indexed | Completed evidence remains readable | Consumed |

The `dispatched` intent is saved **before** launching the child. It cannot prove
that any HTTP request was sent. Conservative uncertainty may include unsent
work or more cells than actually ran. Recovery is an explicit local operator
operation and never releases possible exposure by expiry. Actual paid-request
reconciliation and deliberate bounded reruns are not implemented.

Tests exercise concurrent reservations and duplicate submissions, revocation,
cross-project object/export lookup refusal, stale state transitions, policy
refusal, corrupted replay bytes, export integrity and export-write failure.
A focused timeout injection proves conservative queue handling, not transport
recovery. Fencing guards local state transitions and checks subsequent child
launches; it cannot atomically fence an external send or stop an already
launched child. It is not a distributed lease implementation.

## Boundaries

Scope arguments are trusted local operator inputs, not authenticated authority.
There are no hosted artefact URLs, supplier custody, delegated service principals
or remote access controls. Separate homes and empty child environments prove
only those local boundaries; they do not provide an OS sandbox or customer
isolation. Keep this example limited to synthetic data.

The example does not resolve the session source policy into the batch job.
Its explicit suite constraints exercise batch admission unchanged. Unsupported
batch-policy refusal remains covered by existing benchmark CLI tests, not by
this wrapper. Unattended registration/renewal and durable onward reporting
need their own integrated acceptance; replay does not incur publisher duties.

Grounded answers, reference-answer secrecy, aggregate charge reconciliation,
real billable after-send crashes, schedule pause/cancellation/overlap, missed
slots, daylight-saving behaviour, remote lease enforcement and hosted disclosure
are not established. No catch-up scheduler, live supplier/model call, telemetry
forwarding or deployment is provided. Existing [run-output](../../docs/contracts/run-output.md),
[experiment](../../docs/contracts/experiment.md),
[benchmark](../../docs/contracts/simpleqa-benchmark.md) and
[comparison-export](../../docs/contracts/comparison-export.md) contracts retain
their meanings.
