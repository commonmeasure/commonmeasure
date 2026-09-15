---
title: The QA gate
draft: true
---

# The QA gate

## Scope and trigger

Review the diff and affected behaviour for every change. A full independent
review is required for a release candidate or a material change to
authentication, tenancy, policy enforcement, evidence integrity or destructive
data handling. A package number, new view or documentation edit alone does
not trigger a whole-repository review. Include adjacent code where a shared
invariant may be affected; expand further when findings justify it.

The lead identifies the scope, revision and principal user path. When an
independent review is required, use another reviewer or a fresh-context agent;
the author does not certify their own independence. Existing user authority
covers work within scope; do not add an approval round merely to accept
routine findings or fixes.

## Review and remediation

A requested read-only review stays read-only. Otherwise findings can be fixed
within the authorised task, with a separate verification of the resulting
diff. Investigate evidence before adopting a handoff's conclusions; use the
handoff to locate changes and assumptions without treating it as proof.

## Read and inspect

Read `AGENTS.md`, the affected `ROADMAP.md` entries, and the relevant
parts of `PRODUCT.md`, `DECISIONS.md`, `ARCHITECTURE.md`, `docs/FAIL-POLICY.md`
and contracts. Inspect the changed code and the dependencies its behaviour
reaches. Do not reread every document or inspect every crate for a local edit.

## Review questions

- Does the principal user path work? Are its claims supported by evidence?
- Does enforced policy stay enforced? Do authentication, tenant isolation,
  data egress and deletion preserve the stated boundaries?
- Are unknown measurements distinct from zero, and requested routes distinct
  from executed routes? Are gaps and degraded modes visible?
- Are live and replay integration claims exercised through the real path?
  Focused test doubles are valid for unit tests and controlled failures, but
  do not prove external integration.
- Does the change introduce a concrete durability, resource, concurrency or
  compatibility risk? Is a simpler design sufficient for current use?
- Do affected docs, UI, contracts and roadmap agree with the implementation?
  Is a proposal being presented as shipped behaviour?
- Does new or edited text follow `AGENTS.md` writing rules? Flag mannered
  prose, self-praise, filler and UX narration. Fix within scope; style alone
  is P2 and does not justify a wider copy sweep or release block.
- Do tests catch a meaningful regression? Avoid assertions that merely freeze
  explanatory prose or copy the implementation. Use stable codes, values,
  side effects and accessible controls where appropriate.

## Severity and blocking

- **P0:** demonstrated secret/customer-data exposure, cross-tenant access,
  destructive data loss, fabricated evidence or an enforced boundary bypass.
- **P1:** a broken principal path or material correctness, durability or
  security risk in the affected scope, supported by a concrete failure
  scenario. A missing test is P1 only when it leaves such a risk unresolved.
- **P2:** maintainability, duplication, style, minor docs or additional test
  coverage without a material failure in the current scope. Record it for
  later; it does not block delivery.

**PASS** means the reviewed scope has no unresolved P0/P1 and its required
acceptance evidence is available. **BLOCK** names the concrete failure or
missing critical evidence preventing the scoped claim. Scope the verdict:
a deployment acceptance gap can block deployment without blocking unrelated
local development. Existing findings elsewhere remain on the QA list and
are not silently dismissed. Release review covers the paths actually offered.

Clearly deferred or unavailable features are not defects. Simplifying a
claim to match real behaviour is valid when it preserves the agreed release
scope; do not lower the scope silently to obtain PASS.

## Validation

Choose checks according to the affected behaviour:

- For prose-only changes, check links, examples and consistency. No complete
  application build or database suite is needed.
- For code changes, run relevant tests and formatting/Clippy checks on the
  affected crates; exercise real transport and storage for integration claims.
- For release candidates or broad changes, run workspace formatting, Clippy
  with warnings denied and offline tests, plus browser tests when affected.
- Run the documented principal path when its behaviour or contract changes.
  Revalidate affected live claims before continuing to label them live-verified.

An unavailable environment is reported as unverified evidence, not a code
failure or a pass. Use authorised live calls within the task's scope and
budget. Ask only for missing authority for external effects or spending,
destructive operations, or an unresolved material product decision; a read-only
public documentation fetch does not need a new approval solely for being live.

## Findings and handoff

`OPEN.md` owns unresolved defects. Record a finding directly during an
ordinary engineering task; no separate permission is needed. In a read-only
review, return findings to the caller without editing files. Each finding
names severity, the affected claim, exact evidence, observable consequence,
proposed correction and how to verify closure. Reuse an existing finding
rather than opening a duplicate. Preserve identifiers when closing or
retiring findings, with the fixing revision or reason.

Summarise the reviewed revision and scope, verdict, checks actually run,
remaining gaps and next action. No mandatory report template. Keep a dated
review snapshot only when it preserves useful reasoning or evidence; link it
to current status. The lead updates the owning docs and roadmap after the
integrated change is verified, following `AGENTS.md`.
