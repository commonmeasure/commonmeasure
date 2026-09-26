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

The hub URL must be `https`, or `http` to `127.0.0.1`, `localhost` or
`[::1]`, using the same rule as `policy_url`, and carry no credentials,
query or fragment, which the relay refuses in the receiver `connect` derives
from it ([telemetry projection §One receiver](telemetry-projection.md#one-receiver)).
`connect` refuses other URLs before making a request or writing the home. Where `enrolment.json` holds
a hub URL that fails the rule, the edge sends that hub nothing: a cleartext
URL, and one carrying credentials, a query or a fragment. The transport
never sends credentials, every hub path is appended to the stored URL so a
query or fragment swallows it, and the hosted edge pins the URL as its token
issuer, which it publishes to callers with no token. Relay
(before any spool or relay state is written), standing checks, directory
proofs, managed policy, reporting approvals, supplier credentials, every
signed instance request including the check before a mediated crossing, and
the start of `commonmeasure hosted serve` and `hosted service` are refused with the remedy. `status` (text and `--json` `egress.hub_refused`,
with `egress.key_standing`), `doctor` and the session's
`edge_identity` (`standing`, with `hub_refused`) state it; the standing is
`cleartext_hub` for a cleartext URL and `unusable_hub_url` for the other.
The refusal names the hub by its origin alone, since a hub URL stored by an
earlier `connect` can carry credentials. Directory status (`commonmeasure
enrol` and MCP `context_enrol`) names the enrolled hub by its origin too,
in `edge.hub`, or `null` where the stored URL has none.
`disconnect` does not ask the hub, says so and why, removes the local
enrolment, and names the edge key and the ingest key to revoke on the hub's
API keys page; then re-run `connect` with an https hub, by its address alone.

## What the edge writes

After a `201`, `connect` checks the receiver it will write (the hub URL given
plus `telemetry_path`) by the rule the relay loads `relay.json` with. A
receiver the relay would refuse stops `connect` before anything is written,
and the error names the key id the hub registered, to revoke on the hub's
API keys page. Otherwise, in this order, under the home (`$COMMONMEASURE_HOME` or
`~/.commonmeasure`):

1. `edge-key.json`: the private key ([bot identity §The key](bot-identity.md#the-key)),
   mode `0600` on Unix, synced and renamed into place. It is written before
   the record that names it.
2. `enrolment.json`: `hub` (the URL given, without a trailing `/`),
   `organization`, `name`, `key_id`, `identity` and `enrolled_at` (the edge's
   clock); after a revocation is learnt, also `revocation` (`owner`, `edge`
   or `hub:` followed by the refusal text) and `revocation_learnt_at`.
   `revoked_at` holds the hub's revocation time only when supplied. A `401`
   leaves that time unknown; `revocation_learnt_at` alone also means revoked.
   When the hub states a revocation after a `401` was recorded, `revoked_at`
   and `revocation` take the hub's values and `revocation_learnt_at` keeps
   the first time.
   Unknown members are refused; the fields keep their existing shape.
   A process of another release reading this record signs after a 401
   revocation; see `ARCHITECTURE.md` §What runs where.
3. `relay.json`: `receiver`, the hub URL given plus `telemetry_path`, and
   `api_key`, mode `0600`, with no `suppliers` list, so the hub takes every
   cleared event
   ([telemetry projection §Supplier scope](telemetry-projection.md#supplier-scope)). Where a `relay.json` existed, `connect` prints the
   receiver it named.
4. With `--managed`, `deployment.json`: `mode` `managed`, the `signer` from
   `policy_signer`, the `policy_url` and the organisation. It is read back
   through the loader and removed if the loader refuses it
   ([policy envelope §Deployment mode](policy-envelope.md#deployment-mode)).
   `policy_url` must be `https`, or `http` to `127.0.0.1`, `localhost` or
   `[::1]`.
5. `directory-listing.json`, after the first attempt at a directory proof,
   whatever its outcome (below).

Each of these files is written whole through a temporary file of the
writer's own, `<file>.<pid>.<nonce>.tmp` beside it, and renamed into place.
The temporary files for `edge-key.json` and `relay.json` are created with
mode `0600`. A change to the stored `enrolment.json` (a refusal, a stated
revocation, a withdrawn refusal) and its removal by `disconnect` are made
under an exclusive lock on `enrolment.lock` beside it, against the record as
it then stands. A process holding an older copy of the record therefore
cannot write it back over a newer revocation, a disconnect or a new
enrolment. `connect` replaces the record whole under the same lock, without
reading it again. Readers take no lock. A relay run whose hub says the key
stands reads the record without the lock and takes the lock only when the
record holds a refusal that answer withdraws, reading the record again
under it. `enrolment.lock` is created readable by its owner only. A writer
waits up to 10 seconds for it; then the write refuses, changes nothing and
says another process has held `enrolment.lock`. A lock file that exists and
cannot be opened is named with its own remedy: make it readable and
writable by this user, or remove it while no process holds it.

Two `connect` runs at once are not serialised. Each writes its own key,
record and `relay.json`, and the files left can come from different runs. A
key and record that do not match fail identity loading, so a new session
runs unsigned with the mismatch as its reason and nothing is signed under
either key. `disconnect` and a new `connect` repair the edge; the hub can
still hold the other run's key, which an owner revokes on the hub's API keys
page.

A writer killed between creating its temporary file and renaming it leaves
that file behind. Nothing removes it: not a later write, not `disconnect`.
It holds some or all of the bytes that writer meant to store, which for
`edge-key.json` and `relay.json` is a credential at mode `0600`. The
operator removes the `<file>.*.tmp` files of these five once no
`commonmeasure` process is running. `enrolment.lock` is not one of them and
stays: a process waiting on the old lock file and one that locked a new file
would each believe it held the lock.

`connect` then runs one relay
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

The hub answers `401`, `404` and `409` on this route only as
`application/json` holding `{"detail": "<text>"}`. The edge accepts that
shape with, optionally, a string `code` beside `detail`, and no other
member; it does not store `code`. On an upload answer of `401`, `404` or
`409` in that shape, the edge treats the key as unlisted and retains the
hub's `detail`. This is the edge's conclusion from the refusal, not a
directory statement from the hub, and the listing file marks it as one
(below). For a `404` or `409`, `status`, `edge_identity` and the relay
report show it as `this edge concluded the key is unlisted when the hub
refused its directory proof (<status>): <detail>`, never with the `hub:`
prefix; after a `401` with the enrolled key, the key is revoked (below) and
the revocation is shown instead. The same status in any other shape, such
as an ingress or proxy page, HTML, or no body, came from something in front
of the hub: the edge records it in `failure` as the hub not reached, keeps
the last statement and the expiry rule, and waits the retry delay as after
any failure. A `401` in that shape, when the key refused is the enrolled
ingest key, also records revocation and stops signing, as on a standing
check; a `401` in any other shape is recorded in `failure` only. Every
refusal the hub's handlers and middleware write on this route is in that
shape, so an answer of any other status in another shape, such as a
firewall's `403` or a proxy's `502`, is also recorded in `failure` as the
hub not reached. The hub's own empty `503` when a request exceeds its time
limit reads the same way. On `400` or `422`, the hub stores
nothing and keeps the older proof; the edge retains the last statement and
the normal proof-expiry rule. A `429` or a `5xx` in the hub's shape rules
on nothing: `failure` reads `the hub could not take the proof (<status>):
<detail>; nothing was concluded from it`, and the edge keeps the last
statement and the expiry rule. All refusals record the HTTP status and the
answer's text in `failure`. Transport and server failures also retain the
last statement and the expiry rule.
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
the authority, lifetime and expiry. An edge conclusion from a refused
upload is held as `edge_conclusion` inside `stated`, an object with the
refusal's `status` and the hub's `detail`; `listed: false` and
`unlisted_reason` then repeat it, so that 0.4.1 and earlier, which ignore
the member, still read the key as unlisted. Unknown members of
`edge_conclusion` are ignored. `edge_conclusion` is never
read from the hub's statement, so no member the hub sends can claim an edge
conclusion. The next statement from the hub replaces the conclusion. A file
written by 0.4.1 or earlier after a `401`, `404` or `409` holds the
conclusion as `listed: false` with no marker: it cannot be told apart from
a hub statement, reads as one (`hub: <text>`), and the next statement from
the hub replaces it. A file naming another key is ignored.
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
([bot identity §Revocation](bot-identity.md#revocation)). The time the edge
first learnt of a revocation is kept. A revocation the hub states after a
`401` was recorded replaces the `401`'s text with the hub's time and side;
one the hub already stated is kept as first recorded. A `401` on this route
in the hub's error shape (`application/json` holding `{"detail"}`,
optionally with a string `code`, as on the directory-proof route above) for
the enrolled ingest key (the key in `relay.json` for the hub's origin) also
records revocation, retains the hub's text and stops signing. This applies
both on a relay run and when a session or server start asks for standing to
renew a proof. The time the edge learnt the refusal is recorded; the hub's
revocation time remains unknown. An answer of any status but `200` in any
other shape, such as an ingress or proxy page, HTML or no body, came from
something in front of the hub: the edge reports the hub not reached, naming
the status and the answer, and changes nothing, as when the hub cannot be
reached at all. The hub's own empty `503` when a request exceeds its time
limit reads the same way. Any other status in the hub's shape is reported
as the hub's answer. Revocation from a `401` depends on the hub's answer
reaching the edge unmodified: an ingress that rewrites it leaves the edge
signing until a hub-shaped `401` arrives. A `401` for any
other key, such as one given with
`relay --api-key`, is reported as the standing not checked, naming where
the key came from, and changes nothing. A later `200` for the same `key_id`
with no revocation withdraws a revocation learnt only from a `401`: the hub
never answers `200` for a revoked key. Only a `200` requested after the
`401` was learnt counts, so a relay run whose request went out before another
process recorded the `401` does not withdraw it. A revocation the hub stated
is permanent. Transport failures, 5xx and other status codes leave the recorded
standing unchanged.

| Revoked by | Hub effect |
|---|---|
| an organisation owner, at the hub | the edge key is revoked (`owner`), its directory proofs deleted and its supplier credentials withdrawn; the ingest key stays valid |
| removing the member the key was enrolled for, or closing the organisation | the edge key (`owner`) and the ingest key are both revoked, proofs deleted, supplier credentials withdrawn |
| `commonmeasure disconnect` | the edge key (`edge`, unless already revoked) and the ingest key are revoked, proofs deleted, supplier credentials withdrawn |

After an owner's revocation the edge keeps delivering telemetry, because the
ingest key stands. After a member removal or organisation closure the
standing request is refused with `401`; the edge records revocation and
stops signing. The next session's `edge_identity` and `status` say revoked,
with the hub's refusal text; they do not claim the key is out of the
directory, which a `401` does not tell the edge. A session already open
refuses each signature while the record says revoked or cannot be read, and
its fetches fail rather than go unsigned
([bot identity §Revocation](bot-identity.md#revocation)). After a member
removal or organisation closure a new enrolment is needed to resume signing.

## Disconnect

```sh
commonmeasure disconnect
```

The edge sends `POST <hub>/api/v1/enrolment/disconnect` with an empty body
and the ingest key; where it holds no ingest key for the hub, or the stored
hub URL is one nothing is sent to ([the exchange](#the-exchange)), it sends
nothing and reports the keys as not revoked. Only `204` counts as revoked. Whatever the answer, it then
removes `relay.json`, `edge-key.json`, `enrolment.json`,
`directory-listing.json`, and `deployment.json` where its `policy_url` begins
with the enrolled hub URL. It retires every
`instances/by-session/` pointer and keeps the instance records and the relay
spool ([session evidence §Instance registration](session-evidence.md#instance-registration)).

Where the hub cannot be reached or does not answer `204`, `disconnect` still
removes the files, exits `0` and prints that the edge key and the ingest
key were not revoked at the hub and must be revoked there, naming the edge
key id and the label both keys carry (the edge's name). Its output names the
hub, and the policy URL of a `deployment.json` it removes or keeps, by origin
alone. It exits non-zero
only where the home is not enrolled, `enrolment.json` cannot be read or a
file cannot be removed.

## When the hub cannot be reached

| Step | What the edge does |
|---|---|
| the exchange | fails naming the hub; nothing is written but the home directory |
| directory proof, first relay run, after a `201` | the enrolment stands; the failure is recorded or printed |
| policy sync | the last accepted policy keeps governing, and the outcome is `unreachable` ([policy envelope](policy-envelope.md)) |
| `connect --managed`, first sync | the enrolment stands; exit non-zero |
| standing check | the standing is left as recorded; the relay run goes on |
| directory proof renewal | recorded in `directory-listing.json`; the key stays listed until the held proof is 7,200 seconds from expiry unless the hub states it is unlisted or the edge concludes it is unlisted after an upload refusal of `401`, `404` or `409` in the hub's error shape; `400`, `422` and those statuses in another shape retain the last statement and expiry rule |
| `disconnect` | local files removed; the edge key and the ingest key must be revoked at the hub |

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
