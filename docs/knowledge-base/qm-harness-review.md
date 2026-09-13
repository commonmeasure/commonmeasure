---
title: "QM: a multi-harness, cross-company agent core, reviewed as demand"
draft: true
---

# QM: a multi-harness, cross-company agent core, reviewed as demand

As of 5 August 2026. Source: https://github.com/yc-software/qm at commit
`866764e`. Every claim below is what the code and its tests say, not what a
running deployment was observed to do. File paths are relative to that
repository at that commit.

QM is reviewed here as a buyer-side system: it is the harness codebase most
relevant to this product. It builds admission screening and
provenance labels itself, and it does not value what was admitted, select
between sources, or keep an evidence-grade record. Those are the parts this
product supplies.

## What QM is

A multiplayer agent harness for one company. One open-source TypeScript core
(Fastify + Postgres); per-person and per-room scopes, each with its own memory,
files, keychain, crons, skills and durable sandbox (Fly microVMs, AWS microVMs
or local Docker). Four coding harnesses — Pi (in-process), OpenCode (loopback
HTTP sidecar), Codex (JSON-RPC subprocess), Claude Code (Agent SDK) — drive the
same core through one shared tool surface (`src/harness/pi-tools.ts`), so every
tool call from every harness round-trips through core, where it is ledgered,
screened and gated. Surfaces (Slack, web, admin, public portal) are plugins
over an HMAC-signed HTTP API.

The cross-company layering: upstream open core → per-org deployment directory
(`deploy/layers/<org>/`) validated by a versioned CLI contract → optionally a
private fork whose core stays byte-identical to upstream, maintained by two
skills (`update-qm` merges downstream; `upstream-pr` scrubs org identifiers and
sends fixes up).

Engineering quality is high: ~470 test files including full-app integration
tests, and a `SECURITY.md` that enumerates its own limitations and pins them
with a doc test.

## What QM validates about the thesis

**Admission control at the crossing, driven by provenance labels.** QM's
default "Auto" posture screens provenance-labelled external data before it
reaches the model. Every screened payload carries a source label (`sender`,
`overheard`, `attachment:<name>`, `tool_result:<tool>`, `prior-turn:<role>`,
…) assembled in `src/core/orchestrator.ts`; the classifier's system prompt
reasons about those labels; a flagged input is quarantined with a
`securityTainted` mark that permanently excludes it from model context. A
harness vendor built policy-before-the-crossing on its own initiative.

**Pervasive provenance labelling — admitted incomplete.** Every session entry
and tape row carries a `scopeLabel`; a tool result fetched from another room is
labelled with the *source* room's scope, and history is filtered by unanimous
audience entitlement over those labels (`src/resolution/context-filter.ts`).
Memory carries in-band provenance — `(YYYY-MM-DD)` capture dates,
`(said in #room)` source tags — and an untrusted author's claimed provenance
is demoted to `[claimed source: X]` so forged provenance cannot impersonate
witnessed provenance (`src/memory/memory-service.ts`). Their own `SECURITY.md`
concedes: "model-context entries do not yet carry complete origin labels for
every granted read." The harness wants what the envelope model provides and
has not finished building it.

**Tighten-only policy scopes.** The org security posture is a floor narrower
scopes can only tighten, composed at write time
(`src/resolution/config-store.ts`). Command policy has an org floor of
predeclared rules over a serious shell normaliser, with the honest caveat that
it is "a speed bump, not a sandbox boundary."

**Deliberate context assembly, measured only for latency.** The system prompt
is built cached-prefix-first behind a sealed stable-prefix boundary asserted
byte-identical across turns; compaction never separates a tool call from its
result; the tape fold's stated invariant is "duplication, never amnesia"
(`src/harness/tape-fold.ts`). Cache hit ratio and stable-prefix miss rate are
recorded per turn. It is bounded by two independent token estimators (a
sampled-and-extrapolated tokenizer for compaction; chars/4 for the output
guard), neither treated as an observation with a named basis.

## The gap QM leaves is this product

- **Spend is capped, never valued.** Budgets are a flat-rate estimate over
  input tokens per rolling window (`src/ratelimit/budget.ts`). Turn metrics
  are latency-only. Real per-call usage lands in `session_llm_requests`, but
  the Codex and mock adapters hardcode `costUsd: 0` — unknown represented as
  zero, exactly what [`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §7 forbids here. Nothing asks
  whether admitted content was worth acquiring: no coverage, no freshness, no
  grounding, no selection between supply routes.
- **"Everything it does is audited" does not survive inspection.** Individual
  tool calls are not audit rows (only `reach_exec` and `file_share` are);
  agent memory writes via the memory tool bypass audit; config-change events
  record no values; audit writes are fire-and-forget (a database outage drops
  the row while the action completes); the `detail` field is written but never
  returned by the read API; there is no retention policy and no integrity
  chain; without `DATABASE_URL` audit silently lands in a RAM ring buffer.
  Their own framing is correct — "audit records support investigation; they
  do not prevent an action" — and is the demand-side argument for
  evidence-grade recording as a product.
- **Screening fails open** with a text banner (`[NOT security-screened — …]`)
  as the only compensating control on most paths (mid-turn steers fail
  closed).

## Integration sockets

QM's screening proxy (`SECURITY_SCREEN_BACKEND=proxy`, posting
`{text, hook, metadata}` and expecting `{score, threshold, primary_outcome}`),
its single `ToolContext` mediation seam and its host-side sealed
`session_llm_requests` window are the three sockets an external component
can use. What Common Measure offers on the proxy socket, and its status, is
stated in [`docs/contracts/host-integration.md`](../contracts/host-integration.md) §3.

## Patterns worth adopting

- **The `upstream-pr` scrub is the egress filter's design sibling.** Before
  any commit leaves a private fork, a term list is derived from the org's own
  layer files (config slugs, URL hosts, secret names, Slack manifest,
  Terraform vars, teammate emails); the scrub refuses to run if the list is
  empty; scanning is per-commit rather than per-net-diff because renames and
  add-then-delete hide content from net diffs (`.codex/skills/upstream-pr/`).
  The relay's per-engagement egress filter should be deterministic
  rather than a prompt-driven skill, but "derive deny terms from the
  engagement's own declared artefacts, refuse on empty" is the right shape.
- **Provenance defanging.** Supplier-claimed provenance is preserved but
  demoted to a visibly weaker form unless the recording path witnessed it.
  Cheap and applicable to any operator record carrying self-reported
  provenance.
- **Contract clause status vocabulary.** QM's deployment-directory contract
  doc labels every clause ENFORCED / VALIDATED-ONLY / RESERVED — the same
  honesty as this repository's verification vocabulary, applied to a
  configuration contract.
- **Dependency cooldown.** `min-release-age=7` in `.npmrc` ages new npm
  releases seven days before they may enter a lockfile. A Cargo-side
  equivalent policy is worth considering.
- **Context-integrity invariants as named degradation modes.** Never separate
  a tool call from its result during compaction; prefer duplicated history to
  amnesia when folding; heal dangling calls with explicit synthetic error
  results. Candidates for the context-window guide.

## Cautionary findings

Lessons from defects observed in reading, kept here so they are not relearned:

- QM's three postures form a linear rank of *disjoint* mechanisms: Strict
  gates every tool but turns content screening **off**; Auto screens but never
  gates. Tightening a scope from Auto to Strict silently drops a control.
  Policy composition here must never let a "stricter" level shed a weaker
  level's protections.
- The command-approval key is the matched *rule pattern*, not the command, so
  one "always" approval grants the whole rule class for that actor.
- Invalid stored policy regexes are skipped with a log line rather than
  failing closed — a corrupted rule silently loses coverage.
- Config caches are hydrated per instance and refreshed only on the writing
  instance: a policy change on instance A is invisible to instance B until
  restart, in a system whose own doctrine is "durable by default".

## The contribution model

`CONTRIBUTING.md` inverts the contribution artefact: outsiders submit
human-written intent (informal prose in `adrs/`), and implementation is done
by the maintainers' own agents at the maintainers' token expense, inside their
review gates. Three implications:

- The contribution no longer documents the implementation, so the operator's
  evidence trail is the only record of what the implementing agent read,
  fetched and admitted — the session-evidence layer, as applied to the
  authoring process. QM's own audit covers its product's runtime turns, not
  the agents that build it.
- "Please do not have AI artificially expand what you'd like to do" is a
  supply-authenticity constraint with no evidence basis — declared,
  unverifiable, and honestly recordable only as such.
- The "if we're aligned" gate wants the admit-before-spend shape: a bounded,
  budgeted triage verdict recorded before implementation tokens burn, so
  rejection is near-free and an expensive run traces to a decision.
