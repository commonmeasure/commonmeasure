# Repository rules

This is the product repository for Common Measure.

The repository is private Common Measure Ltd product work. Do not publish or
push to a public remote without the owner's explicit instruction.

## Start here

Read this file, `PRODUCT.md`, and `ROADMAP.md` §Next to establish scope.
For the task itself, read the relevant decision, contract, package and live
brief. Use `docs/GLOSSARY.md` when a term is unclear. Do not reread unrelated
contracts or the entire roadmap for each small edit.

## Coordination and completion

These rules apply to every change, including work split across agents.

- Before editing, inspect the working tree and the relevant live brief. Keep
  other workers' changes intact. Read the product definition once per task;
  then read only the decisions, contracts and roadmap entries affected.
- One lead owns integration and completion. For concurrent work, use one
  shared brief in `work/` listing each worker's scope, checkout or branch,
  dependencies and handoff state. Independent tasks need no team ceremony.
  Assign non-overlapping files where possible; name one editor for shared
  contracts, roadmap entries and other contested files. Use separate
  worktrees, build targets and test databases when workers would collide.
- Workers report what changed, validation actually run, unresolved issues
  and the revision or files to integrate. A worker finishing is not evidence
  that its change is integrated. The lead checks the combined diff and the
  affected end-to-end path before updating product completion.
- The roadmap owns delivery status; the QA list owns defects; a live brief
  owns temporary assignments and integration state. Summaries link to their
  owner and identify their revision or date when they may become stale.
  Do not maintain competing completion checklists or paste full transcripts.
- Record a product decision and its reason in the owning decision document
  when it changes. A proposal, assessment or worker suggestion is not a
  decision. User-authorised changes may update existing decisions; do not
  ask again merely because a document describes the previous choice.
  Resolve routine implementation choices within scope and record material
  ones. Escalate unresolved conflicts before dependent work proceeds.
- Behaviour, contract and user-facing documentation change together. Before
  finishing, inspect references to the changed behaviour, update the owning
  docs and roadmap, and report remaining gaps. For a cross-repository change,
  name the matching revisions or still-open work in the shared brief. Never
  mark a feature complete while required integration or docs are outstanding.
- At completion, remove temporary assignments and duplicated progress notes.
  Preserve useful decision rationale and reproducible evidence in their
  owning documents. Keep a dated handoff or review only if it has continuing
  value; label it as a snapshot and link to current status. Git stores diffs,
  not unwritten reasons. No mandatory report file or heading template.

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

- **Integration claims need real integration evidence.** Do not present a
  fake provider, inference backend, ledger, sidecar or receiver as proof that
  an integration works. Replay claims exercise recorded bytes through the
  production transport, parsing and execution path; use loopback origins.
- Focused test doubles, injected clocks and fault injection are allowed for
  deterministic unit tests and failure handling. Name what they establish.
  They do not establish live or replay integration. Keep test seams small
  and avoid parallel implementations of the product solely for testing.
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
  this repository by name and repository-relative path. Cross-repository
  references are allowed; private source material does not enter public docs.
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

Apply these rules to docs, UI copy, comments, commits and agent handoffs.
Review your own new or edited text before finishing; rewrite violations in
scope without asking for permission or opening another process.

- Use plain British English, concrete verbs and stable product terms. State
  what happens and, when useful, why. Explain unfamiliar terms or link to
  `docs/GLOSSARY.md`. Keep necessary uncertainty; do not replace it with
  confident-sounding prose.
- No mannered prose: aphorisms, rhetorical questions answered by the writer,
  dramatic fragments, strained metaphors, invented slogans, clever inversions
  or sentences shaped for effect. Avoid contrast formulas such as "This isn't
  X. It's Y." and "X, not Y" when a direct statement would do. Use contrasts
  only to explain an actual distinction the reader needs.
- No self-praise or filler: "seamless", "powerful", "robust", "elegant",
  "at its core", "importantly" and "it's worth noting" do not explain a
  feature. Describe the behaviour or evidence instead. Do not rename a thing
  merely to vary the prose, or invent a compound label for an ordinary action.
- Do not narrate the UX. A page needs controls, data and useful consequences,
  not a paragraph announcing what the page, section, card or button does.
  Delete introductions that repeat the heading, captions that repeat the
  value, explanations of obvious controls and conclusions that repeat the
  paragraph. Put walkthroughs in docs or optional help.
- UI explanation earns its place by helping someone choose, act or recover:
  a non-obvious input, consequence, permission, data disclosure, limitation or
  error remedy. Keep it beside the relevant control. Preserve accessible
  labels and necessary instructions; terse but cryptic copy is not an improvement.
  Do not fill empty space with onboarding patter or marketing reassurance.
- Keep implementation and development process out of product copy: source
  file locations, commit rules, internal work packages, test claims and
  source-of-record commentary belong in engineering docs. Mention a technical
  detail in the UI only when the user needs it to complete the task.
- Comments explain constraints and reasons; public APIs document what callers
  need. Handoffs report changes, evidence and remaining work without narrating
  the agent's effort or praising its result.
- Dates, concise decision history, examples and useful summaries are allowed.
  Distinguish current behaviour, proposals and historical observations. Avoid
  machine-specific paths in portable instructions; label them in operational
  evidence when needed.

Examples:

| Remove | Write instead |
|---|---|
| "This isn't just a log. It's trust, made visible." | "The log records each source and the policy decision." |
| "Seamlessly take control of your source ecosystem." | "Choose which sources your agents may use." |
| "This page lets you view and manage your API keys." | Use the heading "API keys" and the relevant controls. |
| "Click Save to save your changes." | Use the button label "Save". |
| "You're all set to begin your journey." | State the result: "Edge connected." |

These are authoring requirements, not a new release gate or a reason for a
repository-wide copy sweep. Fix touched prose as ordinary work. Remaining
style findings are P2 unless they materially misstate behaviour. Do not add
phrase-ban tests or freeze explanatory sentences in assertions.

## Document ownership

Each category has one authoritative owner. Useful summaries elsewhere link to
it and are updated when the underlying fact changes:

- `ROADMAP.md`: scope, delivery status and acceptance. Check completion only
  when evidence exists; confidential captures belong in a named ignored
  directory, never in git.
- `README.md`: the first reader's explanation and current quick start.
- `DECISIONS.md`: adopted decisions, their reasons and conditions for revisiting.
- `PRODUCT.md`: buyer, product definition, current release boundary and thesis.
  Update it when these change, including during an implementation change.
- `ARCHITECTURE.md`, `docs/GLOSSARY.md` and `docs/contracts/`: components,
  terminology and formats respectively.
- `docs/qa/OPEN.md`: unresolved defects and their closure evidence.
- `work/`: live briefs and coordination, following `work/README.md`.

Test commands presented as verified walkthroughs. Proposed or environment-
specific commands may be shown with their prerequisites and verification state
clearly labelled; do not execute destructive or paid operations just to make a
document pass. Existing repository links must resolve; label proposed paths as
proposed rather than presenting them as existing files. The documentation link
checks protect navigation, not prose style or brief templates.

## Verification

Choose checks for the affected behaviour and name what they establish. For a
prose-only change, check links, examples and consistency; no full Rust build is
required. For code, run relevant tests, formatting and Clippy on the affected
crates. Run the full offline workspace checks for release candidates and broad
changes. Use `docs/qa/gate.md` for review scope and blocking criteria. An
unrelated known failure is recorded, not silently repaired or called a pass.
