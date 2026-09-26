---
title: Bot identity contract
domain: network
audience: integrator
section: reference
---

# Bot identity contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

- **Owner:** Network.
- **Producer:** each enrolled edge, which signs its requests to sources with
  its own key; and the hub, which publishes the key directory, the agent card
  and the bot page.
- **Consumer:** publishers and the verifiers acting for them, which check a
  request's signature against the hub's key directory.
- **State:** built. Signing, the directory proof and revocation are
  `fixture-tested`: each side runs against a loopback counterpart, and both
  are held to the same signature vectors. The edge's end-to-end tests verify
  its requests with an independent verifier written from the Web Bot Auth
  drafts. No verification by a publisher's or a CDN's verifier is recorded,
  and `CommonMeasureBot` is not registered with any verified-bots programme.

`CommonMeasureBot` is the name every enrolled edge presents to a source. The
name is shared; the key is not. Each edge holds its own key, and a publisher
trusts a request by its signature, never by its `User-Agent`. The mechanism is
Web Bot Auth: HTTP Message Signatures (RFC 9421) with an Ed25519 key published
in an HTTP Message Signatures directory.

## The key

- **Algorithm:** Ed25519, generated from the operating system's random source
  during `connect` ([enrolment](enrolment.md)).
- **Storage:** `<home>/edge-key.json`, a JWK
  `{"kty":"OKP","crv":"Ed25519","x":…,"d":…}` with `x` and `d` in unpadded
  base64url. On Unix it is written with mode `0600`, synced and renamed into
  place. A file of another key type or curve is an error, not an absent key.
- **One key per home.** `connect` refuses while an unrevoked enrolment stands
  and mints a new key after a revocation. The hub refuses a public key it has
  enrolled before (`409`).
- **Key id:** the RFC 7638 thumbprint: SHA-256 over the canonical JSON of
  `{"crv":"Ed25519","kty":"OKP","x":<x>}`, unpadded base64url, no prefix. The
  hub assigns it at enrolment and the edge checks it against its own key
  before saving anything. The RFC 8037 appendix A.3 key gives
  `kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k`
  (`crates/commonmeasure-harness/src/identity.rs`).

## Signed requests

An enrolled edge signs, with the tag `web-bot-auth`:

- each mediated page fetch, and each redirect hop, signed again for its own
  authority;
- the `robots.txt`, licence and Content Telemetry manifest probes a fetch
  makes. Each probe other than `robots.txt`, and each redirect from one, is
  ruled by `robots.txt` at its own origin before it is sent, except a
  licence a `License:` line names at the origin of the page being fetched;
  the manifest probe after a 404 asks the registrable domain once, and that
  fallback never chooses a public suffix
  ([session evidence](session-evidence.md#manifest-discovery));
- the managed policy fetch from its hub
  ([`docs/contracts/policy-envelope.md`](policy-envelope.md));
- the supplier credential release request
  ([`docs/contracts/supplier-credentials.md`](supplier-credentials.md)), which
  also covers `@method` and `@path`.

Instance registration requests are signed with the same key under another
tag ([instance registration](instance-registration.md)). Telemetry delivery,
the enrolment routes, the reporting approval refresh and supply adapter calls are
not signed; an adapter's
request carries the adapter's own `User-Agent`.

The signature covers, in order: `"@authority"` (the lower-case host, with the
port only where it is not the scheme's default), `"signature-agent"`, any
further component the request names, and `"content-telemetry-id"` where the
request carries a `Content-Telemetry-ID` header. A header that is absent is
not signed as empty. The parameters are:

```
sig1=("@authority" "signature-agent");created=1758614400;expires=1758614700;keyid="<key id>";alg="ed25519";nonce="<16 random bytes, base64url>";tag="web-bot-auth"
```

`expires` is 300 seconds after `created`. The headers sent are:

| Header | Value |
|---|---|
| `Signature-Input` | `sig1=` and the parameters above |
| `Signature` | `sig1=:<base64>:` |
| `Signature-Agent` | the hub's identity origin as a quoted string, for example `"https://hub.example"`; a verifier resolves the key directory at that origin |
| `User-Agent` | on page fetches and their probes, `CommonMeasureBot/<version> (+<bot page>)`, with `; <contact>` before the `)` where the hub states a contact; the policy fetch carries `commonmeasure-managed/<version>` and the release request `commonmeasure/<version>` |

The identity origin is the one the hub returned at enrolment; the edge never
derives it. `crates/commonmeasure-harness/src/identity.rs` holds the signer
and a signature vector the hub's verifier is also tested against.

An edge that is not enrolled sends `User-Agent: CommonMeasureBot/<version>`
and no signature. So does an edge that has learnt its key was revoked, and an
edge whose enrolment cannot be loaded (for example, a record without its key)
still fetches, unsigned, with the reason recorded. Each mediated crossing
whose request left the machine records what it presented: `identity` carries
`user_agent` and either `key_id` and `signature_agent` or `unsigned` with the
reason ([session evidence §Crossing](session-evidence.md#crossing)). A request
that should be signed and cannot be is not sent; a `robots.txt` probe that
fails this way refuses the crossing
([session evidence §Source declarations](session-evidence.md#source-declarations)).

The edge refuses every crossing to its hub's own authorities, so an agent
cannot use the edge's signature to act on the hub
([session evidence §Events](session-evidence.md#events)).

## The key directory

The hub serves
`<identity origin>/.well-known/http-message-signatures-directory` as
`application/http-message-signatures-directory+json` with
`Cache-Control: public, max-age=3600`:

```json
{"keys": [{"kty": "OKP", "crv": "Ed25519", "kid": "<key id>", "x": "…", "use": "sig", "nbf": 1758000000}]}
```

`nbf` is the enrolment time. A key is listed while it is unrevoked, its
organisation is open, and the hub holds a directory proof from it for the
identity origin that expires at least 7,200 seconds from now: two cache ages,
so a copy cached for its full hour still verifies for another hour. An edge's
key is therefore absent between enrolment and its first proof, and after its
proof lapses.

The response carries one signature per listed key, in the order of `keys`,
under labels `binding0`, `binding1` and so on: each is that key's directory
proof. The hub signs nothing in the directory with a key of its own. An empty
directory carries no signature headers.

The hub's identity origin is configuration, never read from a request. A hub
without one answers `503` on the identity documents and refuses enrolment.

## The directory proof

A directory proof is an RFC 9421 signature by the edge's key over
`("@authority";req)` alone, the directory's authority, with tag
`http-message-signatures-directory` and the same six parameters as a request
signature. It covers no body, because the directory changes with every
enrolment and revocation. The edge makes it and the hub stores it; the hub
cannot make one for a key it does not hold.

The hub accepts a proof whose `created` is at most 60 seconds ahead of its
clock, whose lifetime is at most the hub's stated proof lifetime (a week by
default, and never 7,200 seconds or less) and whose `expires` is at least
7,200 seconds away. It keeps whichever of the old and new proof expires
later, or the new one where the held proof covers another authority.
Every proof upload reports the binary's release. The hub may withhold a key
with a current proof and state why; the edge preserves that decision and
reason. The upload body and listing statement are defined in
[enrolment §The directory proof and its renewal](enrolment.md#the-directory-proof-and-its-renewal).

On each session start and MCP server start the edge records the listing it last learnt in
`edge_identity`: `listed_until`, the proof's expiry less 7,200 seconds, or
`unlisted` with the reason ([session evidence §Events](session-evidence.md#events)).
A hub-supplied reason keeps its text with a `hub:` prefix. The same stored
listing appears in `commonmeasure status` without a network call.
A signed request under a key the directory does not list verifies nowhere.

## The agent card

The hub serves `<identity origin>/.well-known/signature-agent-card.json`:

```json
{
  "client_id": "https://hub.example/.well-known/signature-agent-card.json",
  "client_name": "CommonMeasureBot",
  "client_uri": "https://hub.example/bot",
  "jwks_uri": "https://hub.example/.well-known/http-message-signatures-directory",
  "web_bot_auth": {
    "expected-user-agent": "CommonMeasureBot/*",
    "rfc9309-product-token": "CommonMeasureBot",
    "rfc9309-compliance": ["User-Agent", "Allow", "Disallow"],
    "trigger": "fetcher",
    "purpose": "ai-input",
    "targeted-content": "Public web pages an operator's agent reads as grounding for its own work"
  },
  "contacts": ["…"]
}
```

`contacts` is present where the hub is configured with one. The card carries
no keys.

`rfc9309-compliance` lists the `robots.txt` records the edge obeys for the
`CommonMeasureBot` product token. The edge also honours `Crawl-delay`, in
every policy mode, and treats an unreachable `robots.txt` as RFC 9309 §2.3.1
says ([session evidence §Source declarations](session-evidence.md#source-declarations)).
The card does not yet list `Crawl-delay`.

## The bot page

The hub serves a page at `<identity origin>/bot`, the URL in the
`User-Agent`. It states what the bot does (it fetches what an operator's agent
asks for and does not crawl), that each edge has its own key, how to write a
`robots.txt` group for `CommonMeasureBot`, where the directory, card and
manifest are, how to verify a signature, how revocation works and its bound,
and whom to contact, quoting the key id. It reads the contacts from the card
and the bound from the directory's cache age.

## Revocation

A key stops being listed when:

- an organisation owner revokes it at the hub
  (`DELETE /api/v1/enrolment/keys/{key_id}`, answered `204`);
- Common Measure revokes it: support, acting as an owner of the edge's
  organisation, makes the same owner revoke. Acting as an owner is otherwise
  read-only; this revoke is one of the stop actions it leaves open, and the
  hub's audit log records it under the support operator's own identity;
- the edge gives it up with `commonmeasure disconnect`
  ([enrolment](enrolment.md));
- the member the key was enrolled for is removed from the organisation (a
  key enrolled before tokens named a member has none), or the organisation is
  closed.

The hub removes the key and its proof from the directory in the same
transaction. A verifier that cached the directory keeps accepting the key for
at most its cache age, 3,600 seconds. That is the revocation bound.

The edge learns of an owner's revocation at its next relay run, which asks the
hub for the key's standing, and records it in `enrolment.json`; from then on
it signs nothing and `edge_identity` reads `revoked`. Until that run it goes on
signing with the revoked key, and verifiers refuse those requests once their
copy of the directory has expired.

Removing a member or closing the organisation also revokes the ingest key.
A `401` in the hub's error shape for the enrolled ingest key on the
standing check, whether at relay or start-time proof renewal, records
revocation with the hub's text and stops signing. A `401` in that shape on
proof upload does the same. A `401` in any other shape, such as an ingress
page, is reported as the hub not reached and, as when the hub cannot be
reached, changes no standing. `edge_identity` and
`status` say revoked; the edge records when it learnt the refusal and leaves
the hub's revocation time unknown. A `401` for another key, such as one
given with `relay --api-key`, changes nothing, and a `200` for the key with
no revocation, requested after the `401` was learnt, withdraws a revocation
learnt only from a `401`
([enrolment §Standing and revocation](enrolment.md#standing-and-revocation)).
Transport failures and 5xx leave the recorded standing unchanged.

"Stops signing" covers sessions already open. An MCP server or a hosted
service session loads the key when it starts, and reads `enrolment.json`
again before each signature, whether for a mediated request, a signed
instance request or a directory proof:

- While the record says revoked, or cannot be read, the session refuses that
  signature and the fetch fails. It never falls back to an unsigned request.
  A record that is gone, or that names another key, is refused the same way.
- The refusal lasts only while that state holds. A read that fails once
  refuses that signature only; the next signature reads the record again. A
  withdrawn 401-only revocation
  ([enrolment §Standing and revocation](enrolment.md#standing-and-revocation))
  lets the same session sign again. A revocation the hub stated is permanent.
- The MCP server and the hosted service return the refusal as a tool error
  and do not retry it.
- A new session reads the record when it starts, and what it does depends
  on the record:
  - missing or revoked: the session runs unsigned;
  - a new enrolment that `commonmeasure connect` wrote, with its key: the
    session signs under the new key;
  - a record that cannot be read: the session runs unsigned, with the read
    error as its reason ([§Signed requests](#signed-requests)). Relay
    runs, standing checks, directory-proof renewal, instance registration,
    supplier credentials and managed policy sync load the identity and fail
    with that error. `connect` and `disconnect` read the record first and
    refuse too, so neither repairs it. The operator repairs the file or
    moves it aside; with it moved aside, `connect` enrols a new key.
    `disconnect` cannot revoke the old key or its ingest key without the
    record, so an owner revokes them on the hub's API keys page.

The edge's bound is therefore the time until one of its processes records the
revocation: the next relay run, or the next session or server start that
asks for standing. The hosted service's interval relay runs every 300
seconds by default.

Known gaps:

- If the hub's identity origin changes, the edge does not sign a proof for the
  new origin, because it enrolled under the old one. The edge must disconnect
  and connect again to be listed.
