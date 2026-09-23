---
title: Grant contract
---

# Grant contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

**Verification state: planned.** This contract defines the grant object and
nothing implements it: no hub catalogue authors a grant, no envelope carries
one, no edge evaluates one and the hub answers `503 entitlement_authority` to a
registration that names one. The three documents under
[`grant-examples/`](grant-examples/self-issued.json) are pinned inputs for the
implementation's tests; they are not evidence that anything runs.

Any content owner can be a supplier on the network, the organisation itself
included: its knowledge base, memory or retrieval service and its published
pages are collections, and the supplying system keeps ingestion, indexing,
retrieval and access permissions. Grant, grantee scope, issuer, supplier,
receiver, collection and entitlement are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md) §The network.

## What one object covers

| Case | Issuer | Grantee | Who states the grant |
|---|---|---|---|
| Self-issued | The organisation | The same organisation | An owner, in the organisation's Hub catalogue |
| Licensing out | The organisation | Another organisation | An owner of the issuing organisation (a later stage) |
| Licensing in: collective | A collective | The organisation | Hub derives it from the coverage the collective serves for the organisation's agreement |
| Licensing in: direct | A publisher | The organisation | An owner transcribes the agreement, or the publisher signs the grant |

A grant states authority. It carries no content, credential or token, and it
does not choose the delivery route: content arrives through a supply adapter
([`provider.md`](provider.md)), and a credential a route needs is released
under [`supplier-credentials.md`](supplier-credentials.md). Registration binds
a grant to an instance
([`instance-registration.md`](instance-registration.md#entitlement-binding));
held unbound, a grant authorises nothing.

The experiment runtime already has a narrower entitlement: a suite's
governance declares ordered tiers and the tier the run holds
(`granted_entitlement` in `crates/commonmeasure-runtime/src/governance.rs`,
sealed in the run manifest, [`run-output.md`](run-output.md)), and a source
declaring a higher tier is refused at admission. With the internal adapter it
is the starting point for the first internal route. It has no issuer, grantee,
validity or binding, and this contract leaves it as it is. How a tier and a
grant combine on the internal route is settled with the adapter's grant gate
(§What a later increment must add).

## The document

```json
{
  "schema": "commonmeasure-grant/v1",
  "grant": {
    "id": "grant-precedents-1",
    "revision": 1,
    "issuer": {"kind": "organisation", "id": "org-example-firm", "name": "Example Firm LLP"},
    "collection": {
      "id": "example-firm-precedents",
      "name": "Precedent bank",
      "supplier": {"shape": "connector", "adapter": "internal", "credential": "none"},
      "resources": [{"kind": "adapter_collection", "value": "precedents"}],
      "coverage": null
    },
    "grantee": {
      "organisation": "org-example-firm",
      "principals": [{"scheme": "hub-member", "value": "member-ana"}],
      "engagements": ["matter-2291"],
      "instance_classes": []
    },
    "uses": {"permits": ["ai-input", "search"], "prohibits": ["ai-train"]},
    "conditions": [],
    "validity": {
      "not_before": "2026-10-01T00:00:00Z",
      "not_after": "2027-01-01T00:00:00Z",
      "revocation": {"check": "distribution", "max_age_seconds": null}
    },
    "price": {"type": "free", "amount_minor": null, "currency": null, "per": null,
              "budget": {"unit": "acquisition", "ceiling": 500, "period": "month"}},
    "reporting": {"duty": "private_record_only", "form": null, "destination": null,
                  "event_classes": [], "period": null, "deadline_days": null},
    "terms": {"references": []},
    "issuer_fields": {"kb.example/clearance": "partner-review",
                      "kb.example/practice_area": ["employment"]}
  },
  "digest": "sha256:18515fc948885f9716dd7a90225462f7ffd0738978f0ecf63e661ff169127aa0",
  "state": "current",
  "basis": {"kind": "self_issued", "evidence": [], "signature": null}
}
```

This is the document pinned in
[`grant-examples/self-issued.json`](grant-examples/self-issued.json). `digest`
is `sha256:` and the hex SHA-256 of the canonical JSON
([`canonical-json.md`](canonical-json.md)) of `grant`. Every member shown is
required; a member with nothing to say is `null` or empty as shown, so an
omission is never read as a default. An unknown member anywhere outside
`issuer_fields` makes the document invalid; a condition and an evidence entry
have the fixed members their rows give. Numbers are integers; money is in
minor units. Times are RFC 3339 in UTC.

One `id` and `revision` name one `grant` value. A changed grant is a higher
revision under the same `id`. `issuer.id` and `id` are each 1 to 128
characters from `A` to `Z`, `a` to `z`, `0` to `9`, `.`, `_` and `-`, so the
reference `grant:<issuer id>/<grant id>` splits at its one `/`. A collection
id is not part of a reference and may contain `/`.

`state`, `basis` and the evidence in it sit outside `grant`, so they change
without moving the digest or the revision: withdrawal sets `state`, and a
refreshed coverage snapshot replaces its evidence entry.

### Members

"Edge, offline" is what the edge does with no hub or issuer reachable, from
the envelope and the accepted binding it already holds. Where a cell says
*invalid*, the whole document authorises nothing and the reason
`grant_invalid` names the member. An `unavailable` outcome's reason is its
gap: the name of the dependency that was missing
([`docs/FAIL-POLICY.md`](../FAIL-POLICY.md) §5).

"Set by" names whose decision a member records. Who writes the value into the
document depends on `basis.kind` (§What signs a grant). Where a row says
*Issuer*:

- in a `self_issued` grant an owner of the issuing organisation writes it;
- in an `issuer_signed` grant the issuer writes it and signs it;
- in a `derived` grant the hub writes it from what the issuer served, and in
  a `transcribed` grant an owner writes it from the agreement. The value is
  then the organisation's reading of the issuer's decision, and
  `basis.evidence` cites the served value or the clause. The organisation
  does not choose it. Where nothing served or agreed states a member, the
  value entered is the one that permits least, such as `online` revocation,
  and an unstated price is `null`.

| Member | Set by | Verified by | Edge, offline |
|---|---|---|---|
| `schema`, `digest` | Whoever states the grant (§What signs a grant) | Hub on entry to the catalogue; edge on activation | Unknown schema or a digest that does not match: invalid. |
| `state` | Hub. `current`, or `withdrawn` once the issuer or an owner withdraws the grant (§Distribution). | Hub: a withdrawn grant never returns to `current`; a grant stated again is a higher revision. | `withdrawn`: `grant_withdrawn`. Absent or an unknown value: invalid. |
| `id`, `revision` | Issuer in an `issuer_signed` grant; otherwise the hub assigns both. `id` is unique within the issuer; `revision` is an ordered unsigned integer. | Hub refuses a lower revision, or the same revision with another digest. | Compared with the binding's reference (§Evaluation). A revision other than the bound one is `grant_not_bound`. |
| `issuer` | Issuer. `kind` is `organisation`, `collective`, `publisher` or `delegate`; `id` is the Hub organisation id for an organisation on this hub, otherwise the issuer's own stable identifier, a domain name where it has one. A `delegate` names its principal in `basis.evidence`. | Hub, by the means `basis.kind` allows. The edge does not verify issuer authority. | Recorded. Absent or an unknown `kind`: invalid. |
| `collection.id`, `collection.name` | Issuer | Hub checks `id` is unique within the issuer. | Recorded. |
| `collection.supplier` | Issuer, with the grantee for a connector route. `shape` is `connector` or `native`; `adapter` names the supply adapter for a connector (`fetch` for mediated web access); `credential` is `none`, `custody` (a supplier connection released under the custody contract) or `issuer_token` (a per-instance token from the issuer's licensing server). | Hub checks the adapter name exists and, for `custody`, that the organisation holds a connection for it. | An acquisition by another adapter is `wrong_route`. `native` and `issuer_token` are not evaluated at the edge in this version: `unavailable`, gap `issuer_validation`. |
| `collection.resources` | Issuer. Non-empty list of selectors: `adapter_collection` (the collection key the named adapter exposes, matched exactly), `host` (exact host, as a source-policy host is compared) or `url_prefix` (an absolute `https` URL ending in `/`; the requested URL must begin with it). | Hub, against `coverage` where the grant is derived. | The requested resource, and each redirect destination, must match a selector. An unknown selector kind matches nothing. |
| `collection.coverage` | Hub, for a derived grant: `{reference, max_age_seconds}`, the coverage snapshot the selectors were derived from and the age the issuer allows it, as served or agreed. `null` otherwise. The snapshot's digest, retrieval time and `as_of` time are the `coverage` entry of `basis.evidence` with the same `reference`, outside the digest. | Hub re-derives on its own schedule. Unchanged selectors keep the revision, and the refreshed evidence entry travels in a same-revision renewal of the envelope (§Distribution). Changed selectors are a new revision. The hub refuses a registration or renewal that names the grant while the snapshot is past its age. It does not shorten an instance's expiry by the snapshot: the edge's own check stops work at that moment, and a refresh resumes it with no new registration. | The edge compares `as_of` plus `max_age_seconds` with its clock. At or past it: `unavailable`, gap `coverage_stale`. No `coverage` evidence entry with this `reference`, or one with a `null` `as_of`: invalid. |
| `grantee.organisation` | Issuer. Exactly one. | Hub: the catalogue and registration resolve the organisation from authenticated authority, never from the request. | Must equal the organisation of the envelope in force, else `wrong_organisation`. |
| `grantee.principals` | Issuer. A list of `{scheme, value}`; schemes are `hub-member`, `oidc` (`value` is `<issuer>#<sub>`) and `email`. Empty means any named principal of the organisation. | Hub at registration, against the delegation's operator. | Compared with the operator identifiers in the accepted binding. A binding naming no operator: `principal_unresolved`, even when the list is empty. No match: `wrong_principal`. An unknown scheme matches nothing. |
| `grantee.engagements` | Issuer. Engagement names as the organisation's source policy uses them. Empty means any. | Nobody at the hub in this version: registration has no engagement work reference. | Compared with the governing engagement source policy resolves for the crossing ([`source-policy.md`](source-policy.md) §Scopes and principals). Non-empty with no governing engagement: `engagement_unresolved`. No match: `wrong_engagement`. |
| `grantee.instance_classes` | Reserved. No vocabulary is defined (§Open questions 4). | Hub refuses a non-empty list at entry. | Non-empty: `instance_class_unsupported`. A narrowing the edge cannot evaluate is never treated as met. |
| `uses` | Issuer. `permits` and `prohibits` are lists of RSL usage tokens (RSL 1.0 §3.4.1.1): `all`, `ai-all`, `ai-train`, `ai-input`, `ai-index`, `search`. | Hub checks the tokens. | The use the crossing makes (a mediated fetch makes `ai-input`, a mediated search `search`) must be covered by `permits` and by no entry of `prohibits`; a prohibition wins, as it does when the runtime reads an RSL licence. Otherwise `use_not_permitted`. An unknown token covers nothing. |
| `conditions` | Issuer. Each is `{kind, enforced_by, effect, values}` and has no other member. `enforced_by` is `edge`, `grantee` or `supplier`. Kinds: `rsl_user` (RSL's `user` restriction; `effect` is `permits` or `prohibits`, `values` are RSL user types such as `commercial`, `non-commercial`, `education`, `government`, `personal`), `rsl_geo` (RSL's `geo` restriction; the same `effect`, `values` are ISO 3166-1 alpha-2 country codes) and `attribution` (the issuer requires credit; `effect` is `null` and `values` empty). | Hub refuses an unknown kind, or members that do not fit the kind, at entry. Nobody verifies a `grantee` or `supplier` condition; the grantee accepts it with the grant. | An `edge` condition of a kind this edge does not implement: `condition_unknown`. This version implements none. `grantee` and `supplier` conditions, of a known kind or not, are recorded with the admission as not enforced by the edge, and never as satisfied. |
| `validity.not_before`, `validity.not_after` | Issuer. `not_after` is after `not_before`. | Hub at entry, registration and renewal; an instance's expiry is no later than `not_after`. | Against the edge clock with no tolerance: before `not_before` is `grant_not_yet_valid`; at or after `not_after` is `grant_expired`. A clock the edge cannot trust makes authority `unavailable`, gap `clock`. |
| `validity.revocation` | Issuer. `check` is `distribution` (withdrawal reaches the edge in the next envelope) or `online` (the issuer requires a validation call). `max_age_seconds` bounds how old the carrying envelope's `issued_at` may be; `null` leaves the envelope's own expiry as the bound, which only a `self_issued` grant may do. Where the issuer is not the grantee organisation, `distribution` is offline use the issuer must have permitted (§Offline use of an external grant): `max_age_seconds` is the bound the issuer agreed, never a figure the organisation or the hub picks. | Hub refuses entry of an external `distribution` grant with no bound or no evidence of it. It marks a withdrawn grant and publishes that envelope at once. | `distribution`: §Distribution decides. `online`: `unavailable`, gap `issuer_validation`; no cached answer stands in for a required check. An external `distribution` grant with a `null` bound or without the evidence is treated as `online`. |
| `price` | Issuer. `type` is an RSL payment type (`free`, `use`, `subscription`, `purchase`, `crawl`, `training`, `contribution`, `attribution`). `amount_minor`, `currency` and `per` state a unit price where the grant states one. | Hub records it. Settlement is external. | Recorded with each admission. A `null` amount is an amount the grant does not state; it is never recorded as zero. Only `free` means no charge. |
| `price.budget` | Issuer: the most the grantee may take under the grant, as `unit` (`acquisition` or `currency_minor`), `ceiling` and `period`. `null` means the issuer sets none. It is part of the value an issuer signs, so a grantee never writes it. The grantee's own lower budget is the ceiling of the hub limit account and is not in the document. In a `self_issued` grant the two are the same party's and the document carries the one figure. | Hub: the budget is a shared limit account whose authority reference is the grant (§Binding) and whose ceiling is no higher than this one. | The edge counts admitted acquisitions against the reservation in the accepted binding. An acquisition that does not fit what remains: `budget_exhausted`. A budget with no reservation in the binding: `unavailable`, gap `shared_limit_reservation`. |
| `reporting` | Issuer. `duty` is `private_record_only` or `required`. For `required`: `form` (`content_telemetry` or `issuer_batch`), `destination` (`{kind, reference}` with `kind` `receiver` for a Content Telemetry receiver URL, or `issuer_api` for the issuer's own reporting interface), `event_classes`, `period` and `deadline_days`. | Hub: a self-issued grant must be `private_record_only`; a `required` duty must map to a duty the hub can persist (§Binding). | `private_record_only`: records of the acquisition stay in the private record and are never cleared for egress on the grant's account. `required`: the binding must carry the duty, else `reporting_duty_unbound`; where source policy forbids the egress the duty needs, `reporting_conflict`, before any content moves. |
| `terms.references` | Issuer. Each is `{role, reference, version, digest, location}` with `role` `licence`, `agreement` or `terms`. Non-empty unless `basis.kind` is `self_issued`. The authoritative text stays where `location` says; a digest is given where the hub holds the document. | Hub checks a digest against a document it holds. It does not read the terms. | Recorded with each admission. Admission is decided from the grant's members; the edge does not read the terms. |
| `issuer_fields` | Issuer. Any JSON object. | Nobody. | Carried to the supplying system's adapter and recorded, byte for byte. Common Measure never branches on it. |
| `basis` | Whoever states the grant (§What signs a grant). Each evidence entry is `{role, reference, digest, retrieved_at, as_of}` and has no other member; `role` is `issuer_key`, `delegation`, `agreement`, `coverage` or `offline_bound`, and `as_of` is `null` except on a `coverage` entry. | Hub. It checks a digest against a document it holds and records the rest without verification. | Recorded. Unknown `kind` or evidence `role`: invalid. The `coverage` and `offline_bound` entries are read as their rows say. |

## What signs a grant

Two signatures are involved and they establish different things.

**Distribution.** The hub's policy signer signs the envelope that carries the
grant (§Distribution). This is the only signature the edge verifies. It
establishes that the organisation's hub published this grant document to this
organisation's edges. It does not establish the issuer's authority.

**Statement.** `basis.kind` says how the grant came to exist and what stands
behind it:

| `basis.kind` | Meaning | `evidence` | `signature` |
|---|---|---|---|
| `self_issued` | An owner of the issuing organisation authored it in the catalogue. Issuer and grantee organisation are equal. | Empty; the hub's audit trail holds the authoring act. | `null` |
| `issuer_signed` | The issuer signed `grant`. | An `issuer_key` entry: the issuer's key reference. | `{algorithm: "ed25519", key_id, value}` over the canonical JSON of `grant` |
| `derived` | The hub derived it from something the issuer served for the organisation's agreement, such as a coverage snapshot. | A `coverage` entry for the snapshot, with its digest, retrieval time and `as_of`; an `agreement` or `offline_bound` entry for each member taken from the agreement. | `null` |
| `transcribed` | An owner entered it from an agreement the organisation holds. | `agreement` entries: reference and digest of the agreement and the clauses relied on, and an `offline_bound` entry where the grant is distributed. The references are recorded without verification. | `null` |

A `derived` or `transcribed` grant is the organisation's statement of what it
believes it holds. It is never presented as the issuer's assertion, to a
supplier or in a report. An `issuer_signed` grant is verified by the hub under
an issuer key the owner pinned; discovery and trust of issuer keys between
registration services belong to a later stage. A native supplier verifies the
entitlement with the grant's issuer, not with this document.

### Offline use of an external grant

A grant is external when `issuer.id` is not `grantee.organisation`. Carrying
an external grant in the envelope and authorising from it with no call to the
issuer is offline use of the issuer's authority, which
[instance registration](instance-registration.md#expiry-revocation-and-shared-limits)
allows only within a bound the issuer agreed. So an external grant has
`validity.revocation.check: distribution` only when both hold:

- `max_age_seconds` is set, to the bound the issuer agreed;
- the issuer's agreement to it is in the document: the issuer's signature
  over `grant`, which contains the bound, for an `issuer_signed` grant;
  otherwise an `offline_bound` entry in `basis.evidence` naming the clause or
  the served value that permits offline use for that long.

The hub refuses entry of an external `distribution` grant that lacks either,
and whoever states the grant enters `online` where the issuer agreed no
offline use. The
edge makes the same test on the document it holds and treats a grant that
fails it as `online`: `unavailable`, gap `issuer_validation`. For an
`issuer_signed` grant the edge tests that `basis.signature` is present; it
verifies no issuer signature and relies on the hub's check. This contract
states the rule and no figure; the figures in the examples are synthetic
(§Open questions 8).

A grant one organisation issues to another (a later stage) is external too;
how the issuing organisation's agreement to a bound is shown on a shared hub
belongs to that stage.

How long a withdrawal can take to stop work at an edge that cannot reach the
hub is the least of three bounds: `max_age_seconds` from the envelope's
`issued_at`, the envelope's expiry, and the instance's expiry (24 hours at hub
`77875ed`), because a renewal must present the current envelope and that
envelope marks the grant withdrawn. An edge that can reach the hub stops at
its next synchronisation.

## Distribution

A grant whose `validity.revocation.check` is `distribution` travels in the
signed policy envelope ([`policy-envelope.md`](policy-envelope.md)). That
contract describes what is built, and until the members below are built no
envelope carries a grant. Its §Activation and the last-known-good said "This
envelope does not grant or renew supplier entitlements"; this section
supersedes that sentence, and the envelope contract now carries a paragraph
labelled planned in its place. Its §The envelope says "one revision names one
policy"; with this addition a revision names one policy and one grant set,
the opposite choice to its §Directory reporting grants, which keeps a separate
revision space (§Open questions 2). The addition this contract needs:

- `payload.grants`: the catalogue's grant documents whose
  `grantee.organisation` is the envelope's organisation, each at its latest
  revision, sorted by `grant.issuer.id` then `grant.id`. A grant the
  organisation issues to another organisation is not among them, so rule 3
  (§Evaluation) never closes an issuer's own collection to itself.
- A grant stays carried after it expires and after it is withdrawn
  (`state: withdrawn`). It authorises nothing, and it keeps its resources
  closed to ordinary access under rule 3 with the reason recorded. It leaves
  the envelope when an owner removes it from the catalogue, an audited act
  that returns its resources to source policy alone. A superseded revision is
  not carried.
- `payload.grants_digest`: the digest of the canonical JSON of the list of
  `{"digest", "state"}`, one per document in `payload.grants` order.
  `payload.digest` remains the digest of `payload.policy`.
- A grant added, revised, withdrawn or removed changes `grants_digest` and is
  a new envelope revision, which the hub publishes at once. The reuse check
  extends to it: the applied revision offered with another `grants_digest` is
  refused.
- A change to `basis` alone, such as a refreshed coverage snapshot, leaves
  `grants_digest` and the revision unchanged. It reaches the edge in a
  same-revision renewal, which already replaces the envelope kept as
  last-known-good. An accepted binding names the grant's digest, so it stays
  good across a refresh.
- The envelope signature already covers `payload`, so it covers the grants.
  An edge built before this version ignores both members and enforces no
  grant. It names no entitlement when it registers, so its instances hold
  none. The hub treats a collection as grant-enforced only on an edge whose
  fleet-status document reports the applied `grants_digest`.

Policy and grants age differently inside one envelope. A policy restricts, so
an applied policy stays in force after its envelope expires. A grant permits,
so it stops:

- A grant authorises only while the envelope in force is unexpired and, where
  the grant sets `max_age_seconds`, only while the envelope's `issued_at` is
  no older than that. Outside either bound the outcome is `unavailable`, gap
  `grant_distribution_stale`. The policy keeps governing.
- A same-revision renewal of the envelope renews its grants with it.
- A failed synchronisation, a rejected envelope or a missing `grants` member
  never extends a grant and never empties the set before its bound.
- A grant document that is invalid authorises nothing and is recorded with its
  reason. It does not reject the envelope, because one defective permission
  must not hold back the restrictions the policy carries.

Every managed edge of the organisation receives every grant, including prices
and named principals. The hub already binds each served envelope to the edge's
key, but it serves every edge the same grant set; confining a grant to named
edges needs per-edge grant sets, a hub capability listed under later
increments.

## Evaluation

The edge answers one question for one acquisition: does a grant bound to this
instance authorise this resource, for this use, now? Inputs are the edge
clock, the envelope in force, the instance's accepted binding, the governing
engagement, the route (adapter), the resource, the use and the amount the
acquisition would consume.

An acquisition needs an authorising grant when any of these holds:

1. the route's adapter is grant-gated. The internal adapter is, on an edge
   that implements this contract;
2. the instance's binding has `requires_entitlements: true`;
3. the resource matches the collection of any grant in the envelope in
   force, bound to this instance or not. A grant is left out of this rule
   only while its `not_before` is later than both the edge clock and the
   `issued_at` of the envelope in force. A clock the edge cannot trust
   leaves no grant out.

Otherwise the grant layer is silent (`not_covered`) and source policy alone
rules, as today.

The requested resource is `{kind, value}`: `url`, an absolute URL, on the
fetch route, matched by `host` and `url_prefix` selectors; or
`adapter_collection`, the collection key, on a connector adapter.

Rule 3 keeps a grant's narrowing from being avoided by not using the grant.
Without it, a principal the grant does not name, work on another engagement,
or an instance whose budget is spent would reach the same content by ordinary
access. It holds for as long as the grant is carried, which includes an
expired or withdrawn grant until an owner removes it (§Distribution).

Its consequence is that taking a licence removes access that exists today.
A derived collective grant with `host` selectors makes every mediated fetch
to those hosts `refused`, reason `grant_not_bound`, for any registered
instance that did not name the grant and for every session with no
registered instance, which is most sessions today. Source policy cannot
reopen them: a crossing the grant layer refuses is refused. An organisation
should enter such a grant once registration is routine for the work that
reads those hosts. The narrower alternatives are §Open questions 9.

Candidates are the carried grants whose `collection.resources` match the
resource. With no candidate the outcome is `refused`, reason
`resource_not_granted`. Each candidate is checked in this order, stopping at
the first failure:

| Order | Check | Outcome and reason |
|---|---|---|
| 1 | Document valid | `refused` `grant_invalid` |
| 2 | Grantee organisation | `refused` `wrong_organisation` |
| 3 | State and validity window | `refused` `grant_withdrawn`, `grant_not_yet_valid`, `grant_expired`; `unavailable` `clock` where the state is `current` and the edge cannot trust its clock |
| 4 | Distribution bound | `unavailable` `grant_distribution_stale` |
| 5 | Coverage snapshot age, for a derived grant | `unavailable` `coverage_stale` |
| 6 | Revocation check is one this edge performs, and an external grant carries its agreed offline bound | `unavailable` `issuer_validation` |
| 7 | The accepted binding is verified, in force and names this `id`, `revision` and `digest` | `refused` `grant_not_bound` |
| 8 | Principal | `refused` `principal_unresolved`, `wrong_principal` |
| 9 | Engagement | `refused` `engagement_unresolved`, `wrong_engagement` |
| 10 | Instance class | `refused` `instance_class_unsupported` |
| 11 | Route | `refused` `wrong_route`; `unavailable` `issuer_validation` for a native or token route |
| 12 | Use | `refused` `use_not_permitted` |
| 13 | Edge conditions | `refused` `condition_unknown` |
| 14 | Reporting | `refused` `reporting_duty_unbound`, `reporting_conflict` |
| 15 | Budget | `refused` `budget_exhausted`; `unavailable` `shared_limit_reservation`, `price_unknown` |

The binding check compares the grant's identity only. The envelope's
revision may have moved since the binding was accepted, and a refreshed
`basis` does not disturb it. `requires_entitlements` is read from the
registration the accepted binding retains. `grant_not_yet_valid` is reached
under rules 1 and 2, or where the edge clock is behind the hub's: the hub
binds no grant before its `not_before`, and rule 3 ignores one.

The acquisition is `authorised` when one candidate passes every check. When
none does, the crossing is refused or unavailable and the record carries each
candidate's reason. A grant never overrides source policy: a crossing the
policy refuses is refused whatever a grant says, and for an internal
collection the supplying system's own refusal for the principal stands
whatever the grant says.

Each evaluation is recorded with the grant's `id`, `revision` and `digest`,
the outcome, the reason, the resource, the use, the amount and the instance
reference. The record members belong to
[`session-evidence.md`](session-evidence.md) and are added with the
implementation.

Budget is consumed only by an admitted acquisition, in the grant's unit: one
per acquisition, or `price.amount_minor` in `currency_minor`. A
`currency_minor` budget on a grant with a `null` amount cannot be counted and
is `unavailable`, gap `price_unknown`.

## Binding

[Instance registration §Entitlement binding](instance-registration.md#entitlement-binding)
is the authoritative text. In summary: a registration names grants in its
existing `entitlements` member as `grant:<issuer id>/<grant id>`; the hub
resolves each against the grant set of the envelope the registration already
presents; the accepted binding carries each grant's `id`, `revision` and
`digest`, its duty and its limit reservation, with the operator's identifiers
and the retained registration's `requires_entitlements`; and a grant's
`required` reporting duty is bound as the `owner:<content owner id>` duty the
hub already persists.

## Examples

Each file carries one grant document (`document`), the inputs of an
authorised acquisition (`defaults`: the clock, the envelope in force, the
instance's accepted binding with what it carries for the grant, the governing
engagement, the route, the resource, the use and the amount) and `cases`. A
case replaces the members it lists under `with`, and under `document_with`
the document's members outside `grant`, and states the `outcome` and `reason`
the edge must produce. Parties, identifiers, hosts and agreement
references are synthetic, and evidence digests are digests of placeholder
text. The grant digests are computed under
[`canonical-json.md`](canonical-json.md).

| File | Case | Reporting |
|---|---|---|
| [`grant-examples/self-issued.json`](grant-examples/self-issued.json) | An organisation grants its precedent bank, reached through the internal adapter, to one principal on one engagement. | Private record only |
| [`grant-examples/collective-coverage.json`](grant-examples/collective-coverage.json) | Hub derives a grant from a collective's coverage snapshot. Access is ordinary mediated fetch with no per-instance token. | Required, as the issuer's batch report to the issuer's interface |
| [`grant-examples/direct-agreement.json`](grant-examples/direct-agreement.json) | An owner transcribes a direct agreement with one publisher. | Required, as Content Telemetry to the publisher's receiver |

Every file has an `authorised` case and refusals for the wrong organisation,
engagement, resource, use, expiry and budget. The self-issued and direct
files have the `wrong_principal` refusal. The collective grant names no
principal, so no operator can be the wrong one, and its principal case is
`principal_unresolved`: a binding that names no operator. The self-issued
file adds work with no registered instance, an unresolved engagement and a
grant not yet valid. The collective file adds a stale distribution, a stale
coverage snapshot, a same-revision renewal whose refreshed snapshot leaves
the binding good, an unbound reporting duty, and rule 3 refusing a registered
instance that named no grant and a session with no instance. The direct file
adds a binding to a superseded revision, a withdrawn grant, a document with
no evidence of an agreed offline bound, a stale distribution, and a resource
outside the collection for an instance that does not require entitlements,
where the grant layer is silent.

The collective example's `issuer_batch` duty can be bound and its events
attributed, but no mapping from observed activity to the issuer's report
exists, so that duty cannot reach `delivered` until one is agreed.

## What a later increment must add

- The hub catalogue: authoring, revisions, withdrawal, removal, derivation
  from a coverage snapshot and its refresh, the entry checks of §Offline use
  of an external grant, and the audit of each.
- Per-edge grant sets, for an organisation that must confine a grant to named
  edges.
- Period rollover of a limit account, which the hub lists as planned; a
  budget with a `period` relies on it.
- `payload.grants` and `payload.grants_digest` in the envelope contract, the
  edge's activation of them and the applied `grants_digest` in the
  fleet-status document.
- Evaluation at the edge, its session-evidence members and the internal
  adapter's grant gate and principal pass-through, including how the gate
  combines with the governance entitlement tier.
- Resolution of `entitlements` at registration and renewal in place of
  `503 entitlement_authority`.
- An engagement work reference, so the hub can verify engagement narrowing.
- Coverage carried by reference for a repertoire too large to inline.
- The mapping from the private record to an `issuer_batch` report, agreed
  with the issuer.
- `online` revocation and the `native` shape through the Open License
  Protocol; issuer key discovery and trust (a later stage).
- Upstream terms on documents inside an internal collection. This version has
  no member for them; adding one is a schema revision.

## Open questions

Owner choices. Recommendations are not decisions.

1. **Should RSL carry grantee scope?** Options: (a) keep it outside RSL, as
   now; (b) propose it for RSL's licence vocabulary; (c) propose it as claims
   in the Open License Protocol's token exchange. Recommend (a) for this
   version and (c) if it is raised with the standard. An RSL licence is a
   public declaration attached to a resource and addressed to anyone; grantee
   scope is a private fact about one licensee that names its organisation,
   principals and engagements, and does not belong in a published file. The
   token exchange is already bilateral and per licensee.
2. **Envelope or a separate signed snapshot.** The adopted product direction
   distributes self-issued grants in the signed policy envelope. The alternative is the directory-grant precedent
   ([`directory-enrolment.md`](directory-enrolment.md#signed-grant-snapshot)):
   its own revision space and expiry. The envelope needs no change to the
   registration request, because registration already binds the envelope, but
   every grant edit becomes a policy revision that each edge must fetch before
   it next registers or renews. Recommend keeping the envelope and revisiting
   if catalogue churn makes policy revisions noisy.
3. **Principal identifiers.** Options: Hub member ids only, or `{scheme,
   value}` pairs including `email`. Recommend the pairs: a supplying system
   applies its own permissions to an identity it knows, which is rarely a Hub
   member id.
4. **Instance class.** The grantee scope may narrow to an instance class, and
   nothing defines one. Options:
   drop it, define it as the host name, or define it as labels an owner puts
   on a delegation. Recommend leaving it reserved until an organisation needs it, then
   delegation labels.
5. **May a self-issued grant name an external destination?** Recommend no.
   The private-record default stays fixed, and upstream terms are the one
   route by which activity on an internal collection is reported outward.
6. **Is a `derived` or `transcribed` grant enough for the collective and
   direct routes?** Options: require the issuer's signature, or accept the
   organisation's labelled statement with evidence digests. Recommend
   accepting the labelled statement for the first demonstration of each route
   and asking each counterparty whether it will sign a grant or serve a
   verifiable snapshot.
7. **Budget offline.** Options: spend only the instance's reservation offline,
   or require an online consume per acquisition. Recommend the reservation: it
   belongs to one root instance on one edge, so local counting cannot
   overspend the account.
8. **Revocation bounds.** A self-issued grant is bounded by the envelope's
   expiry. An external grant is distributed only within a bound its issuer
   agreed, with the evidence in the document (§Offline use of an external
   grant). The numbers are the owner's and each issuer's, and this contract
   sets none. The custody contract's 300 second refresh is the precedent for
   a hosted edge. The owner's part is what bound to ask each issuer for, and
   whether the hub applies a ceiling of the organisation's own to any agreed
   bound.
9. **How far rule 3 reaches.** As written it applies to all work of the
   organisation, registered or not, for as long as a grant is carried.
   Options: (a) as written; (b) only to registered instances, so a session
   with no instance keeps ordinary access; (c) only to `adapter_collection`
   selectors, so a grant over web hosts never closes ordinary fetch.
   Recommend (a). Registration is the session's own choice, so under (b) a
   principal the grant excludes, or an instance whose budget is spent, reaches
   the same content by not registering, and the grant's narrowing binds
   nobody. (c) has the same effect for every web collection. The cost of (a)
   is the one §Evaluation states: a host-selector grant closes those hosts to
   unregistered work, so it should be entered once registration is routine
   for that work, and the hub should show which hosts a grant will close
   before an owner enters it.
10. **What happens to a collection when its grant ends.** As written, an
    expired or withdrawn grant stays carried and keeps its resources refused
    (`grant_expired`, `grant_withdrawn`) until an owner removes it. Options:
    (a) as written; (b) the hub omits an ended grant from the next envelope,
    so the resources return to ordinary access under source policy after one
    synchronisation, and an owner who wants them closed says so in source
    policy; (c) carry it for a fixed period. Recommend (a). The end of a
    licence is when continued reading of that publisher is most likely to be
    disputed, so reopening should be an owner's recorded act. Under (b)
    `grant_withdrawn` is unreachable and should be deleted. (c) needs a
    number nothing constrains.
