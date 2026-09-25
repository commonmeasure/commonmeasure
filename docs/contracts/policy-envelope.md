---
title: Policy envelope contract
domain: hub
audience: integrator
section: reference
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
  confidentiality. The same rule is applied when `connect --managed` pins
  the hub's signer: a hub whose `policy_url` fails it is refused before
  anything is written, the enrolment stands and the command exits
  non-zero, so no deployment file is ever written that synchronisation
  would then refuse.
- Unknown fields are load errors. A file that does not load is reported as
  `unavailable` wherever the mode is reported; it is never read as `local`,
  because an operator who wrote `managed` must not quietly stop taking
  policy.
- Nothing an envelope carries can change this file. Synchronisation never
  writes it. `commonmeasure connect <hub> --token <token> --managed` writes
  it once, at enrolment, from the signer the hub publishes (§The hub side),
  says what file it replaced if one was there, and makes a first
  synchronisation; it exits non-zero when the signer could not be read (the
  enrolment stands, the edge stays `local`) or when that synchronisation did
  not activate a policy, except when the hub explicitly reports no revision
  and this edge has never applied one (§Cadence and staleness). `connect`
  without the flag leaves the edge in `local`. `commonmeasure disconnect` removes a managed deployment whose
  `policy_url` is under the hub being left, because the pin could only
  answer `unauthenticated` from then on, and keeps one that names another
  hub.

## The envelope

`GET <policy_url>` returns one envelope. The request is a management
request on a path of its own: it carries no telemetry key and nothing from
`relay.json`, and its `User-Agent` names it (`commonmeasure-managed/<version>`).

The edge authenticates to the hub's policy endpoint by signing the request
with its enrolled key, as HTTP message signatures (RFC 9421, Ed25519), the
mechanism the verified fetcher identity uses towards publishers. There is
no second credential
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
- `digest` is `sha256:` and the hex sha256 of the canonical JSON
  ([`docs/contracts/canonical-json.md`](canonical-json.md)) of `payload.policy`
  exactly as the envelope carries it. The edge checks it before it parses
  the policy, so a hub names a revision by the document it published and
  never has to reproduce the edge loader's serialisation. Once the policy
  is in force the fleet-status document reports `applied.policy_digest`,
  the digest of the policy as the loader serialises it
  ([`docs/contracts/fleet-status.md`](fleet-status.md) §Policy digest). The
  two are equal when the hub carried the loader's form. The fleet-status
  document also reports `applied.digest`, the digest this envelope named
  the revision by, so a receiver compares the digest it published with the
  digest the edge accepted, whatever form the policy was published in; a
  revision keeps the digest it was published under. The shared vectors in
  [`docs/contracts/canonical-json.md`](canonical-json.md) §Shared policy
  vectors pin both digests of one policy.
- `signature.value` is the Ed25519 signature over the canonical JSON of
  `payload` ([`docs/contracts/canonical-json.md`](canonical-json.md)), hex encoded.
  `signature.key_id` names the signing key.

## Validation

Validity is checked against the edge clock read after the complete response
arrives. `issued_at` may be up to five minutes (300 seconds) ahead of that
clock, inclusive. This bounded tolerance accommodates modest clock skew
between the signer and edge; request duration is handled by reading the
clock again, not by the tolerance. `expires_at` has no tolerance: an envelope
that expires at or before the response-time clock is expired.

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
| `not_yet_valid` | `issued_at` is more than the stated tolerance after now; the rejection names the tolerance |
| `expired` | `expires_at` is not after now |
| `digest_mismatch` | `digest` is not the digest of `policy` as the envelope carries it |
| `invalid_policy` | `policy` does not load through the runtime's loader: an unknown field, an empty scope match, an egress clearance without an engagement, any rule the loader enforces |

Two more checks follow, against the edge's own state:

- **rollback**: a `revision` earlier than the applied revision is refused
  however valid its signature, so a captured earlier envelope cannot move
  an edge backwards;
- **reuse**: the applied revision offered again with another digest is
  refused, because a revision names one policy.

The applied revision offered again with the same digest is `already_applied`
when the policy on disk still digests to the loader's form of it, and
`reapplied` when the file was changed locally after activation: the desired
policy is written back. An `already_applied` envelope whose `issued_at` or
`expires_at` differ from the applied ones is the hub renewing the revision:
the envelope kept as last-known-good and the recorded window are replaced,
and the policy file is not touched.

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
| `no_revision` | the hub explicitly reports that the organisation has no published policy revision; nothing is activated or removed |
| `unreachable` | no envelope was obtained: a transport error or an unexpected non-`200` answer, in `reason` |
| `unauthenticated` | this edge's key does not authenticate it: no request was made because the edge holds no key to sign with (not enrolled, or a revocation already learnt), or the request was made and the policy endpoint answered 401, which is what a revoked key or a closed organisation answers; `reason` says which |

The fleet-status document carries the last outcome and its reason as
`desired`.

Every failure, including a hub that cannot be reached, leaves the applied
policy in force and records the failure. An applied policy whose
`expires_at` has passed stays in force: expiry is a condition for accepting
a new envelope, never a reason to relax the one in force; what expiry
changes is the record (§Cadence and staleness). Managed source-policy enforcement
reads local `policy.json` without waiting for Hub.

Planned ([grant contract §Distribution](grant.md#distribution)): the envelope
will carry the organisation's grants as `payload.grants` and
`payload.grants_digest`, and a `revision` will then name one policy and one
grant set. A grant in an expired envelope authorises nothing, while the policy
in it stays in force. Until that is built this envelope carries no grant. The
envelope issues no supplier credential, and a selected access route may
separately require online authority; its absence never widens access.

The fleet-status document ([`docs/contracts/fleet-status.md`](fleet-status.md)) reports the
mode, the last desired revision learned of with its outcome, and the
applied revision, which is how drift is visible before convergence and a
rejected rollback stays visible after it.

## Cadence and staleness

A managed edge refreshes its policy itself, and `commonmeasure policy sync`
runs the same synchronisation on demand:

- **At session start.** Once per session, by whichever path opens it
  first: on a host with a session-start hook (Claude Code) the hook runs
  it after the nudge is out; the MCP server runs it at start, before it
  resolves the policy the session runs under, for any session whose log
  carries no refresh yet, whatever the host calls itself. The request may
  wait three seconds for the hub, name resolution included
  (`SESSION_START_BUDGET` in `crates/commonmeasure-harness/src/managed.rs`):
  a host is waiting, and a host that abandons a server which has not
  answered its first request in ten seconds would lose the mediated tools
  for the session. A hub, or a resolver, that has not answered by then is
  recorded as `unreachable` and the policy in force keeps governing. The
  outcome is written to the session log as a `policy_sync` record
  ([`docs/contracts/session-evidence.md`](session-evidence.md) §Policy
  synchronisation) and nothing is said to the agent.
- **Before relay.** `commonmeasure relay` runs it before it reads the
  clearances, so egress is decided under the policy the hub desires now.
  It prints one line naming the outcome, except when the desired revision
  was already in force and its envelope has not expired, when it prints
  nothing.
- **On the service's interval.** A hosted edge run as `commonmeasure
  hosted service` ([`docs/contracts/host-integration.md`](host-integration.md)
  §1) runs the synchronisation and then the relay itself, every
  `interval_seconds` of its configuration (default 300), with the ordinary
  budget and no session open. Each session it opens still refreshes at its
  start under the three-second budget. The outcome is written to the
  service's log on stderr; a session record carries only its own
  session-start refresh. No new outcome is introduced.

A local edge makes no request on any occasion and records nothing about
one. A deployment file that does not load is recorded as `unavailable`
with the loader's reason, never read as `local`.

An applied envelope whose `expires_at` has passed is **stale**: the policy
it carried stays in force, unchanged, and the record says since when. The
fleet-status document carries `applied.expires_at` and `applied.stale_since`
([`docs/contracts/fleet-status.md`](fleet-status.md)); `commonmeasure doctor`
and `commonmeasure status` print `stale since <expires_at>` on the applied
revision's line; the `policy_sync` session record and the output of
`policy sync` carry the same field. Staleness is a fact about the time it is
read at, not a stored flag. The next synchronisation that accepts a later
revision, or that finds the applied revision renewed with a later expiry,
clears it.

`connect --managed` succeeds when `no_revision` is reported and no managed
revision has previously been applied. It says that enrolment is complete and
policy publication is pending; the policy already on disk stays in force.
This is not convergence: `policy sync` continues to exit non-zero until a
revision is accepted. Absence after an applied revision also remains a
non-zero connect outcome and preserves that revision, its envelope and its
staleness.

## What a distributed policy carries

The policy in an envelope is a source policy
([`docs/contracts/source-policy.md`](source-policy.md)) and every edge that
accepts the envelope enforces the same document. A hub that publishes a
policy the loader refuses makes every managed edge reject the revision as
`invalid_policy` and keep the policy in force, so a hub checks a policy
before publishing it against the loader's checks and the contract's
vectors, not against the structure alone.

Two fields mean something different on each edge
([`docs/contracts/source-policy.md`](source-policy.md) §Policy written for
many machines). A principal binding is keyed by an operating-system user id,
which each machine assigns, so a distributed binding applies to whoever
holds that id on each edge. A distributed policy therefore declares no
`principals` until an identity that names the same person on every edge
exists. The hub refuses one at publishing; the edge does not refuse one,
because it cannot tell a policy written for many machines from one written
for itself. A scope's `match` is compared with each edge's own
directories, so a distributed scope governs alike only where the
organisation lays out its directories alike.

## The hub side

The hub publishes its policy-signing key by key id and public key for
pinning at `/api/v1/policy/signer`, as `{key_id, algorithm, public_key,
organisation, policy_path, policy_url}`, to an owner's session and to an
enrolled edge presenting its ingest key, which is how `connect --managed`
reads it; a hub that also returns the same object as `policy_signer` in its
enrolment exchange answer saves the edge that request. `policy_url` is the
absolute address of the desired policy built from the hub's public origin,
and the edge pins it when present, falling back to the address the
operator typed plus `policy_path` only when the hub gave no URL: the
endpoint verifies signatures over the public origin alone, so an edge that
enrolled through an internal address must not pin that address. It serves the organisation's
current envelope at `/api/v1/policy/desired`, and only increases revisions. The endpoint
verifies the request signature against the keys the organisation has
enrolled and resolves the organisation from the key id, so the credential
and the identity are the same thing; it binds the envelope it serves to
that key id. Before any revision is published it answers HTTP 404 with
`{"detail":"no policy revision has been published for this organisation"}`.
The edge recognises that exact detail as `no_revision`; an arbitrary 404,
malformed body or another status is not evidence of an unpublished policy.

The signature is valid for five minutes from its `created` time
(`SIGNATURE_LIFETIME_SECS` in `crates/commonmeasure-harness/src/identity.rs`),
and the hub accepts one further minute either side of that window for clock
skew (`CLOCK_SKEW_SECS`, in the hub's Web Bot Auth verifier). The two constants belong to one bound and move together: a lifetime shorter than the skew allowance would
accept a signature that had never been valid. The edge sends a nonce and the
hub does not store it, so a signature is not replay-bound inside its window:
anyone holding a copy can repeat the request it was made for until the
window closes, which buys the organisation's own desired policy and nothing
else. Storing nonces is what closes that if the endpoint ever serves
something narrower.

The signed request must reach the hub carrying the authority the edge
signed. A proxy in front of the hub that rewrites or drops it refuses every
enrolled edge.

Each half is tested against the other's rule, and one pinned signature
vector holds both to the same signature base.

## Conformance

`crates/commonmeasure-harness/src/managed.rs` tests each rejection kind and
the rollback, reuse, reapply and unreachable paths against a loopback
origin; that the digest is checked over the policy as carried, in the
owner's form and in the loader's, with the loader's digest kept beside it;
and that an expired envelope keeps enforcing, is reported stale since its
own expiry, and is cleared by the hub renewing the revision.
`crates/commonmeasure-cli/tests/managed_policy.rs` drives the refresh
through the real binary: the session-start hook and the MCP server each
record it, a session is refreshed once, an expired envelope is enforced and
reported stale by `status`, `doctor`, the relay and the record until the hub
renews it, and a hub that accepts a connection and never answers costs a
session start its budget and nothing else. `crates/commonmeasure-cli/tests/managed_policy.rs` drives the real
binary against a loopback origin: a local edge makes no request; a managed
edge activates the signed policy and a mediated crossing meets it; a local
edit shows as divergent and is reapplied; rollback, forgery, an edge-bound
envelope, an expired envelope and an invalid policy are refused by name
with the applied policy kept; with the origin gone the last-known-good
policy refuses a crossing offline and the status document says the hub was
unreachable.


## Reporting approvals

Directory reporting uses an independent signed snapshot of reporting
approvals, with its own revision space and 24-hour expiry under the existing
signer pin. It never changes a source-policy payload or the meaning of an
existing policy revision. An expired snapshot removes reporting permission while
source policy retains its existing enforcement semantics. See the
[directory enrolment contract](directory-enrolment.md).
