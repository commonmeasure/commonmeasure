---
title: Read a run in five minutes
---

# Read a run in five minutes

This walkthrough reads two committed runs from a clean shell. It needs this
checkout, a Rust toolchain and the standard shell tools used below, and no
credentials, configuration or source code. Every command here was run, in
this order, while the document was written. Terms like run, plan, manifest
and replay are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

A committed run carries the contract version it was sealed under and is read
as it declares itself; the dossier's `contract` line says which. Step 0 opens
`demo/output/latest/`, a run at the current contract with a rubric-ranked
selection. Steps 1 to 5 read `demo/output/replay/`, an earlier-contract run
whose answers came from a real gateway, so that inference and grounding lines
can be read too.

The goal is to answer five questions a sceptical reader brings to any
published run — what actually ran? what data did it see? what was real? why
was this selected? where is the evidence? — using local artefacts alone.

A run directory holds four readable JSON artefacts (`summary.json`,
`manifest.json`, `evidence.ndjson`, and `replay.json` for replay runs) plus
the sealed provider bytes under `responses/`. The format is
[`docs/contracts/run-output.md`](contracts/run-output.md); nothing below requires reading it.

## 0. Print a dossier (first build takes a few minutes; the read is fast)

```sh
cargo run -q -p commonmeasure-cli -- inspect demo/output/latest
cargo run -q -p commonmeasure-cli -- inspect demo/output/replay
```

With the binary installed (`cargo install --path crates/commonmeasure-cli`), the same
commands are `commonmeasure inspect <dir>`, and there is no build wait. The
first dossier's `contract` line reads `contextops-run/v7 — the current run
contract`; the second's names `contextops-run/v3` and says the run predates
the current contract and is read as it declares itself. Its `selection`
line names the plan the coverage and freshness ranking chose and the rubric
it measured against; the rest of this walkthrough reads the second run.

The dossier is computed from the run directory on every invocation and writes
nothing back: the artefacts stay the only record. Its header names the files
it read, and **every claim ends with a citation** telling you where to check
it: `s:<pointer>` is a JSON pointer into `summary.json`, `e:<n>` is the
`evidence.ndjson` line whose `seq` is `n`, `r:<pointer>` points into
`replay.json`, `m:<pointer>` into `manifest.json`.

## 1. What actually ran?

Read the header block. For this run it says: suite `eu-ai-act-replay/v1`, one
research job, mode `replay` — provider responses served from verified
recordings over loopback, no external provider contacted, and inference *not*
replayed: the answers came from a real TensorZero gateway at run time. Then
one `plan` block per supply route: a no-context baseline, then Exa,
Firecrawl, Tavily and TollBit, each with its status on the header line
(`completed` or `unavailable`) and the reason in plain words.

The `manifest` line is the run's identity. It prints the hash, which is
present identically in `summary.json` and `manifest.json` and is
**recomputed from the `/manifest` object during this inspect**, so a
manifest whose sealed content was edited under an untouched hash reads
`SEAL MISMATCH` instead of corroborated. It also prints the sealed inputs
the manifest names, read from `manifest.json`'s own keys so the list is the
run's and not this document's ([`docs/contracts/run-output.md`](contracts/run-output.md) §Manifest
says what each one is and why the inference endpoint is deliberately outside
the hash). The `evidence` line tells you the append-only log is complete and
that its digest was **recomputed from the file** during this inspect rather
than repeated from the summary.

## 2. What data did it see?

Each plan's `sources` lines list what was discovered and what was admitted,
with a content hash per source. The `context` line shows what entered the
model's window: for `exa-only`, 45 of 58 retrieved words survived
deduplication, and each removed span is named with its character offsets and
the retained source span where the same words were first kept, down to
`chars 75-96`. The `question` line is the exact prompt; `token basis`
reminds you every count is whitespace words, not model tokens.

## 3. What was real?

Each plan's `bytes` line states where the provider bytes came from, read from
the per-acquisition replay record: `recorded replay, not live capture` for the four provider
plans here, each naming its capture date, recorded endpoint and recording
file. The hash claim on this line is recomputed, not repeated, and has three
states: with the bytes present, as here, it says "the sealed response hash
equals the recorded hash, recomputed here from the sealed bytes, so these
are those bytes"; bytes that hash differently are reported as a `MISMATCH`;
and a directory published without `responses/` says the recorded hash "rests
on the runtime's own matches_recorded_input record and is not verified
here"; the dossier states the absence and does not vouch for the hash. A live run
prints `live capture` on this line instead, and an internal-corpus read
prints `local capture` (see `demo/output/tensorzero/` for one). The
`inference` lines are the live half of this run: real gateway calls with
token counts, latencies and request ids.

Check a sealed hash yourself rather than trusting the tool:

```sh
sha256sum demo/output/replay/responses/exa-only.json
```

The digest equals the sealed hash printed on the `exa-only` plan's `sealed`
line, and also the recorded hash in its `bytes` citation, because this is a
replay run.

## 4. Why was this selected?

The `selection` line at the end. In this run: **no plan was selected**, and
the dossier quotes the recorded reason — the job's objective weights quality,
coverage, freshness and policy risk, none of which this run measured, and the
router does not publish a verdict it did not measure. (Coverage and
freshness are measurable, but only against a suite-declared rubric and
as-of date, which this suite does not declare; `demo/output/latest/` is the run
whose objective weights exactly those measured terms, and its selection
line names the winning plan and the rubric that chose it.) Each plan's
`grounding` line accounts for evaluation: this suite does not set
`require_cited_answer`, so the dossier says the evaluator checked nothing
rather than inventing a measurement (a run from a suite that sets it shows
verdict counts and per-citation lines here instead: supported,
contradicted, uncovered and unavailable, kept distinct, the counts tallied
from the citation records themselves; `demo/output/cited/` is one). A plan
with an answer also carries a `fidelity` line, the answer's claims each
supported, contradicted or unsupported against the window with the span
cited, and, where the job declares a judge, a `judge` line with the judge's
agreement figure beside those verdicts (`demo/output/cited/` again).
Absences are stated the same way throughout: TollBit's plan shows two
sources `refused` with the recorded reason (the provider returned no
excerpt), an undisclosed charge reported as unknown rather than zero, and an
`unavailable` field where the gateway declined to name its upstream provider.

## 5. Where is the evidence?

Pick any claim and resolve its citation. The Exa `bytes` line cites
`s:/plans/1/acquisition/replay`:

```sh
python3 -c "import json; d=json.load(open('demo/output/replay/summary.json')); \
print(json.dumps(d['plans'][1]['acquisition']['replay'], indent=2))"
```

(`jq '.plans[1].acquisition.replay' demo/output/replay/summary.json` does the
same if you have jq.) The plan header cites `e:6`, the append-only log's
record of the same completion:

```sh
grep '"seq":6,' demo/output/replay/evidence.ndjson | python3 -m json.tool
```

The binding citation `r:/bindings/0` pairs the recorded hash with the sealed
hash:

```sh
python3 -c "import json; d=json.load(open('demo/output/replay/replay.json')); \
print(json.dumps(d['bindings'][0], indent=2))"
```

and its `source`, `exa/exa-search.json`, is the committed capture under
`demo/recon/`, whose `demo/recon/exa/NOTES.md` records how and when those
bytes were obtained and what was redacted. That is the whole chain: dossier
line → summary record → evidence log line → replay binding → committed
recording → provenance notes.

## If something disagrees

The dossier verifies where it can instead of repeating: a summary and
manifest carrying different hashes, a manifest whose `/manifest` object no
longer hashes to its own seal, an evidence log that does not hash to the
summary's claim, a verdict tally that is not the tally of its citation
records, or a sealed response that no longer matches its recording
are each reported as a `MISMATCH` line rather than displayed as
provenance. If you see one, the directory does not hold one coherent run;
`crates/commonmeasure-cli/tests/inspect_dossier.rs` holds the tests that verify those
reports.
