---
title: Directory enrolment
domain: hub
audience: integrator
section: reference
---

# Directory enrolment

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

One edge enrols with one organisation. A local directory selection is a separate
record and creates no edge key. The CLI `commonmeasure enrol`, the Claude plugin
command `/commonmeasure:enrol`, the Codex skill `$commonmeasure-enrol` and local
MCP `context_enrol` share `crates/commonmeasure-harness/src/directory.rs`.
They add no shell or filesystem permissions.

## Local selection

`directories.json` under the operator home contains project UUIDs, names,
canonical roots, operating-system user ids, filesystem device/inode identities,
Git common directories (or null for non-Git folders), local random nonces,
binding digests and reporting choices. Writes are locked, atomic and mode 0600
on Unix. Before writing the registry, enrolment persists
`directory-selection.json` with schema `commonmeasure-directory-selection/v1`.
Its presence keeps directory selection active if the registry disappears;
the missing registry then behaves as an empty selection and withholds both
new and queued reporting while source admission remains enforced. Local opt-out
retains this marker. Re-enrolling a root under the same identity retains its
project UUID and nonce. A replaced folder or changed Git identity needs a new binding and
approval. Windows currently returns an explicit unavailable result because the
runtime has no authenticated OS user basis there.

A versioned binding is the canonical SHA-256 digest of
`{schema: commonmeasure-directory-binding/v1, nonce, filesystem_id, root,
os_user, git_common}`. The nonce is a randomly generated UUID held locally;
it prevents a Hub reader from testing guessed paths against the digest. Git
identity comes from `.git` and `commondir`, never from a remote URL.

A selected root covers itself and descendants on path-component boundaries.
Similarly named siblings do not match. Canonicalisation follows symlinks;
an alias of a selected root is the same root, while an escaping symlink has
no reporting permission from that root. A different nested Git repository or
worktree needs its own selection. A local-only ancestor vetoes more specific
reporting selections, including nested repositories and submodules. The veto
validates the ancestor's canonical root, OS user, filesystem identity, binding
and its own root's Git identity. Positive reporting inheritance additionally
requires the current directory's Git identity to match the selected root.
Otherwise the most specific selected root applies.
Removed, unresolvable or replaced paths cannot clear historical evidence.

Existing source-policy substring scopes retain their order and meaning.
Absolute scopes that resolve to existing directories also match their canonical
root and descendants, so a symlink alias cannot hide a restriction. Conflicting
spelled and canonical scopes fail closed. Linked Git worktrees are identified
from Git's on-disk metadata; a different source-policy scope from the
corresponding path in the main worktree fails closed and requires an owner to
align the policy. Reporting is never inherited by a guessed worktree name.
Effective policy identities carry resolver version `2` for these resolution
rules. Source-policy document digests and its schema are unchanged.

Homes that have never adopted directory selection retain legacy reporting
clearances. Once adopted, evidence outside selected roots stays local, including
when the registry is missing but the persistent mode marker remains. Local-only
choices never relax admission. On a managed edge, reporting additionally needs an applied,
unchanged managed source policy and a current signed reporting approval for
this project's UUID and binding. Every winning existing scope with
`allow_telemetry_egress: false` vetoes reporting; an omitted boolean is false
and has exactly the same effect. A reporting approval can therefore enable an otherwise
unmatched directory but cannot override a confidential scope or any admission
rule. Provider credentials are unaffected.

## Managed requests and approval

The edge posts `{project_id, binding, name, requested}` to
`/api/v1/edge/directories` using its existing ingest credential. No canonical
path, nonce or OS user id leaves the machine. The server resolves the edge
from the authenticated credential; it accepts no caller-supplied edge identity.
Repeated unchanged requests are idempotent. Changed bindings or names and
withdrawn requests remove approval.

An owner reviews requests on the Hub's `/directories` page and approves or
revokes a project through `/api/v1/directories/{edge}/{project}`. The decision
includes the binding displayed for review, so a changed binding cannot receive
approval for an old review. Renaming clears existing approval but does not
change the binding; an already-open review can still approve that binding.
Approval holds the organisation lock and rechecks the current owner before
changing the approval, so completed removal or closure refuses a queued decision.
The owner queue and signed snapshots carry `Cache-Control: no-store`. Members and ingest keys cannot
read the owner queue or approve. The edge reads only its own signed snapshot
of reporting approvals at `/api/v1/edge/reporting-approvals`.

## Signed reporting approvals

A snapshot is separate from the source-policy envelope:

```json
{
  "payload": {
    "schema": "commonmeasure-reporting-approvals/v1",
    "organisation": "organisation-id",
    "edge_key_id": "enrolled-key-id",
    "revision": 3,
    "approvals": [{"project_id": "project-uuid", "binding": "sha256:…"}]
  },
  "digest": "sha256:…",
  "key_id": "pinned-policy-signer",
  "issued_at": "2026-09-14T12:00:00Z",
  "expires_at": "2026-09-15T12:00:00Z",
  "signature": "hex-ed25519-signature"
}
```

The digest covers canonical JSON of `payload`. The signature covers canonical
JSON of the whole snapshot with `signature` removed, using the existing pinned
policy signer. The edge verifies the format, organisation, enrolled key id,
signer id, signature, payload digest and validity window. The issue time permits
five minutes of clock skew; validity is at most 24 hours. An expired snapshot
cannot authorise egress. A valid snapshot can carry zero approvals: this is
an authenticated revocation, not an error.

Each edge has an independent monotonically increasing approval revision. Request
changes, approval and revocation advance it transactionally under the edge row
lock. Reusing a revision with a different digest and lowering a revision are
refused. A same-revision refresh may renew expiry only with a valid signature
and identical payload. The last accepted snapshot is kept atomically at
`reporting-approvals.json`; a failed fetch, malformed response or HTTP 404 never
becomes an empty snapshot and never extends the saved expiry.

`enrol --sync` refreshes source policy and reporting approvals. Enrolment
submits requests and obtains the current snapshot. Relay refreshes the
approvals before projection; if that fails, only the previously accepted,
unexpired snapshot can authorise reporting. There is no push or background
service. Source-policy expiry keeps its existing last-known-good semantics;
approval expiry removes egress authority while admission still applies the
source policy.

Source-policy envelopes carry no reporting approvals, and a reporting-approval
snapshot is never accepted as source policy.

Every process using an Edge home runs the same release (`ARCHITECTURE.md`
§What runs where).

## Evidence and withdrawal

Hub opt-in explicitly acknowledges the canonical root and descendants and
existing eligible witnessed evidence under them (`--include-history` or MCP
`include_history: true`). Reconstructed, internal and private evidence stays
local under the existing projection rules. Previously delivered events cannot
be recalled by local opt-out.

`enrol --remove` changes local permission before contacting the Hub; a failed
Hub update does not undo opt-out. It preserves evidence and the edge connection.
Relay reprojects every queued session against current source policy, local
selection and reporting approvals, expiry included, immediately before
delivery, holding the local selection lock across the send. Opt-out waits for
an already-running send and blocks subsequent ones. Withheld events are held,
never dropped:

- A queued batch with no event still cleared, after opt-out or a revoked or
  expired approval, stays queued with a hold reason, sends nothing and
  consumes no delivery attempt. Status counts it as held.
- A queued batch with some events still cleared sends those; the withheld
  events stay out of the delivered record.
- Evidence recorded while approved and first relayed after revocation is not
  projected.

After a new opt-in or re-approval, the next relay projects the evidence it
did not project and sends the held events under their original stable ids,
each once. A batch is rechecked only when due: one whose earlier delivery
failed waits for its retry deadline, under revocation and after re-approval
alike ([telemetry projection](telemetry-projection.md) §Delivery state).
Retained spool records and original evidence remain. Batches without session
evidence or directory binding cannot inherit reporting permission.

Each local spool entry records a `directory_selection` boolean outside its
wire document. A true value requires consent revalidation even if both the
registry and persistent mode marker disappear. In that case relay returns an
explicit error and keeps the batch pending across retries until directory
consent is restored; it cannot acknowledge the batch and later reproject its
events under scope clearances. A spool line without the field is damage and
nothing is delivered until it is repaired; the relay never defaults it. A
batch with `queued_at: null` is held and never sent, because a rewrite of the
spool may have filled the field in
([telemetry projection](telemetry-projection.md) §Delivery state). Current
directory selection requires revalidation of every batch.
Deleting consent files is not an opt-out operation; use `enrol --remove`.

Status shows the canonical directory, edge and organisation, applied source
policy/revision, the reporting-approval snapshot's state, revision, digest and
expiry (`approvals`), reporting clearance, receiver, and whether witnessed
evidence exists locally. Reporting clearance is one of `not_enrolled`,
`local_only`, `receiver_missing`, `permitted`, `policy_refused`,
`policy_withheld`, `managed_policy_unapplied`, `approval_expired`,
`approval_unavailable` and `approval_pending`. This is separate from delivery.
`relay --dry-run` reports eligible projection, and `relay` reports accepted
batches/events; a permitted private retrieval still produces zero eligible
wire events. No successful setup message implies a delivered first batch.

## Verification

- `crates/commonmeasure-cli/tests/directory_enrolment.rs`: real CLI/MCP input,
  filesystem writes, repeat selection, path boundaries, worktree restrictions,
  symlinks, nested Git/submodule ancestor vetoes, host-config preservation,
  queued opt-out, missing registry/marker and legacy spool compatibility.
- `crates/commonmeasure-cli/tests/hosted_service/reporting.rs`: signed
  approvals against a loopback hub through the real hook and relay; a batch
  held under a revoked approval and sent once after re-approval, and a batch in
  retry backoff sent once when due.
- Harness `directory::tests`: signature and identity checks, revision reuse,
  rollback, signed empty revocation, expiry, failed refresh preservation,
  and refusal of the pre-release `commonmeasure-directory-grants/v1` format
  and `directory-grants.json` file.
- The Hub server’s `directories.rs` integration test: real handlers/database for owner and
  edge authority, isolation, monotonic snapshots and old-policy compatibility.
  Its explicit `cli_enrol_approval_mcp_relay_and_revocation` test runs a supplied
  new edge binary against the real local Hub and database, exercises a permitted
  local fetch and policy refusal, proves zero private events leave, and delivers
  synthetic public hook evidence idempotently. It does not establish a live
  external supply-provider or interactive host integration.
