# Repository rules

This is the product repository for Common Measure.

The repository is private Common Measure Ltd product work. Do not publish or
push to a public remote without the owner's explicit instruction.

## Start here

Read, in this order, before changing anything: this file; `PRODUCT.md`;
`DECISIONS.md`; `ROADMAP.md` §Next and the package being picked up; any
brief in `work/` that serves that package; `docs/GLOSSARY.md` for each term
the package uses. The QA gate has its own reading order (`docs/qa/gate.md`).

## Product model

Common Measure rules on which content an agent may take in, records where
each piece came from and what it cost, and measures whether it helped. The
definition, the buyer, the core and add-ons and the boundary are `PRODUCT.md`.

## Naming

- In prose the product, the company and the agent are **Common Measure**;
  the category is **ContextOps**; the evidence a session or run leaves is the
  **source record**; the operator's policy file is the **source policy**; the
  hosted tier is **Common Measure Hub**.
- Identifiers are listed once in `PRODUCT.md` §Naming. Format identifiers
  inside sealed evidence keep the `contextops-` namespace; do not rename them.
- Harness, host and the other defined terms are `docs/GLOSSARY.md`.

## Implementation rules

- **No mocks or fake implementations.** Do not use a fake provider, inference
  backend, ledger, sidecar or receiver as evidence that an integration works.
  Recorded external bytes may replace an uncontrolled network only when they
  pass through the same real parser, transport boundary, policy, storage and
  inference path used by live execution. Use a loopback origin to serve recorded
  bytes. Remove interfaces created solely for test substitutes.
- Pure unit tests are appropriate for deterministic value types and functions;
  they do not satisfy an integration acceptance criterion.
- Unknown measurements remain unknown: never turn missing cost, latency, token,
  route or evaluation evidence into zero or a requested value
  (`docs/FAIL-POLICY.md` §7).
- Prefer the smallest real vertical path over speculative traits, parallel type
  systems or copied subsystems. Every retained abstraction needs a current
  consumer or a concrete external protocol boundary. Retain tested code for a
  scheduled work package when early integration is cheaper than rewriting it.
  Consolidate duplicated domain models while preserving proven infrastructure.
- A human must be able to trace a run from named input bytes through selection,
  policy, inference, evaluation and evidence without trusting the UI or reading
  an agent transcript.
- Use idiomatic, self-documenting Rust. Document public APIs and non-obvious
  invariants; comments explain why rather than narrating mechanics. Formatting,
  Clippy with warnings denied and relevant tests are required gates.

- Keep consulting deliverables, client-confidential material, CRM, mail and
  standards governance out of this repository. Strategy, market, regulation,
  investor and buyer material, and deployment targets, unit files, runbooks
  and incident records, live in separate private repositories that cite
  this repository by name, never by path. Nothing from them, and no name of
  them, enters this repository.
- Distil private conversations; do not copy full transcripts.
- Preserve provenance for imported claims and design decisions.
- Mark integrations `planned`, `fixture-tested`, `replay-tested`,
  `spec-verified` or `live-verified`; never collapse those states.
- Never commit credentials, tokens, raw customer prompts or licensed content.
- Keep the private operator record separate from Content Telemetry egress.
- Keep every external telemetry receiver standalone and optional.
- Supply adapters implement capabilities; do not pretend content and skill
  suppliers have equivalent search, fetch, invocation, provenance, terms or
  payment semantics.
- Specialist optimisation, verification and provenance components stay
  replaceable behind `docs/contracts/processor.md`; preserve their raw evidence
  and never collapse their scores into a universal truth value.
- "Processor" and "harness plugin" are different things. A processor is a stage
  around a ContextJob (`docs/contracts/processor.md`); a harness plugin
  integrates the product with a host (`plugin/`). Do not reuse the bare word
  "plugin" for the first.
- Deterministic policy precedes learned optimisation.
- Use British English.

- A missing dependency produces an explicit unavailable result with a gap that
  names it, never a weaker successful mode (`docs/FAIL-POLICY.md` §5).

## Writing

Plain English is the norm. Write for a capable outsider reading the document
for the first time:

- Say what a thing does in ordinary words before, or instead of, giving it a
  name. If a sentence would need translating when read aloud in a meeting,
  rewrite it.
- No invented idiom, metaphor or theatrical phrasing in documents, code
  comments or commit messages. State the fact and its reason.
- Do not use mannered prose: no aphorisms, no rhetorical inversions, no
  elegant-variation synonyms for a thing already named, no sentence shaped
  for effect. State the fact and its reason in the plainest ordering.
- The product's defined terms (crossing, engagement, admission, manifest,
  evidence log and the rest) are precise vocabulary. Use
  them where precision requires them, keep each one's plain-English
  definition in `docs/GLOSSARY.md`, and have any audience-facing document
  define or link a term the first time it uses it.
- Documents state current fact in the present tense. They do not narrate
  how a thing came to be, reference a superseded position, or carry dates,
  work-package ids, person names or machine-specific paths in reader-facing
  text. Git holds the history.
- Code comments state the constraint or the reason. They do not cite
  work-package or finding ids, because a closed id resolves to nothing, and
  they do not name where code came from.

## Document ownership

Each category of information has one authoritative document. Another
document may restate it in one sentence with a link, never more:

- **`ROADMAP.md`** owns what is built, what is not, and the open packages
  with their acceptance criteria. A landing updates its boxes; it records
  status nowhere else. Check a box only when its acceptance evidence exists
  in the repository, or in a named gitignored directory where the evidence
  must not enter git.
- **`README.md`** describes the product for a first reader and links to the
  board; it asserts no checklist of its own.
- **`DECISIONS.md`** records the current rules that constrain the product.
  Edit it when a decision changes; git retains the history.
- **`PRODUCT.md`** holds the definition, the buyer, the boundary and the
  question the first version answers; it changes only when the thesis
  changes, never as part of a landing.
- **`ARCHITECTURE.md`** owns the components, the data flow and what runs
  where. **`docs/GLOSSARY.md`** owns the terms. **`docs/contracts/`** own the
  formats.
- **`docs/qa/OPEN.md`** holds the open defects, each row self-contained, and
  its header says what a gate is; `docs/qa/gate.md` is the gate procedure.
  No report files are retained.
- **`work/`** holds live briefs: bounded tasks an agent can land in one
  change, each in the shape `work/README.md` states and each naming the
  roadmap package it serves. A brief carries no status; the package's boxes
  do. A landed brief is deleted.
  `crates/commonmeasure-cli/tests/work_briefs.rs` enforces the shape.
- **A document that quotes a command has run that command.** The walkthrough's
  discipline (`docs/GETTING-STARTED.md`) applies to any doc with a fenced
  shell block: if it cannot be run as written, it does not go in.
- Repo-relative paths named in the documents must exist;
  `crates/commonmeasure-cli/tests/docs_paths.rs` enforces this in the offline gate.
