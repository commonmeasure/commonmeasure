---
title: Fleet-status contract
---

# Fleet-status contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This contract defines what one edge reports about the policy it applies,
so an organisation running many edges can tell whether each applies the
policy it is meant to without any edge exporting its policy document. It
is a management contract, separate from Content Telemetry: the telemetry
wire is purpose-limited to what an agent retrieved and grounded on, and
nothing here extends it. Terms such as edge, hub, principal, allowance and
policy identity are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).

Everything in the document is the edge's report about itself. A digest is
drift evidence: two edges reporting one digest resolved one effective
policy. It is not attestation that an uncompromised edge enforced it, and
no document in this repository describes it as such.

## The document

`commonmeasure status --json` prints it; `commonmeasure status` prints the
readable summary of the same values. Nothing is sent anywhere by either.

```json
{
  "contract": "contextops-fleet-status/v1",
  "generated_at": "2026-09-06T10:00:00.000Z",
  "edge": {"key_id": null, "unknown": "not enrolled: no enrolment record at /home/op/.commonmeasure/enrolment.json"},
  "deployment_mode": "local",
  "desired": null,
  "applied": {
    "revision": null,
    "digest": null,
    "edited_locally": null,
    "expires_at": null,
    "stale_since": null,
    "policy_declared": true,
    "policy_digest": "sha256:…",
    "policy_identity": {"schema": "contextops-policy-identity/v2", "resolver": "1", "digest": "sha256:…"},
    "principal": {"name": "os-user:1000", "basis": "os_user"}
  },
  "versions": {"commonmeasure": "0.2.0", "resolver": "1", "identity_schema": "contextops-policy-identity/v2"},
  "last_enforcement": {"at": "2026-09-06T09:58:12.411Z", "basis": "the latest mediated or refused crossing in this edge's session logs"},
  "allowances": [
    {"principal": "research-agent", "period": "day", "period_key": "2026-09-06",
     "remaining": {"currency": "USD", "micros": 4200000}, "exceeded": false}
  ]
}
```

- `edge` is the edge's pseudonymous identity: the key id minted at
  enrolment ([`docs/GLOSSARY.md`](../GLOSSARY.md) §Key id), read from the enrolment record
  in the operator home. An edge with no enrolment record is not enrolled
  and reports `key_id: null` with that reason in `unknown`. No hostname,
  user name or machine identifier stands in for it.
- `deployment_mode` is `local` or `managed`, chosen on the edge
  (§Deployment mode). A local edge accepts no remote policy and reports
  `desired: null`.
- `desired` is the last synchronisation: `revision` and `digest` where an
  envelope was verified, `learned_at`, `outcome` and `reason`. The
  outcomes are enumerated once, in [`docs/contracts/policy-envelope.md`](policy-envelope.md)
  §Activation and the last-known-good. It is the edge's view; the receiver
  holds the desired revision itself and does not rely on this field to
  know it.
- `applied.revision` is the desired revision whose policy is the one on
  disk, when the edge activated one; `null` on a local edge and on a
  managed edge that has activated nothing.
- `applied.digest` is the digest the applied revision's envelope named it
  by, over the policy as the envelope carried it
  ([`docs/contracts/policy-envelope.md`](policy-envelope.md) §The envelope),
  and `applied.edited_locally` says whether the policy file has since been
  changed away from that revision's policy (the file no longer digests to
  the loader's form of the policy in the kept envelope); `null` where it
  cannot be determined. Both are `null` where `applied.revision` is. A
  receiver compares `applied.digest` with the digest it published, whatever
  form it published the policy in, and a revision keeps the digest it was
  published under; `applied.policy_digest` is the loader's digest of the
  file and agrees with the published digest only where the loader's form
  was published.
- `applied.expires_at` is when that revision's envelope expires, as the
  envelope states; `applied.stale_since` is the same time once it has
  passed, and `null` before. An expired envelope's policy stays in force
  and the edge refreshes it at session start and before relay
  ([`docs/contracts/policy-envelope.md`](policy-envelope.md) §Cadence and
  staleness). Both are `null` where `applied.revision` is.
- `applied.policy_declared` says whether a policy file exists.
  `applied.policy_digest` is the declaration's digest (§Policy digest);
  `null` when there is no file, and `null` with `applied.unavailable`
  naming the loader's error when the file does not load, because a policy
  that does not load governs nothing a mediating server would start under.
- `applied.policy_identity` is the identity of the effective policy the
  reporting process resolved for its own working directory and principal
  (§Policy identity). It is the identity that process's mediated crossings
  would record. `applied.principal` is that principal and its
  authentication basis.
- `versions` names the binary, the resolver and the identity schema, so a
  receiver can tell a digest computed under one resolver from one computed
  under another.
- `last_enforcement` is the latest mediated or refused crossing on this
  edge's record, with its basis, or `at: null` with the reason. An observed
  crossing is not enforcement and does not move it.
- `allowances` is the bounded summary the decisions allow
  (`DECISIONS.md` §Delegated authority and fleet management): for each
  principal with a declared allowance, the period it is in and what
  remains, read from the ledger enforcement reads. A principal whose
  ledger state cannot be read carries `unavailable` in place of the
  amounts. Where the policy itself does not load the field is an object
  with `unavailable`.

What the document never carries: a prompt, an answer, the policy document
or any rule in it, a scope matcher, an engagement name (governing or
reported), a working directory, a provider, a quote, a receipt, or
per-crossing spend. It carries no count of refused crossings either: that
count travels on the telemetry wire as `refused` on every batch of a
session, the session's running total of refused crossings at the time the
batch is projected and never a per-batch difference, so a receiver keeps
the larger value it has seen for the session and a redelivery cannot
double-count ([`docs/contracts/session-evidence.md`](session-evidence.md)
§The refused count on the wire).

## Policy digest

The digest of the declaration as parsed: `sha256:` and the hex sha256 of
the canonical JSON (§Canonical JSON) of the parsed policy file, serialised
as the loader serialises it on save. Reformatting the file does not move
it; a distributed copy of the same declaration digests equal. It is not
the byte-level revision token the policy editor uses to refuse a stale
save; that token follows the bytes, this one follows the declaration. Nor
is it the digest a policy envelope names its revision by, which is over the
policy as the envelope carries it
([`docs/contracts/policy-envelope.md`](policy-envelope.md) §The envelope);
the two agree when the envelope carried the loader's form.

## Policy identity

The identity of one resolved effective policy. A resolution is the loader
applying the policy document to one working directory for one principal:
the matched scope's overlay, the principal binding, the fail-closed state.
The pre-image is the result of that resolution and nothing else:

| Field | What it carries |
|---|---|
| `schema` | `contextops-policy-identity/v2` |
| `resolver` | the resolver version, `1` |
| `mode` | the effective policy mode |
| `constraints` | the effective set-semantics constraints (every kind but `access_rule`), each as its canonical object with hosts normalised the way admission normalises them, sorted by canonical text, duplicates removed |
| `access_rules` | the effective access rules in declaration order, each with its `position`, because the first matching rule decides and their order is the policy |
| `allow_private_hosts` | the effective value |
| `refuse_on_pii` | the effective value |
| `record_internal_prefixes` | sorted, duplicates removed |
| `scope` | the matched scope's `match` string, or `null` |
| `governing_engagement` | the matched scope's engagement, or `null` |
| `allow_telemetry_egress` | the matched scope's clearance |
| `principal` | `{"name", "basis"}` as resolved |
| `fail_closed` | the refusal reason when the resolution fails closed, else `null` |
| `allowances` | the bound principal's declarations (`period`, `amount`, `timezone`), sorted by canonical text |
| `addons` | the active add-on set with its settings; `"unknown"`, because every processor compiled into the binary runs and there is no active set to represent (`ROADMAP.md` §Add-on management and settings) |
| `terms` | the effective terms declarations (host normalised the way admission normalises it, `reference`, `requires_reporting`, `access_context` where declared), sorted by canonical text, duplicates removed, because terms govern over a source's published preference |

Left out on purpose: the policy file's path, the asserted principal label,
the authenticated subject id, the working directory, and every scope the
resolution did not match. Editing a scope a session is not in therefore
leaves that session's identity unchanged; reordering a file's set
constraints is not drift, and reordering its access rules is.

The identity is `sha256:` and the hex sha256 of the canonical JSON of the
pre-image. A reviewer recomputes it with a canonical serialiser and sha256
alone; `crates/commonmeasure-harness/src/policy.rs` holds the test that
does exactly that. The identity appears on every mediated crossing and
session boundary ([`docs/contracts/session-evidence.md`](session-evidence.md) §Policy identity),
in `context_status`, on the console's Policy page, and in this document.

Adding a field to the pre-image or changing how one is computed moves
every digest, so it moves `schema`. Changing which scope wins, how a
principal overlays or when a resolution fails closed moves `resolver`. A
receiver comparing digests compares only digests computed under one
schema and one resolver.

## Canonical JSON

The RFC 8785 canonical form, which [`docs/contracts/canonical-json.md`](canonical-json.md)
defines: members sorted at every depth, no whitespace, and one spelling for
every string and number. The caller sorts an array whose order carries no
meaning before serialising it.

## What a receiver concludes

A receiver holds the desired revision and its policy digest itself. From
those and the document alone it reaches one of four answers:

| Answer | When |
|---|---|
| `current` | `applied.revision` equals the desired revision, `applied.digest` equals the desired digest and `applied.edited_locally` is not `true` (an edge reporting no `applied.digest` is compared on `applied.policy_digest`) |
| `stale` | `applied.revision` is earlier than the desired revision |
| `divergent` | `applied.revision` equals the desired revision but the digest differs or the policy was changed on the edge after activation, or `applied.revision` is later than the desired revision |
| `unknown` | `applied.revision` is `null` (a local edge, or a managed edge that has activated nothing), or `applied.policy_digest` is `null` (no policy, or one that did not load) |

`crates/commonmeasure-harness/src/fleet.rs` implements this classification
and tests each answer. No answer needs the policy document.

## Deployment mode

The mode is chosen on the edge and nothing remote can change it. `local`,
the default and the state of a fresh checkout, accepts no remote policy
and makes no management request. `managed` pins one signing identity and
accepts a signed desired-policy envelope from it and from nothing else;
the envelope, its validation and the last-known-good rule are
[`docs/contracts/policy-envelope.md`](policy-envelope.md).

## Where it travels

The document is built locally. Delivering it to a hub is a management
request on a path of its own, never on the telemetry receiver path
(`relay.json`), so a telemetry ingest key can never carry policy state and
policy distribution can never carry a crossing.

## Conformance

`crates/commonmeasure-cli/tests/fleet_status.rs` drives the real binary
against two edge homes: a policy edit both edges are in moves the
identity, an edit to a scope neither is in does not, and the receiver's
four answers are reached from the documents alone.


## Directory resolver version

Directory enrolment advances the effective-policy resolver to version `2`:
canonical directory identity, conservative linked-worktree checks, and separate
local/signed reporting clearance. Source-policy declaration digests and the
fleet-status schema remain unchanged. Directory status exposes the independent
grant revision and expiry; these are never presented as source-policy revisions.
See [directory enrolment](directory-enrolment.md).
