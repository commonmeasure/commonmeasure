---
title: Policy envelope contract
---

# Policy envelope contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This contract defines how a desired policy reaches an edge from a hub, and
what the edge does with it. The edge decides: it takes remote policy only
under a deployment mode it chose in a local file, from a signer pinned in
that file, and it keeps the policy already in force whenever any check
fails. Terms such as edge, hub, policy identity and policy mode are defined
in [`docs/GLOSSARY.md`](../GLOSSARY.md); the policy file itself is the one the runtime loads
from `policy.json`, and there is no second policy engine.

## Deployment mode

`$COMMONMEASURE_HOME/deployment.json`, or `~/.commonmeasure/deployment.json`.
Absent means `local`, which is the state of a fresh checkout.

```json
{"mode": "local"}
```

```json
{
  "mode": "managed",
  "signer": {"key_id": "hub-policy-1", "algorithm": "ed25519", "public_key": "<hex, 32 bytes>"},
  "policy_url": "https://<your hub>/api/v1/policy/desired",
  "organisation": "org-…"
}
```

- `local` accepts no remote policy. `commonmeasure policy sync` refuses,
  names this file, and makes no request.
- `managed` pins one signer: the hub's policy-signing key, by key id and
  raw Ed25519 public key. An envelope is accepted from that key and from no
  other, for `organisation` and for no other.
- `policy_url` is an `https` URL. Plain `http` is accepted only to a
  loopback origin, which stays on the machine: the policy document crosses
  in the envelope, and the signature protects its integrity, not its
  confidentiality.
- Unknown fields are load errors. A file that does not load is reported as
  `unavailable` wherever the mode is reported; it is never read as `local`,
  because an operator who wrote `managed` must not quietly stop taking
  policy.
- Nothing an envelope carries can change this file. Synchronisation never
  writes it.

## The envelope

`GET <policy_url>` returns one envelope. The request is a management
request on a path of its own: it carries no telemetry key and nothing from
`relay.json`, and its `User-Agent` names it (`commonmeasure-managed/<version>`).

The edge authenticates to the hub's policy endpoint by signing the request
with its enrolled key, as HTTP message signatures (RFC 9421, Ed25519), the
mechanism the verified fetcher identity uses towards publishers
(`ROADMAP.md` §Verified fetcher identity). There is no second credential
and `deployment.json`
has no credential field. The signature covers the endpoint's authority and
the `Signature-Agent` header naming the key directory the hub publishes the
key in; the key id is the RFC 7638 thumbprint enrolment assigned.

An edge that is not enrolled, or whose enrolled key the hub has revoked,
has nothing to sign with. It makes no request at all — the endpoint would
refuse an unsigned one, and an edge reporting the hub as unreachable for
its own missing key would send the operator to look in the wrong place —
and records the outcome `unauthenticated` with the reason. The policy
already in force keeps governing, as through every other failure.

```json
{
  "envelope": "contextops-policy-envelope/v1",
  "payload": {
    "organisation": "org-…",
    "edge_key_id": null,
    "revision": 7,
    "issued_at": "2026-09-06T08:00:00Z",
    "expires_at": "2026-09-13T08:00:00Z",
    "policy": { "policy_mode": "strict", "constraints": [], "scopes": [], "allow_private_hosts": false, "record_internal_prefixes": [] },
    "digest": "sha256:…"
  },
  "signature": {"algorithm": "ed25519", "key_id": "hub-policy-1", "value": "<hex, 64 bytes>"}
}
```

- `organisation` binds the envelope to one organisation.
- `edge_key_id` binds it to one edge by the key id minted at enrolment, or
  is `null` for every edge in the organisation. An edge with no key id
  accepts only the `null` form.
- `revision` is an unsigned integer, ordered: a later policy has a higher
  revision, and one revision names one policy.
- `issued_at` and `expires_at` are RFC 3339 times; `expires_at` is after
  `issued_at`.
- `policy` is the policy file as the runtime loads it, in full.
- `digest` is the policy digest ([`docs/contracts/fleet-status.md`](fleet-status.md) §Policy
  digest): `sha256:` and the hex sha256 of the canonical JSON
  ([`docs/contracts/canonical-json.md`](canonical-json.md)) of the policy as the loader
  serialises it after parsing. It is what the fleet-status
  document reports as `applied.policy_digest` once the policy is in force,
  so a receiver can compare the two without either policy.
- `signature.value` is the Ed25519 signature over the canonical JSON of
  `payload` ([`docs/contracts/canonical-json.md`](canonical-json.md)), hex encoded.
  `signature.key_id` names the signing key.

## Validation

An answer that is not `200` with an envelope, including the hub's `404`
before any revision is published, is recorded as the hub `unreachable`,
with the status named; nothing is verified from it. An envelope is checked
by `commonmeasure policy sync` in this order, stopping at the first check
that fails, and each failure is recorded as `rejected` with its kind:

| Kind | Refused when |
|---|---|
| `invalid_schema` | not JSON, not this envelope format, no payload object, a malformed field |
| `wrong_signer` | the signature algorithm is not Ed25519, or `signature.key_id` is not the pinned key id |
| `bad_signature` | the signature does not verify under the pinned public key over the canonical payload, which is also what a payload edited after signing produces |
| `wrong_organisation` | `organisation` is not this edge's |
| `wrong_edge` | `edge_key_id` names another edge, or names any edge and this edge has no key id |
| `not_yet_valid` | `issued_at` is after now |
| `expired` | `expires_at` is not after now |
| `invalid_policy` | `policy` does not load through the runtime's loader: an unknown field, an empty scope match, an egress clearance without an engagement, any rule the loader enforces |
| `digest_mismatch` | `digest` is not the digest of `policy` |

Two more checks follow, against the edge's own state:

- **rollback**: a `revision` earlier than the applied revision is refused
  however valid its signature, so a captured earlier envelope cannot move
  an edge backwards;
- **reuse**: the applied revision offered again with another digest is
  refused, because a revision names one policy.

The applied revision offered again with the same digest is `already_applied`
when the policy on disk still digests to it, and `reapplied` when the file
was changed locally after activation: the desired policy is written back.

## Activation and the last-known-good

An envelope that passes is activated under the policy file's lock, the
one the console's editor holds for its own save, so a read, a comparison
and a replacement are one step against any other writer. The envelope is
kept verbatim at `<home>/managed/last-known-good.json` and
`<home>/managed/state.json` is written first; then `policy.json` is
replaced by the loader's atomic save. A failure before the replacement
leaves the policy in force unchanged. A failure of the replacement itself
leaves the state naming a revision whose policy is not on disk, which the
status document reports as divergent and the next synchronisation
reapplies.

`state.json` records what is applied (revision, digest, issue and expiry
times, activation time, signer key id) and what the last synchronisation
did, as one of these outcomes:

| Outcome | Meaning |
|---|---|
| `accepted` | the desired revision was activated |
| `reapplied` | the applied revision was written back over a local edit |
| `already_applied` | the applied revision was offered again and is on disk unchanged |
| `rejected` | the envelope failed a check named above, or rollback or reuse; `reason` says which |
| `unreachable` | no envelope was obtained: a transport error or a non-`200` answer, in `reason` |
| `unauthenticated` | no request was made: this edge holds no key to sign with, because it is not enrolled or its key was revoked; `reason` says which |

The fleet-status document carries the last outcome and its reason as
`desired`.

Every failure, including a hub that cannot be reached, leaves the applied
policy in force and records the failure. An applied policy whose
`expires_at` has passed stays in force: expiry is a condition for accepting
a new envelope, never a reason to relax the one in force. The hub is never
in the path of a crossing; a mediated crossing under a managed edge reads
`policy.json` exactly as one under a local edge does.

The fleet-status document ([`docs/contracts/fleet-status.md`](fleet-status.md)) reports the
mode, the last desired revision learned of with its outcome, and the
applied revision, which is how drift is visible before convergence and a
rejected rollback stays visible after it.

## The hub side

The hub publishes its policy-signing key by key id and public key for
pinning, serves the organisation's current envelope at
`/api/v1/policy/desired`, and only increases revisions. The endpoint
verifies the request signature against the keys the organisation has
enrolled and resolves the organisation from the key id, so the credential
and the identity are the same thing; it binds the envelope it serves to
that key id.

The signature is valid for five minutes from its `created` time
(`SIGNATURE_LIFETIME_SECS` in `crates/commonmeasure-harness/src/identity.rs`),
and the hub accepts one further minute either side of that window for clock
skew (`CLOCK_SKEW_SECS`, in the Common Measure Hub repository's Web Bot Auth
verifier). The two constants belong to one bound and move together: a lifetime shorter than the skew allowance would
accept a signature that had never been valid. The edge sends a nonce and the
hub does not store it, so a signature is not replay-bound inside its window:
anyone holding a copy can repeat the request it was made for until the
window closes, which buys the organisation's own desired policy and nothing
else. Storing nonces is what closes that if the endpoint ever serves
something narrower.

The signed request must reach the hub carrying the authority the edge
signed. A proxy in front of the hub that rewrites or drops it refuses every
enrolled edge.

The two halves have not been run against each other: each is tested against
the other's rule, and one pinned signature vector holds both to the same
signature base.

## Conformance

`crates/commonmeasure-harness/src/managed.rs` tests each rejection kind and
the rollback, reuse, reapply and unreachable paths against a loopback
origin. `crates/commonmeasure-cli/tests/managed_policy.rs` drives the real
binary against a loopback origin: a local edge makes no request; a managed
edge activates the signed policy and a mediated crossing meets it; a local
edit shows as divergent and is reapplied; rollback, forgery, an edge-bound
envelope, an expired envelope and an invalid policy are refused by name
with the applied policy kept; with the origin gone the last-known-good
policy refuses a crossing offline and the status document says the hub was
unreachable.
