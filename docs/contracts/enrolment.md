---
title: Enrolment contract
domain: hub
audience: integrator
section: reference
---

# Enrolment contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

- **Owner:** Hub.
- **Producer and consumer:** the edge and the hub, each both.
  `commonmeasure connect` and `commonmeasure disconnect` on the edge
  (`crates/commonmeasure-relay/src/enrolment.rs`); the hub's enrolment routes.
- **State:** built. `fixture-tested` on the edge against a loopback hub that
  verifies the key proof and the directory proof by an independent rule
  (`crates/commonmeasure-cli/tests/connect_e2e.rs`), and in the hub's own
  suite against a real database. An ignored test in the same file drives the
  binary against a running hub; no recorded run of it is kept here.

Enrolment joins one edge home to one organisation on one hub. The edge proves
it holds a new Ed25519 key, the hub lists that key as `CommonMeasureBot`
([bot identity](bot-identity.md)) and issues an ingest key for telemetry
([telemetry projection](telemetry-projection.md)). Directory reporting consent
is a separate enrolment of a working directory
([directory enrolment](directory-enrolment.md)); signed policy is
[policy envelope](policy-envelope.md).

## The enrolment token

A signed-in member mints a token at the hub for themselves; an organisation
owner may mint one for any member. It is `et_` followed by 64 hexadecimal
digits, is shown once, lasts 15 minutes and exchanges once. The hub stores
only its SHA-256. An owner can list and cancel tokens. A token buys one edge
key and one ingest key with the scope `telemetry:ingest`. No token is minted
under an impersonated session or for a closed organisation.

## The exchange

```sh
commonmeasure connect https://hub.example --token et_…
commonmeasure connect https://hub.example --token et_… --managed
```

`connect` refuses while the home holds an enrolment that is not revoked,
naming the hub and key and asking for `disconnect` first; there is no
override. Over a revoked enrolment it starts again with a new key. It then
generates an Ed25519 key and sends:

```
POST <hub>/api/v1/enrolment/exchange
{"token": "et_…",
 "public_key": {"kty": "OKP", "crv": "Ed25519", "x": "<base64url>"},
 "proof": "<base64url>"}
```

`proof` is the key's Ed25519 signature over the token's bytes, trimmed. The
single-use token is the challenge; the hub issues no nonce. The request
carries no other credential and no request signature. The hub checks the
proof before it spends the token, and the whole exchange is one transaction,
so a refusal spends nothing.

The hub answers `201`:

| Member | Meaning |
|---|---|
| `organization` | `id` and `name` |
| `name` | the edge's name, from the token |
| `key_id` | the key's RFC 7638 thumbprint ([bot identity §The key](bot-identity.md#the-key)) |
| `identity` | `origin` (the identity origin a verifier resolves the key at), `bot_page`, and `contact` where configured |
| `api_key`, `api_key_id` | the ingest key, `ak_` and 64 hexadecimal digits, and its identifier |
| `telemetry_path` | `/api/v1/telemetry` |
| `policy_signer` | the key that signs the organisation's policy envelopes |
| `token_issuer` | the issuer of the hub's OAuth access tokens |
| `directory_proof` | `authority`, `lifetime_secs`, `expires_at` (null before the first proof), and optional `listed` and `unlisted_reason` (below) |

The edge accepts only `201`, and only where `key_id` is the thumbprint of the
key it generated; otherwise it writes nothing and exits non-zero. It reads
`directory_proof` where present and does not use `api_key_id` or
`token_issuer`.

| Status | Refusal |
|---|---|
| `400` | the key is not an Ed25519 OKP key, `x` is not an unpadded base64url Ed25519 point, `proof` is not an unpadded base64url signature, or the proof does not verify |
| `401` | the token is unknown, used, cancelled or expired, or its member or organisation is gone |
| `409` | the public key is already enrolled |
| `503` | the hub has no identity origin configured |

The refusals above carry `{"detail": "…"}`. The request times out after 30 seconds.

The hub URL may be `http` or `https`; `connect` does not require `https`, so
an operator must give an `https` URL for any hub not on the same machine.

## What the edge writes

After a `201`, in this order, under the home (`$COMMONMEASURE_HOME` or
`~/.commonmeasure`):

1. `edge-key.json`: the private key ([bot identity §The key](bot-identity.md#the-key)),
   mode `0600` on Unix, synced and renamed into place. It is written before
   the record that names it.
2. `enrolment.json`: `hub` (the URL given, without a trailing `/`),
   `organization`, `name`, `key_id`, `identity` and `enrolled_at` (the edge's
   clock); after a revocation is learnt, also `revoked_at`, `revocation`
   (`owner` or `edge`) and `revocation_learnt_at`. Unknown members are
   refused, and the shape stays the one every released binary reads.
3. `relay.json`: `receiver`, the hub URL given plus `telemetry_path`, and
   `api_key`, mode `0600`. Where a `relay.json` existed, `connect` prints the
   receiver it named.
4. With `--managed`, `deployment.json`: `mode` `managed`, the `signer` from
   `policy_signer`, the `policy_url` and the organisation. It is read back
   through the loader and removed if the loader refuses it
   ([policy envelope §Deployment mode](policy-envelope.md#deployment-mode)).
   `policy_url` must be `https`, or `http` to `127.0.0.1`, `localhost` or
   `[::1]`.
5. `directory-listing.json`, after the first attempt at a directory proof,
   whatever its outcome (below).

Files are written whole and renamed into place. `connect` then runs one relay
run and, with `--managed`, a first policy sync. A failed directory proof or
relay run leaves the enrolment standing and exits `0`. With `--managed`, a
signer that cannot be pinned or a first sync that does not converge exits
non-zero with the enrolment standing; an organisation that has not yet
published a first revision exits `0`.

## The directory proof and its renewal

The hub lists a key in its directory only while it holds a current directory
proof from the key ([bot identity §The directory proof](bot-identity.md#the-directory-proof)).
The edge signs one for the authority of the identity origin it enrolled
under and uploads it:

```
PUT <hub>/api/v1/enrolment/directory-proof
X-API-Key: <ingest key>
{"key_id": "…", "signature_input": "…", "signature": "…", "release": "<CARGO_PKG_VERSION>"}
```

Every upload carries `release`, the uploading binary's Cargo package version,
including the first proof at enrolment and each renewal. On the hub, `release`
is optional: absent and `null` both mean no release was reported. A supplied
value must be a string holding a semantic version without a `v` prefix, at
most 64 bytes long. A malformed release receives `400` with `{"detail": …}`;
the hub stores no proof, release or audit row for that refused upload, and
the edge preserves the explanation. Every accepted upload updates the
hub's reported release, even when `stored` is false because it keeps a proof
that expires later.

The hub leaves a key unlisted when the last accepted upload reported
no release, or a release below the hub's configured floor.

`expires` is `created` plus the lifetime the hub states, a week by default.
The hub answers `200` with `key_id`, `stored` and its current
`directory_proof` statement. It keeps whichever of the old and new proofs
expires later, or the new one where the held proof covers another authority.
It refuses with `400` without an ingest key, `401` for a revoked ingest key
(after a member removal or organisation closure), `404` where the named key
was not enrolled under this ingest key, `409` for a revoked edge key, `422`
for a proof that fails its checks and `503` without an identity origin.

The `directory_proof` statement on the enrolment, upload and status answers
may also carry `listed` (boolean) and `unlisted_reason` (string or null).
These state the hub's whole listing decision for the key. The hub sends
`unlisted_reason: null` exactly when `listed` is true; otherwise it sends a
sentence. Reasons are shown verbatim and never parsed. The edge does not
infer a release floor. Where `listed` is false, the edge records the key as
`unlisted` even while its proof remains current, preserving the hub's reason
verbatim with a `hub:` prefix when displayed or recorded in `edge_identity`.
A null or absent reason is reported as not supplied. Where these fields are
absent, listing continues to be derived from the held proof and its expiry.
Unknown members of the statement are ignored.

On an upload refusal of `401`, `404` or `409`, the edge treats the key as
unlisted and retains the hub's error text. This is the edge's conclusion
from the refusal, rather than a new directory statement from the hub.
On `400` or `422`, the hub stores nothing and keeps the older proof; the
edge retains the last statement and the normal proof-expiry rule. All
refusals record the HTTP status and the hub's text in `failure`. Transport
and server failures also retain the last statement and the expiry rule.
JSON `detail` and `message` strings are kept whole. Other error bodies are
cut at 2 KiB on a character boundary, with a truncation marker that gives
the original body size in bytes.

A proof is due where none is held, where less than 7,200 seconds of it remain,
or where it is more than 86,400 seconds old, reckoned as the stated
lifetime less the time remaining. It is also due when the locally recorded
release of the last accepted upload differs from the running binary's
release, or no release is recorded. The hub's `listed` decision and reason
text do not determine whether an upload is due. The edge does not sign
one for an authority other than the one it enrolled under, or where the
stated lifetime is 7,200 seconds or less. It asks:

- at `connect`, from the exchange answer;
- on every relay run that finds the key enrolled, from the standing answer
  (below): the session-end relay, `commonmeasure relay` and the hosted
  service's interval relay. After a failed upload, a proof due only because
  the release changed waits 3,600 seconds before another attempt; status
  reads during that delay do not restart it. Proofs due by age or expiry
  are attempted on every relay run;
- at session start and MCP server start, where a proof is due, inside the
  start's time budget; after a failure a start waits 3,600 seconds before it
  asks again, and a hub that states no proof is asked once a day.

A failure is never fatal. The edge records what it last learnt in
`directory-listing.json`: `key_id`, `checked_at`, the hub's statement and the
last failure. The edge records `last_uploaded_release` beside `stated`
only after an accepted upload and preserves it across status reads and
failed attempts. An accepted upload whose statement cannot be read still
records the release, keeps the pre-upload statement and records the parse
error in `failure`. An accepted answer without `directory_proof` clears
`listed` and `unlisted_reason` and uses the later proof expiry.
`listed` and `unlisted_reason` stay inside `stated`, alongside
the authority, lifetime and expiry. A file naming another key is ignored.
`edge_identity` reads its listing from this file
([session evidence §Events](session-evidence.md#events)).
`commonmeasure status` prints the stored listing and reason without asking
the hub; `status --json` carries `directory_listing` with `listed_until` or
`unlisted`.
An edge that stops reaching the hub drops out of the directory 7,200 seconds
before its last proof expires.

## Standing and revocation

On every relay run the edge asks for its key's standing:

```
GET <hub>/api/v1/enrolment/status
X-API-Key: <ingest key>
```

The hub answers `key_id`, `name`, `revoked_at`, `revocation` and, where it
has an identity origin, `directory_proof`; `404` where no edge key is
enrolled under the ingest key. Where the answer carries a revocation, the edge records it
in `enrolment.json`, once, and stops signing
([bot identity §Revocation](bot-identity.md#revocation)). No answer, or any
other status, leaves the recorded standing as it was and does not fail the
run. A `401` is recorded as the hub refusing the ingest key, not as a
revocation.

| Revoked by | Hub effect |
|---|---|
| an organisation owner, at the hub | the edge key is revoked (`owner`), its directory proofs deleted and its supplier credentials withdrawn; the ingest key stays valid |
| removing the member the key was enrolled for, or closing the organisation | the edge key (`owner`) and the ingest key are both revoked, proofs deleted, supplier credentials withdrawn |
| `commonmeasure disconnect` | the edge key (`edge`, unless already revoked) and the ingest key are revoked, proofs deleted, supplier credentials withdrawn |

After an owner's revocation the edge keeps delivering telemetry, because the
ingest key stands. After a member removal or organisation closure the
standing request is refused with `401`, so the edge does not learn of the
revocation: it goes on signing, and `edge_identity` reads `enrolled`, until
the operator disconnects.

## Disconnect

```sh
commonmeasure disconnect
```

The edge sends `POST <hub>/api/v1/enrolment/disconnect` with an empty body
and the ingest key; where it holds no ingest key for the hub it sends nothing
and reports the key as not revoked. Only `204` counts as revoked. Whatever the answer, it then
removes `relay.json`, `edge-key.json`, `enrolment.json`,
`directory-listing.json`, and `deployment.json` where its `policy_url` begins
with the enrolled hub URL. It retires every
`instances/by-session/` pointer and keeps the instance records and the relay
spool ([session evidence §Instance registration](session-evidence.md#instance-registration)).

Where the hub cannot be reached or does not answer `204`, `disconnect` still
removes the files, exits `0` and prints that the key was not revoked at the
hub and must be revoked there. It exits non-zero only where the home is not
enrolled, `enrolment.json` cannot be read or a file cannot be removed.

## When the hub cannot be reached

| Step | What the edge does |
|---|---|
| the exchange | fails naming the hub; nothing is written but the home directory |
| directory proof, first relay run, after a `201` | the enrolment stands; the failure is recorded or printed |
| policy sync | the last accepted policy keeps governing, and the outcome is `unreachable` ([policy envelope](policy-envelope.md)) |
| `connect --managed`, first sync | the enrolment stands; exit non-zero |
| standing check | the standing is left as recorded; the relay run goes on |
| directory proof renewal | recorded in `directory-listing.json`; the key stays listed until the held proof is 7,200 seconds from expiry unless the hub states it is unlisted or the edge concludes it is unlisted after an upload refusal of `401`, `404` or `409`; `400` and `422` retain the last statement and expiry rule |
| `disconnect` | local files removed; the key must be revoked at the hub |

## The ingest key

The ingest key in `relay.json` belongs to the receiver `relay.json` names,
on an enrolled edge the hub. Origins are compared by scheme, host and port.

- Delivery sends it only to the origin of that receiver.
- The standing check, and the directory proof sent from a relay run, use the
  key held for the enrolled hub's origin: the `relay.json` key where its
  receiver has that origin, or a key given with `--api-key` for a receiver at
  that origin.
- The directory proof sent at a start, the reporting approval refresh and
  `disconnect` send the `relay.json` key only where its receiver has the
  origin of the hub in `enrolment.json` (`EnrolmentRecord::hub_ingest_key`),
  and otherwise report that no ingest key is held for the hub and send
  nothing.

[Telemetry projection §Receiver and key](telemetry-projection.md#receiver-and-key)
states the same for the relay.

## Known gaps

- `disconnect` compares the deployment's `policy_url` with the enrolled hub
  URL by string prefix, not by origin. A hub enrolled under an internal
  address with a public `policy_url` leaves the pin in place, and
  `https://hub.example` also matches `https://hub.example.evil/…`.
- The hub's identity origin is compared by the edge with a lower-cased host
  and the default port removed; the hub does not normalise it. An origin
  configured with capitals or `:443` makes every directory proof unsignable.
- The edge refuses unknown members under `identity`, so a hub that adds one
  would fail the edge's parse after the token was spent, leaving an enrolled
  key the edge does not hold.
