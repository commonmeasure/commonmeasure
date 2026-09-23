---
title: Supplier credential custody contract
---

# Supplier credential custody contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

**Verification state:** the Edge and Hub custody implementations have fixture
coverage. On 22 September 2026 the deployed hosted edge serving Word made an
Exa search with a Hub-released key; names-only connection provenance and fresh
Hub receipts were verified. Word reported USD 0.007, but the inspected
acquisition records do not persist that charge. Complete case 1, live
rotation/revocation and the value-based leakage sweep remain open. The
operational evidence is kept privately. On the same day an Ozone Live key
released the same way completed one native Word search, with Hub connection
provenance, one admitted source and a fresh automatic Hub receipt; its charge
and licence remain unknown.

This contract covers credential custody on the **existing supplier API** path,
for Exa, Ozone Live and Tavily: an organisation's supplier account key is held
by the Hub and released to that organisation's hosted edges. It establishes
credential custody, per-edge authorisation, release, reload, limits and audit.
It claims no entitlement, licence, publisher admission, reporting mapping or
connector. The owner's decisions are at the end.

An organisation connects a supplier account once, and a permanent supplier key
stays with an authorised credential service rather than with each temporary
instance. A hosted edge is a durable enrolled edge belonging to one
organisation ([`docs/GLOSSARY.md`](../GLOSSARY.md) §Hosted edge), not a
temporary instance, so releasing the organisation's key to a named enrolled
edge keeps to that rule. Releasing it to a registered instance is not covered
here.

## Identities and relationships

| Identity | Identifier and relationship |
|---|---|
| Organisation | Reuse Hub `organization.id`. Resolved from the enrolled key on release, from the session on administration; never from a request body. |
| Supplier connection | New Hub row, one per connected supplier account: `id` (opaque), `organization_id`, `provider` (a name `required_variable` in `crates/commonmeasure-supply/src/lib.rs` maps), `label`, `ciphertext`, `created_by`, `created_at`, `rotated_at`, `revoked_at`, `revoked_by`. A connection belongs to one organisation. `provider` names a remote supplier, `exa` or `tavily` in this slice. The edge holds an entry only where `variable` equals its own `required_variable(provider)`, and refuses `internal` and `skill:` providers, whose variables are paths on the edge's machine. |
| Enrolled edge | Reuse the hub's `edge_keys.key_id`, assigned at enrolment. An edge belongs to one organisation; the mapping from key id to organisation lives only in that table. |
| Edge authorisation | New Hub row `(connection_id, key_id, authorised_by, authorised_at, withdrawn_at, withdrawn_by)`. Both sides must resolve to the same organisation, enforced by constraint and by test, not by the administration screen alone. |
| Provider variable | The environment name `required_variable(provider)` returns. It is the only name the edge's `unavailable` message, credentials doctor and adapter construction share. The hub holds a copy for the providers it serves, because the release answer carries `variable`; the edge holds a released value only when that `variable` is the one `required_variable(provider)` returns. |

One connection may be authorised to many edges of its organisation. One edge
may hold authorisations to several connections, at most one per provider: an
authorisation for a second connection of the same provider is refused until
the first is withdrawn. The connection id is stable across rotation.

## Custody at the hub

- The credential value is stored as ciphertext under the hub's existing
  `API_KEY_ENCRYPTION_KEY` (XChaCha20-Poly1305), under which the hub already
  keeps its own API keys retrievable by their owner. That retrievability is
  accepted for the hub's own keys; a supplier key opens a third party's
  service, so its exposure reaches beyond the hub.
  The contract therefore forbids reveal: an owner can create, rotate, revoke
  and authorise, and can never read the value back through any read or
  screen. Until the hub can refuse a key that is not a hosted edge's, an owner who enrols an edge and authorises it obtains the
  value through a release; the record of those requests is the control.
- Administration is owner-only, on a screen under **Organisation settings**
  beside **API keys**. A member sees which connections an edge they own is
  authorised to and nothing else.
- Create takes provider, label and value; the value is accepted once, over the
  session, and is not echoed in the response, the page or the audit row.
- Rotate takes a new value for an existing connection. It replaces the
  ciphertext, sets `rotated_at`, and MUST leave the connection id and every
  edge authorisation unchanged.
- Revoke sets `revoked_at` and drops the ciphertext in the same statement, as
  API-key revocation does. Revocation withdraws the connection's
  authorisations in the same transaction. A revoked connection MUST NOT be
  re-authorised or rotated; the owner creates a replacement.
- The value MUST NOT appear in logs, error bodies, audit detail, telemetry,
  policy envelopes, browser state after the create or rotate request, or any
  Hub response other than the release below.

## Authorisation of edges

An owner authorises a connection to an enrolled edge by key id, and withdraws
it the same way. Authorisation MUST refuse a key id enrolled under another
organisation, a revoked edge key and a revoked connection, and the refusal
must not disclose whether the foreign key id exists (404, as the enrolment
routes answer). Revoking an edge key withdraws its authorisations in the same
transaction. Withdrawal and revocation are effective at the next release
request; the edge-side bound is stated under Revocation.

## Release

The edge fetches the credentials it is authorised to hold on a management
route of its own, `GET /api/v1/supplier-credentials`. The policy envelope
carries no credential and its signature covers endpoint authority only
(`docs/contracts/policy-envelope.md` §The envelope); this contract keeps that
invariant and adds a second signed request rather than a field.

| Aspect | Requirement |
|---|---|
| Authentication | The edge's enrolled Ed25519 key, RFC 9421 under the Web Bot Auth profile, as the desired-policy request ([`policy-envelope.md`](policy-envelope.md)), and also covering `@method` and `@path` (§Signature components of the release request). The ingest key MUST NOT authenticate this route: it is a delivery credential. |
| Replay | The desired-policy route does not store nonces because a replay buys only the organisation's own policy (`policy-envelope.md` §The hub side). This route serves a secret, so the hub MUST store nonces for the signature window and refuse a repeat. The hub MUST refuse a signature on this route whose `expires` is more than 360 seconds after `created`, and MUST keep each nonce past the last second it would accept the signature, judged by the clock that verified it. |
| Response | `200` with `{organisation, key_id, served_at, valid_for_seconds, credentials: [{connection_id, provider, variable, value, rotated_at}]}`, one entry per current authorisation whose connection is not revoked. `valid_for_seconds` is the kept-release bound of §Validity of a release (owner decision 7). `value` is plaintext under TLS and nothing else in this slice (owner decision 3). The hub records `credentials_fetched_at` on the edge key row as it records `policy_fetched_at`. |
| Caching | `Cache-Control: no-store`, `Pragma: no-cache`, and the response MUST NOT pass through any cache the hub operates. The web layer's `/api` proxy forwards `Cache-Control` unchanged, as it does for the policy route; it does not yet forward `Pragma`. |
| Empty | An edge with no authorisation receives `200` with an empty list and `valid_for_seconds`. That is a valid state, not a refusal: it is what a withdrawn edge sees. |
| Refusals | `401` with a stable `code` (§Signature components of the release request): invalid or expired signature, unknown or revoked key id, wrong authority, missing components, missing or repeated nonce, window over 360 seconds. `400` `query_refused` for a query string. `404` with a stable detail when the hub has no public origin configured, for a request that passes the signature check. No refusal names another organisation's connections. |
| Audit | Every `200` writes one `supplier_credential.release` row naming the key id and the connection ids served, never values. |

### Validity of a release

Owner decision 7. Planned on both sides; until both land, the edge
keeps a set for three intervals and the hub sends no `valid_for_seconds`.

- Every `200` of the release route, an empty one included, MUST carry
  `valid_for_seconds`: an integer from 60 to 3600, 900 unless the hub's
  deployment configuration sets another value. It is a duration, not a time:
  the hub states no clock reading the edge must trust.
- The edge measures it from the moment it has read the whole response body,
  on a monotonic clock that is not set by wall-clock changes. On Linux that
  clock MUST count time the host was suspended (`CLOCK_BOOTTIME`); on a
  platform with no such clock the acceptance record says the bound excludes
  suspension.
- A `200` whose `valid_for_seconds` is missing, not an integer, or outside 60
  to 3600 is not a release document: the fetch is `unreachable`, recorded with the
  fixed text `validity_missing` (§Edge side), and the set already held keeps its own
  deadline. An edge of this revision against a hub before it therefore keeps
  nothing new; the hub is deployed first.
- Each `accepted` or `unchanged` fetch sets the held set's deadline to the
  earlier of receipt plus `valid_for_seconds` and receipt plus three
  `interval_seconds`. `unreachable` leaves the deadline where it is; `refused`
  and an empty `200` clear the set at once, as today. At or past the deadline
  the set is expired where a value is read.
- A hosted service whose `hosted-service.json` sets `supplier_custody` MUST
  refuse to start when `interval_seconds` exceeds 300, naming the field and
  the limit. The kept set's longest life is therefore 900 seconds, whatever
  the hub states.
- The connection answer's `refresh_seconds: 300` becomes the most a custody
  edge may run between fetches, which the start check makes true of every
  conforming edge, and the answer gains the hub's `valid_for_seconds`. The Hub
  screen states both bounds.
- Pinned by a test on each side: the hub's default 900 and range 60 to 3600;
  the edge's cap of 300 on `interval_seconds` with custody on.

### Signature components of the release request

**Status, 21 September 2026: built on both sides, landed together.** The
two met once on loopback before landing: the ignored test in
`crates/commonmeasure-cli/tests/supplier_custody_e2e.rs` passed against the
hub's candidate revision, and six signed releases each answered `200`. A hosted hub before this landing refuses every
release from an edge at or after it, so the hosted hub is deployed first.

Today a release request differs from a desired-policy request by its stored
nonce alone, so a signed request captured before the hub first sees it can be
presented once at the release route.

- The edge MUST cover `@method` and `@path` on the release request, in
  addition to `@authority` and `signature-agent`. `@path` is the path of the
  URL the edge requested, `/api/v1/supplier-credentials`.
- One release request is fixed by value in
  [`supplier-credentials-release-vector.json`](supplier-credentials-release-vector.json):
  the key, `created`, the nonce, the signature base and the three signature
  headers. The edge's signer and the hub's verifier are each tested against
  it. The edge lists the components as `@authority`, `signature-agent`,
  `@method`, `@path`; a verifier rebuilds the base in the order
  `Signature-Input` lists and MUST NOT require that order.
- The hub MUST refuse, with `401`, a release request whose signature does not
  cover `@method` and `@path`, or whose covered values differ from `GET` and
  the release path as the edge addressed it. Where the hub's server sits
  behind its web layer's `/api` proxy, the hub verifies the path the edge
  signed, not the path the proxy forwarded.
- The hub MUST refuse a release request that carries a query string, with
  `400` and the code `query_refused`. The route takes none, and `@query` is
  not covered. A `400` rules on nothing, so an edge keeps its released set.
- The desired-policy route MUST keep accepting a signature that covers
  `@authority` and `signature-agent` only, so that released edges keep
  working. It MUST also accept one that covers `@method` and `@path` and MUST
  then verify them.
- The edge MUST NOT add `@method` or `@path` to the desired-policy request, or
  to any request it signs for a publisher. A hub revision before this change
  reads an unknown covered component as a header name and refuses the
  signature, and a publisher's verifier is not the hub's to change.
- Two numbers are pinned against each other by a test on each side. The
  edge's nonce is 16 random bytes in unpadded base64url, 22 characters; the
  hub's minimum is 22 characters. The edge's signature window is 300 seconds;
  the hub's cap is 360 seconds.
- Each `401` of the signed-edge routes MUST carry a stable `code` beside its
  `detail`: `key_unknown`, `key_revoked`, `signature_invalid`,
  `signature_expired`, `window_too_long`, `authority_mismatch`,
  `components_missing`, `nonce_missing`, `nonce_repeated`. `key_unknown` and
  `key_revoked` are rulings on the key. `signature_expired` answers a signature
  outside its window on either side: expired, or created more than the hub's
  skew allowance in the future. `key_revoked` is given only to a request whose
  signature verifies under the revoked key; without it the hub answers
  `key_unknown`. The seven others are faults, on which an
  edge MAY keep its released set within the maximum age of §Edge side; the
  edge signs again once and keeps it on a second fault or on a fault with too little budget left for a retry. Any `401` code outside
  those seven is a ruling at the edge (since 22 September 2026).
- The hub checks the window before it looks up the key, so a revoked key
  whose edge's clock is outside the window is answered `signature_expired`,
  a fault, never `key_revoked`. Such an edge keeps its set to the kept-release
  bound while the hub answers. The hub MUST look up the key before it judges
  the window, and answer `key_revoked` to a signature that verifies under a
  revoked key whatever its window (planned).

## Edge side

**Status, 19 September 2026: fixture-tested.** The requirements of this
section are built: the store (`ReleasedStore`,
`crates/commonmeasure-supply/src/credentials.rs`), the release client
(`crates/commonmeasure-relay/src/supplier_credentials.rs`), the hosted
service's fetch at start and on its interval
(`crates/commonmeasure-cli/src/hosted.rs`) and the `hub` evidence member
(`crates/commonmeasure-harness/src/mcp.rs`). They are tested against a test
double of the release route, written from §Release
(`crates/commonmeasure-relay/tests/supplier_credentials.rs` and the custody
test in `crates/commonmeasure-cli/tests/hosted_service.rs`). Both doubles refuse a release
signature that does not cover `@method` and `@path`, and a window over 360
seconds. A hub has
served releases to this code once, on loopback: the ignored test in
`crates/commonmeasure-cli/tests/supplier_custody_e2e.rs` passed on
19 September 2026 against the real hub server, at its revision of that day,
and a PostgreSQL database made for the run (one run, no transcript kept). The
hub accepted the service's signed release request, and authorisation, rotation
(as `rotated_at`), withdrawal and revocation each changed the service's fetch
record at its next fetch with no restart. At the run's interval of 2 s each
change arrived between 1.9 s and 2.1 s after the owner's act; the test fails
past one interval plus the request budget, 32 s. A connection is listed as
held only when a non-empty value was served for it. The run does not compare
that value with the owner's, shows no value reaching an adapter (the offline
tests do, against the double), opens no hosted session and meets no
acceptance case, so this section stays `fixture-tested`. No test calls a
supplier. Five points of the build add detail to the
requirements below:

- The service fetches only where `hosted-service.json` sets
  `"supplier_custody": true`. It defaults to unset, and an unset service
  makes no release request and reads credentials as before.
- An answer that is neither a `200` release document addressed to this
  edge's key and organisation nor a `401` that rules on the key (a fault
  `401` answered twice or too late to retry, a `400`, a `403`, a `404`, a `429`, a
  `5xx`, a malformed `200`) is recorded `unreachable` with its status and the
  hub's stable reason, and keeps the last released set. A transport failure
  is recorded `unreachable` with one fixed sentence; the transport's own
  message is never recorded, because it can quote response bytes. A served
  entry is held only for a remote provider's own variable, one connection
  per provider; any other is recorded as not held. Each fetch is recorded by
  name in `<home>/supplier-credentials.json`.
- A kept set is used for at most three intervals after the last `accepted`
  or `unchanged` fetch (900 s at the default), as the third bullet below
  requires. The fetch record, the journal and the `credentials_loaded`
  record (`hub_expired`) name what is no longer used. The owner kept the
  multiple on 22 September 2026 and added the hub's bound (owner decision 7,
  §Validity of a release, not yet built).
- The fetch runs at a fixed rate in a thread of its own, beside the loop
  that refreshes policy and keys and not inside it: each fetch starts one
  interval after the last one started. Inside that loop a fetch would wait
  for the policy refresh and the relay, and the bound under Revocation and
  rotation (the interval plus one request budget) would not hold. The bound
  as built excludes name resolution, which the 30 s request budget does not
  cover, and assumes an interval of at least the budget: a fetch that
  overruns a shorter interval is followed at once, and the bound is then
  two budgets.
- The run-output credential provenance is unchanged. A run is not made by a
  hosted service, so no run holds a released credential. In the session
  record a released credential the environment shadows is listed under
  `hub_shadowed`, and `shadowed` stays the file's list of variable names
  (`docs/contracts/session-evidence.md` §Credential provenance).

Without custody configured, the hosted edge applies `credentials.env` into
the process environment once at start, before any thread exists, and
`supplier_from_environment` reads that environment at each adapter
construction (`run` in `crates/commonmeasure-cli/src/hosted.rs`, `apply` in
`crates/commonmeasure-supply/src/credentials.rs`, `supplier_from_environment`
in `crates/commonmeasure-supply/src/lib.rs`). Rotation there is a restart.
Setting the environment from a running thread is unsound, so reload cannot
reuse `apply`.

- The edge MUST hold hub-released values in an in-process credential store
  read at adapter construction, with the process environment and
  `credentials.env` as the other two sources. Precedence is fixed:
  environment, then `credentials.env`, then hub release. A locally supplied
  variable shadows the hub's, as the file's is shadowed today, so an operator's
  shell can never be overridden by the hub.
- A hosted service whose `hosted-service.json` sets `supplier_custody` MUST
  fetch once at start after `apply` and then at a fixed rate of one fetch
  every `interval_seconds` (default 300, `hosted.rs`; `policy-envelope.md`
  §Cadence and staleness), on a schedule that does not wait for the policy
  refresh or the relay. A hosted service that does not set it MUST NOT call
  the release route. A local edge does not fetch (owner decision 2).
- A fetch outcome is one of `accepted`, `unchanged`, `unreachable`, `refused`.
  On `refused` (a `401` whose `code` is not one of the seven faults of
  §Signature components of the release request, or that carries no `code`)
  and on an empty `200` the store is cleared at once. A `401` coded as a
  fault (exactly one of the seven strings) is a fault in one request: the
  edge signs again with a fresh nonce and sends once more within the same
  request budget, provided at least a quarter of that budget is left; a
  second fault, or a fault with less than a quarter left, is `unreachable`,
  recorded with its code, and the fetch record's `retried_after` names the
  first fault of a retried fetch.
  Any other answer (a `403`, a `404`, a `429`, a `5xx`, a `200` that is not a
  release document addressed to this edge's key and organisation) and any
  transport failure is `unreachable`: it is recorded with its status and the
  hub's stable reason, or with fixed text where there was no answer, and never
  with a transport's or a parser's message, which can quote response bytes. On
  `unreachable` the store keeps the last released set until the deadline of
  §Validity of a release: the earlier of the hub's `valid_for_seconds` and
  three intervals from the last `accepted` or `unchanged` fetch. The age is checked
  where a value is read, so a fetch that stops running for any reason expires
  the set as an unreachable hub does. Hub absence therefore never widens
  access beyond what the last fetch granted. The revocation bound is
  `interval_seconds` plus the request budget while the hub answers with a
  release or a ruling, and the kept-release bound (at most 900 seconds) plus
  the request budget otherwise, a fault `401` included; name resolution is
  outside the request budget; acceptance records state both.
- Evidence records names only. The `credentials_loaded` record
  (`docs/contracts/session-evidence.md` §Credential provenance) gains a
  `hub` member listing `{connection_id, provider, variable, fetched_at}` per
  held credential, beside `applied` and `shadowed`. A variable the hub
  supplies but the environment or file already holds is listed under
  `hub_shadowed` as `{connection_id, provider, variable}`; `shadowed` stays a
  list of variable names. A credential past its maximum age is listed under
  `hub_expired` with its `fetched_at` and `max_age_seconds`. The run-output
  credential provenance carries the same shape. A value MUST NOT enter any
  record, log line, `context_status` answer or error.
- The existing `unavailable: <provider> needs <VARIABLE>` message
  (`unavailable_credential` in `crates/commonmeasure-harness/src/mcp.rs`)
  stays the message for a
  provider no source supplies; it gains no mention of the hub, because the
  agent needs no hint that a key exists elsewhere.

## Revocation and rotation

- **Hub revocation or withdrawal is edge-side expiry within one refresh
  window.** After the owner acts, the edge stops using the value at its next
  successful fetch: at most `interval_seconds` plus the request budget on the
  hosted service while the hub answers its fetches with a release or a
  ruling, and at most the kept-release bound plus the request budget where it
  does not, a fault `401` included (§Validity of a release). An edge whose
  clock is outside the hub's window is the case to name: until the hub looks
  up the key first, its revoked key is answered as a fault. The
  supplier's key is unchanged and still works for anyone
  holding it. The acceptance record MUST state this bound and MUST NOT
  describe it as revocation at the supplier.
- **True revocation for Exa, Ozone Live and Tavily is rotation of the upstream key**,
  which revokes every edge using that key, followed by a Hub rotate. This
  custody path releases the supplier account key; it does not mint scoped
  sub-credentials.
- **Per-edge true revocation needs a managed connector** making the upstream
  request, which is a CM content-processing service with its own operator,
  isolation, disclosure and retention boundary. It is out of scope here.
- Hub rotation reaches the edge on the same window. A rotation MUST NOT
  require a process restart once the store above exists; that is an
  acceptance case.
- Each acceptance record and the Hub screen MUST name which of the three the
  organisation has: edge-side expiry, upstream rotation, or connector.

## Usage limits

Bounding is the edge's existing per-principal `allowances` (day or month,
`docs/contracts/source-policy.md` §Scopes and principals) on the Word scope's
policy revision; this slice adds no Hub-side limit.

- Per-edge allowance storage cannot enforce a cross-edge limit
  (`docs/contracts/instance-registration.md` §Accepted binding, shared
  limits). A connection authorised to two edges has two independent
  allowances. A Hub-side shared reservation is deferred to registration's
  owner choice 4.
- Exa receipts carry an observed charge; Tavily's search charge is quoted from
  the published price and its extract cost is unknown
  because its responses carry no cost field. An unpriced dispatch can exceed an
  allowance by one dispatch's cost (`docs/FAIL-POLICY.md` §7. Unknown is never zero).
  Allowance evidence MUST be labelled `observed` for Exa and `quote-bound` for
  Tavily, and `unknown` for Ozone Live, whose response reports no charge and
  supplies no price to quote. The Hub screen MUST show the evidence beside
  the provider. Unknown cost MUST NOT be recorded as zero or described as free.

## Audit

Every action below is one `audit_log` row, written in the same transaction as
the mutation, failing the request if the insert fails. Detail carries ids, provider and
label, never a value or its digest.

| Action | Actor | Detail |
|---|---|---|
| `supplier_connection.create` | owner | connection id, provider, label |
| `supplier_connection.rotate` | owner | connection id |
| `supplier_connection.revoke` | owner | connection id |
| `supplier_connection.authorise` | owner | connection id, key id |
| `supplier_connection.withdraw` | the person whose act caused it (withdrawal, connection revocation, edge-key revocation, member removal, closure), or the edge on its own disconnect | connection id, key id, cause |
| `supplier_credential.release` | edge | key id, connection ids served |

Release is an act of an edge, not a person. The hub's audit log carries an
actor kind (`person`, `edge`, or `system` for the hub's own acts) and an
`actor_key_id`, added by 19 September 2026 because this slice required them; the release row MUST NOT be written under a member's
id.

## Security requirements

- Ciphertext at rest under `API_KEY_ENCRYPTION_KEY`; no reveal route.
- Tenant isolation: administration and release are scoped by row-level
  policy through `edge_keys` as the directory-proof table is; a second
  organisation's session and enrolled key are refused in test.
- Rotation preserves id and authorisations; revocation drops ciphertext and
  cannot be undone.
- Release requires a stored-nonce signature, serves `no-store`, and is
  rate-limited per key like every enrolled-edge route.
- No value in records, logs, errors, telemetry, envelopes or browser after
  submission. The product's sweep helper
  (`crates/commonmeasure-cli/tests/credential_sweep/mod.rs`) runs over the
  hosted edge's session records, service stderr and `context_status` output
  with a hub-released key configured.
- Edge revocation withdraws authorisations; organisation closure revokes
  connections and drops ciphertext with the closure.

## Acceptance evidence

No acceptance case is met. As of 19 September 2026, hub cases 3 (hub
side), 6 and the hub part of 8 are `fixture-tested`; 2 on the hub side only.
An edge has fetched releases from a hub once, on loopback, in an ignored test
that meets no case (§Edge side). The edge's fixture tests, against a
test double of the release route, are under §Edge side. Every case stays
**planned** as an end-to-end record. Each is recorded with the edge, key id,
connection id, policy revision and window in force.

1. A real `context_search` from native Word through the hosted edge using an
   Exa key released by the hub, with an acquisition record carrying an
   observed charge and a `credentials_loaded` record naming the connection.
2. A second enrolled edge of the same organisation receives an empty list
   until authorised, then the credential, then an empty list after
   withdrawal.
3. An enrolled edge of another organisation receives an empty list (`200`),
   an unenrolled key `401`, and neither ever a connection id of the first.
4. Revocation at the hub, then refusal at the Word edge as
   `unavailable: exa needs EXA_API_KEY` within one refresh window, with
   `interval_seconds` stated.
5. Rotation at the hub, then a successful search under the new value with
   no restart, and the old value refused by the supplier.
6. The cross-tenant tests above run against Postgres beside
   `tests/tenancy.rs`.
7. The credential sweep over records, stderr and `context_status` finds
   nothing.
8. The audit rows for every action, the release row under an edge actor.

Evidence labels: `fixture-tested` for the Hub and edge tests;
`live-verified` only for case 1 through the deployed Word edge. Case 1
establishes search on this path and nothing about licensed access.

## Owner decisions

Decided by the owner on 18 September 2026.

1. **Refresh interval.** 300 seconds, the hosted service's default. The
   revocation bound is therefore 300 seconds plus the request budget, and
   acceptance records state it.
2. **Local edges.** Hosted edges only. A local edge keeps `credentials.env`
   and does not call the release route. The restriction is edge-side until
   implementation establishes whether the hub can tell a hosted edge's key
   from a local one at authorisation; if it can, authorisation refuses a
   local edge's key.
3. **In-flight protection.** TLS only. The value is not sealed to the edge's
   key in this slice.

Decided by the owner on 21 September 2026.

5. **Impersonation.** The hub refuses to authorise an edge for a supplier
   connection, and to mint an enrolment token, while a super admin acts as
   another person. The hub still cannot tell a hosted edge's key from a local
   one, so an owner, or a stolen owner session, can enrol a key, authorise it
   and obtain the value through a release; decision 2 stays edge-side and the
   audit record is the control for that path. Built in the hub on
   21 September 2026. The hub also refuses to make or accept an invitation
   under impersonation, for either role, so a super admin acting as an owner
   cannot invite an address they hold and walk the owner path as that person.
   The owner confirmed the refusal for the member role on 22 September 2026.

Decided by the owner on 22 September 2026.

6. **Create and rotate under impersonation.** Both refused, by the same check
   and answer as `authorise`, audited under the real operator. Each outlives
   the session: an operator's own supplier key, stored by rotation or in a
   new connection a real owner later authorises, would carry the
   organisation's supplier traffic on the operator's account. Withdrawal and
   revocation stay allowed, so support can still stop a leaked key. Built in
   the hub on 22 September 2026, with discovery and an import carrying a
   destination refused as well.
7. **Kept-release bound.** The release answer carries `valid_for_seconds`,
   900 by default, a duration the edge measures on its monotonic clock from
   receipt, so clock skew between the two cannot stretch or cut it. The edge
   keeps a released set for the earlier of that and three of its intervals
   after the last good fetch. A service with custody on refuses at start an
   `interval_seconds` above 300. Planned; the requirements are §Validity of
   a release.

9. **Act-as is read-only apart from the stop actions.** Under `act` the hub
   refuses every mutating request by default, in its middleware, with the
   answer and audit row of decision 5, except an allowlist of actions that
   stop what leaked: revoking an API key or an edge key, withdrawing an
   edge's authorisation, revoking a supplier connection, clearing an onward
   destination, and ending the impersonation. A test over the router's route
   table fails any mutating route that is neither refused nor allowlisted.
   The per-route refusals of decisions 5 and 6 stay as a second layer.
   Setup for an organisation is a super admin's own session, never
   impersonation. Planned.
