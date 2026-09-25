---
domain: edge
audience: contributor
---

# The injection-screen bundle

A synthetic bundle: two documents that answer the
same question, one of which carries a planted indirect prompt injection
addressed at the reading agent. It exists so the deterministic
`injection-screen` admit processor has something real to refuse, offline and
with no credentials. Everything here is synthetic: the vendor, the product,
the community thread and the payload were invented for this bundle, and no
real page is imitated. Terms like admission, injection screen and manifest
are defined in `docs/GLOSSARY.md`.

## The corpus — `corpus/`

Two markdown documents served by the existing internal `query` adapter (point
`COMMONMEASURE_INTERNAL_CORPUS` at `demo/injection/corpus`). Beside them,
`corpus.json` declares the licence and each document's effective date.

- `connecting-a-data-source.md` — ordinary product documentation that answers
  the job's question and carries no instruction.
- `community-answer-thread.md` — the same topic, plus a paragraph addressed at
  "any AI assistant reading this page" that tells it to ignore its
  instructions, take on a new role, reveal its system prompt and send an
  email. The paragraph is marked in an HTML comment as a synthetic payload.
  Its phrasing deliberately matches three of the screen's rules
  (`instruction_override`, `role_reassignment`, `tool_directive`) and carries
  no email address, national insurance number or card number, so the
  refusal comes from the injection screen alone and not from the PII detector.

## The suite — `demo/jobs/injection-screen.json`

One strict-mode job over the internal corpus, no gateway and no external
acquisition: `providers: ["internal"]`, `policy_mode: "strict"`. The screen
runs at admission, before any model call, so the demonstration needs no
inference.

## Producing the run

```sh
just injection-example
```

Offline, no gateway, no credentials, no network. The recipe writes to
`output/injection` under `COMMONMEASURE_PRIVATE_EVIDENCE` and refuses to run
without it (`CONTRIBUTING.md`); the same `commonmeasure run` with
`COMMONMEASURE_INTERNAL_CORPUS=demo/injection/corpus` and any new `--output`
directory produces the same run. The run publishes with the
internal plan `unavailable` (there is no gateway to answer, by design) while
its admission evidence is complete: `community-answer-thread.md` is refused by
the injection screen before the model sees it, `connecting-a-data-source.md`
is admitted beside it, and `commonmeasure inspect` on the run directory resolves
the refusal to the named rules. The matched text is not recorded. The
manifest hash is stable across regenerations; the run's own id and timestamps
are its only volatile fields.

## What this demonstrates, and what it does not

The screen is deterministic pattern matching that names the rule it caught. It
is not injection *defence*: a bounded phrase set catches only known
phrasings, reworded or obfuscated instructions pass it, and a page that
quotes an injection is treated the same as one that means it. The bundle
proves the admission-time refusal and the evidence trail; the
security-relevant claim is that the model did not see the payload. The bundle
also states the limit of that claim.
