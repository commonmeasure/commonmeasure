---
title: Instance registration contract
---

# Instance registration contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

**Verification state: planned, except where a section says otherwise.** This
is the authoritative design for network instance registration. The hub's first
increment and the edge client and session reference
([session evidence §Instance registration](session-evidence.md#instance-registration),
`fixture-tested`) implement part of it; reporting duties are bound by the
hub's second increment and named by the edge client (`fixture-tested`), and no
run has shown one delivered; no entitlement,
shared limit, hosted-principal route or CM Attestation assertion below is
implemented or verified by this document. Existing edge enrolment, hosted authentication and
source-policy distribution are prerequisites with their own evidence; they do
not establish instance registration.

The network's working participant is an agent instance. Registration binds it
to an accountable operator and the authority under which it acts; it does not
infer licence rights from a declaration of identity. Child instances act under
bounded delegated authority with their own attribution. Expiry, revocation and
unexpected termination leave outstanding duties and retained provenance with an
accountable durable service after the process has gone. An organisation
connects its licences and supplier accounts once, and permanent supplier keys
are not distributed to temporary instances. Open owner choices are listed
below; recommendations are not adopted decisions.

## Identities and relationships

| Identity | Identifier and relationship |
|---|---|
| Organisation | Reuse Hub `organization.id` (`organization_id` in its database, `organisation` in policy envelopes). Resolve it from verified authority, never from a request's assertion alone. |
| Enrolled edge | Reuse `edge_keys.key_id`, the RFC 7638 thumbprint of its Ed25519 key. An edge belongs to one organisation. Its ingest key remains a separate delivery credential. |
| Hosted edge | Reuse the organisation's `hosted_edges` row and registered origin. Bind the exact MCP resource URL and the enrolled key of the service operating it. An origin alone is not proof of that key relationship; registration must validate an explicit service binding. |
| Operator principal | For a hosted OAuth principal reuse verified issuer, `sub`, organisation and grant reference. For edge authority reuse `edge_keys.member_id` where present, plus an explicit delegation identifying who may act for the operator. An edge signature authenticates the edge, not every person using it. A local OS principal is evidence local to that edge, not a Hub user id. |
| Host | Reuse session evidence `host` and, where supplied, `client` name/version. Native implementations supply their own host identifier and version. These are attributed declarations, not credentials or proof of behaviour. |
| Instance | A new opaque service-issued identifier for one working participant. Scope references by registration-service issuer and organisation. Never reuse an edge key, OAuth client id, process id or session id as the instance id. |
| Job or session | Reuse `ContextJob.id` as `job_id`, run `run_id` where available, and the host's existing `session_id` (including hosted `Mcp-Session-Id`). Native work identifiers include their namespace. At least one work reference is required; send identifiers and purpose/use categories, never raw prompts. |
| Parent instance | Reference another registered instance under the same organisation and an authorised delegation. A root has no parent. The parent chain is acyclic, retained and checked when registering a child. |
| Durable service | Reference an operator-approved service identity and its authenticated acceptance of reporting, evidence and recovery duties. It can outlive the host and need not be Hub. A URL supplied by the instance cannot establish acceptance. |

One edge can serve many instances; one instance may span several sessions or
runs for its declared work. A session may contain separately attributable parent
and child activity. Each governed action resolves to one instance and the
registration revision in force then. Session and work references are scoped by
organisation and originating edge/host; equal strings on different edges do not
join records. Adding a work reference requires an authenticated revision and
cannot silently expand the permitted work.

The existing identity sources are [directory enrolment](directory-enrolment.md),
[host integration](host-integration.md#1-the-two-ways-in),
[session identity](session-evidence.md#client-identity) and
[run output](run-output.md). The hub stores enrolled edges, hosted edges and
OAuth grants in its own database, whose schema is not published.
Registration must preserve those identities and their privacy bounds.

## Authority and request protection

All lifecycle operations use HTTPS, with HTTP limited to loopback verification.
An enrolled edge authenticates with its existing key signature. For these
mutations the signature must cover the method, public request target, body digest,
creation/expiry times and nonce. Verify the active enrolled key and organisation,
retain used nonces for the accepted signature window, and bind the idempotency
key to the authenticated caller and body digest. A retry signs a fresh nonce
while retaining its idempotency key. The read-only desired-policy
signature's replay tolerance is insufficient for these operations. Signature
canonicalisation and shared vectors must be fixed before server/client coding.

A hosted principal authenticates with a token from a trusted issuer, valid for
the registration resource and lifecycle scope. Verify signature, issuer, audience,
subject, organisation, expiry, scope and current grant/member standing on each
mutation. Current MCP access tokens have the MCP endpoint as audience; they must
not simply be accepted at a new Hub registration endpoint. The owner must select
the token path below. An OAuth client registration identifies client software;
it grants no operator authority.

An edge may register for a hosted principal only with verifiable delegation that
binds that principal, hosted resource, edge and permitted work. A request cannot
choose an arbitrary `sub`. An edge token's `token:<label>` is edge-local and does
not resolve to a Hub member: it needs an explicit sponsor delegation or receives
`unavailable`, naming the missing operator authority. Legacy enrolled keys with
no member likewise require an explicit operator/sponsor binding.

A native host running no CM code sends the same lifecycle fields, work references
and acceptance evidence over this interface. It either implements the enrolled
key-signature path or obtains the scoped hosted-principal token. It must enforce
the returned conditions at its own boundary and record its observations and gaps.
A host name, installation or self-declared attestation cannot substitute for
authentication. No plugin installation or CM-private library is required.

## Registration, renewal and closure

The routes and field names in this section are **planned API design**, not
available endpoints or a final serialisation specification. CM Attestation has
no wire format here. All requests carry a unique idempotency key; mutations of
an existing instance also carry its expected revision. Responses include a
request reference and server time. Unknown fields, unsupported versions and
unrecognised authority are refused explicitly.

| Operation | Request | Successful response |
|---|---|---|
| `POST /api/v1/instances` | Host/client declaration, typed work references, optional parent reference, requested entitlement scopes and limits, applied source-policy revision/digests, requested expiry, reporting duty/destination references and durable-service acceptance reference. Operator and edge claims must match authenticated bindings. | `201`: instance reference, revision 1, active status and the complete accepted binding described below. A replay of the identical request returns that same instance/revision with `200` and current lifecycle status; it cannot make an expired response usable. |
| `POST /api/v1/instances/{instance}/renew` | Expected revision, current work references and policy identity, requested new expiry or narrower conditions, durable-service acceptance and any changed duty references. | `200`: the same instance, incremented revision and revalidated binding. Retain previous revisions. No counters or reservations reset. Broader authority requires fresh explicit issuer/delegator authorisation. |
| `POST /api/v1/instances/{instance}/close` | Expected revision; outcome `completed`, `cancelled` or `failed`; final evidence references and digests; outstanding-duty references and last known delivery states. | `200`: closed lifecycle state, closure time, retained binding, durable-service receipt and separate duty states. Closure never says a pending duty is fulfilled. |
| `GET /api/v1/instances/{instance}` | The same authorised caller, or an operator/durable-service reader explicitly permitted for this instance. | `200`: current revision, validity, revocation/closure state and duty status, with only the fields that reader may see. No public operator directory is created. |

Use `400` for malformed requests, `401` for invalid authentication, `403` for
insufficient authority, `404` for absent or inaccessible instances, `409` for
revision/idempotency conflicts or terminal lifecycle state, and `503` with an
`unavailable` outcome naming a missing required dependency. A refusal carries a
stable reason and no new authority. Reusing an idempotency key with changed bytes
is a conflict. Retried close/renew requests return their retained operation
result and current standing without repeating the transition. Authorisation is
rechecked before returning a replay result.

Commit the registration revision, delegation, limit references, accepted duty
handoff and audit record atomically, or leave the request unsuccessful. A remote
handoff needs a durable, verifiable receipt before activation; a failed handoff
cannot leave an active instance. Serialise renewal/closure with revocation and
limit changes. Owner-authorised revocation records actor, reason, effective time
and affected revisions; the owner management route remains to be specified.

### Accepted binding

The service signs each immutable accepted binding under an operator-trusted
registration signer, with an explicit registration purpose and schema version.
A deployment may reuse its pinned policy key only with that purpose separation;
a policy envelope cannot be interpreted as registration authority. Verify signer,
signature, digest, audience/identity bindings, revision and validity before work.
Retain the signed bytes and the key evidence needed for historical verification.
A signature proves issuance; current standing still requires the agreed revocation
check. Signing layout and canonicalisation are implementation prerequisites,
separate from the deferred CM Attestation wire format.

The response binds all of the following, with references retained for later
verification rather than only mutable URLs:

- The issuer, organisation, authenticated operator/delegation, edge or hosted
  resource, host declaration, instance, work references and parent chain.
- Each entitlement's issuer, immutable revision or digest, covered resources,
  permitted uses, delegation limits, validity, revocation/validation mechanism
  and explicit offline bounds (§Entitlement binding). An unresolved entitlement
  cannot authorise work.
  An empty set grants no licensed access; it is acceptable only where the declared
  work has no entitlement requirement.
- The source-policy revision and envelope digest, plus the locally applied policy
  digest/effective policy identity where available. A fetch does not prove
  activation. No published or verified policy means required setup is unavailable.
  A subsequent policy change needs a matching registration revision before work
  relying on that change; admission continues enforcing restrictions meanwhile.
- The validity window, next required authority check and agreed revocation bound.
  Expiry is no later than the applicable delegation and entitlement bounds.
- Stable shared limit/reservation references for the operator, job and applicable
  day/month periods. Instances and descendants consume the same relevant accounts.
  Existing per-edge allowance storage alone cannot enforce a cross-edge limit.
- Each reporting obligation's issuer and terms revision, intended receiver and
  routing authority, required event classes, deadlines, retention conditions and
  any explicitly agreed outage duration and queue limits. An explicit publisher
  destination takes precedence over a Hub default; registration cannot redirect
  reports by inventing publisher approval.
- The durable service's identity, accepted duties, receipt, evidence-location
  references and access/retention conditions. Credentials and private content
  never appear in the registration response or audit trail.

Registration itself grants neither a licence nor supplier credentials. It binds
references to separately authorised rights; credential issuance, supplier admission
and payment settlement keep their own interfaces and verification. Registration
also cannot override [directory reporting consent](directory-enrolment.md) or
source-policy egress restrictions. If those restrictions conflict with mandatory
reporting, refuse affected work and identify the conflict before it begins.

## Entitlement binding

**Planned.** An entitlement is a grant bound to an instance. The grant object,
its members, its evaluation at the edge and its pinned examples are the
[grant contract](grant.md); this section says how registration carries it. The
hub changes below are proposals for the hub; today the hub answers
`503 entitlement_authority` to any registration that names an entitlement or
sets `requires_entitlements`.

**Reference.** A registration names each grant in the existing `entitlements`
member as `grant:<issuer id>/<grant id>`; neither id contains `/`
([grant contract §The document](grant.md#the-document)). It names no revision: the hub
resolves the reference against the grant set of the envelope the registration
already presents in `policy.envelope`, which the hub has checked is the one
it served this edge for the current revision. The edge and the hub therefore
bind the same grant revision without a second fetch. `requires_entitlements:
true` declares that every acquisition by the instance needs an authorising
grant ([grant contract §Evaluation](grant.md#evaluation)); with it, an empty
`entitlements` is refused.

**What the hub verifies at registration and at renewal**, for each reference:

| Check | Proposed refusal |
|---|---|
| The reference resolves in the caller's organisation and in the presented envelope's grant set. One that does not resolve, including another organisation's grant, is answered as an unknown one is, so the answer discloses nothing | `503 entitlement_authority` |
| `requires_entitlements` is true and `entitlements` is empty | `403 entitlements_required` |
| The grant's `grantee.organisation` is the caller's organisation | `503 entitlement_authority` |
| The grant is inside its validity window, and the requested expiry is no later than its `not_after` | `403 entitlement_not_valid`, `403 requested_expiry_exceeds_entitlement` |
| The delegation's operator holds one of `grantee.principals`, where the grant names any | `403 entitlement_principal` |
| The grant's `state` is `current` | `403 entitlement_withdrawn` |
| A `derived` grant's coverage snapshot is within its `max_age_seconds`. The snapshot does not bound the instance's expiry: the edge stops at the snapshot's age (`coverage_stale`) and resumes on a refresh | `503 entitlement_authority` |
| The grant's revocation check is one the hub can serve: `distribution` in this version | `503 entitlement_authority` |
| A `distribution` grant whose issuer is not the caller's organisation carries the offline bound its issuer agreed and the evidence of it ([grant contract §Offline use of an external grant](grant.md#offline-use-of-an-external-grant)). The catalogue refuses entry without them; registration checks again | `503 entitlement_authority` |
| A grant whose reporting duty is `required` has its duty among `duties` (below) | `403 entitlement_duty_missing` |
| A grant with a budget has its limit account among `limits` (below) | `403 entitlement_limit_missing` |
| A child names no grant its parent does not hold | `403 entitlements_wider_than_parent` |
| A renewal names the same grants or fewer. A grant revised since binds at its new revision, which is the issuer's own authorisation. A grant withdrawn since is refused by name, and the caller renews without it | `403 renewal_widens_entitlements`, `403 entitlement_withdrawn` |

The hub does not verify `grantee.engagements`: a work reference is a job, a
run or a session, and none names an engagement. The edge enforces engagement
from source policy. An engagement work reference is a later increment.

**What the binding carries.** For each grant: the reference, the issuer, the
grant's `id`, `revision` and `digest`, its `basis.kind`, the collection id,
`validity` with its offline bound, the duty reference bound for it or `null`,
and the limit account and reservation bound for it or `null`. The binding
already embeds the retained registration, and the edge reads
`requires_entitlements` from there. At hub `77875ed` the binding's `operator`
is a member id. It keeps that type, and a new member `operator_identifiers`
carries the operator's identifiers as `{scheme, value}` pairs, so the edge can
compare principals offline. The binding is signed as every binding is. It
copies no price, terms reference or issuer field; those stay in the grant
document the envelope carries and are read from there by digest.

**Reporting duty.** A grant's duty becomes the reporting duty the hub already
persists for a registration, with no second duty model:

- `private_record_only` binds no duty. Records of acquisitions under the grant
  stay in the private record.
- `required` maps to one content owner of the organisation, and the catalogue
  holds the mapping from the grant to that owner. The registration carries the
  existing reference `owner:<content owner id>` in `duties`, the hub checks it
  is the one the grant maps to, and the duty is persisted, computed and closed
  as it is now. The request still cannot name a receiver.
- A `content_telemetry` duty has a `receiver` destination. The owner's onward
  routing names that receiver, an explicit publisher destination keeps
  precedence over a Hub default, and the onward delivery that exists delivers
  the duty.
- An `issuer_batch` duty has an `issuer_api` destination, which takes
  aggregate batches. It is never an onward destination: onward delivery sends
  event-level Content Telemetry, and pointing it at the issuer's interface
  would send the issuer records it did not ask for. The duty maps to a content
  owner with no onward destination. Under the hub's duty states at `77875ed`
  it answers `outstanding` with reason `owner_has_no_destination` from
  registration, and `held` from its first attributed event, because each
  event is `unrouted`. From hub `f84767c` each duty also carries `reasons`,
  every cause that holds, the most specific first; `reason` is its first
  entry, or `null` when none holds. This duty's `reasons` read
  `["owner_has_no_destination"]` from registration and
  `["owner_has_no_destination", "events_unrouted"]` once held. That reads as
  a fault when the duty is waiting on a report nobody has built. A distinct
  reason or state for a duty owed as the issuer's batch report is a proposed
  hub change. Until a mapping to the issuer's report is agreed and built the
  hub never reports the duty `delivered`.

**Budget.** A grant's `price.budget` is the issuer's ceiling, or the
organisation's own in a self-issued grant. It is enforced as a shared limit
account whose authority reference is the grant reference and whose ceiling an
owner may set lower, never higher. The grantee's lower figure lives on the
account and not in the grant document, which the issuer may have signed. A
`period` relies on period rollover, which the hub lists as planned. The
registration names the account in
`limits`, the root instance takes its reservation, and the edge spends that
reservation offline and reconciles at closure, under the rules that exist.

**Offline enforcement.** The grant travels in the signed policy envelope
([grant contract §Distribution](grant.md#distribution)). The edge authorises
an acquisition from three things it holds: the envelope in force, the signed
accepted binding and its clock. It needs no hub call for a `distribution`
grant. A withdrawn grant reaches the edge marked `withdrawn` in the next
envelope, and the binding that names it then authorises nothing under it
(`grant_withdrawn`). A refreshed coverage snapshot reaches the edge in a
same-revision renewal of the envelope and leaves the grant's digest, and so
the binding, unchanged. For an edge that cannot reach the hub, a withdrawal
takes effect within the least of the grant's agreed offline bound, the
envelope's expiry and the instance's expiry.

**Unchanged.** The registration vector
(`crates/commonmeasure-relay/tests/fixtures/registration-request.json`, a copy
of the hub's) pins the
signature profile over the method, target, content digest and idempotency
key; it does not move. The request body stays `version: 1`: `entitlements` is
already a list of strings and `requires_entitlements` a boolean. The
`policy.envelope` check is unchanged in kind, since the grants are inside the
envelope it already compares. Routes, idempotency, standing and closure are
unchanged. The duty states are unchanged for a `content_telemetry` duty; an
`issuer_batch` duty fits them poorly, as Reporting duty says.

**What a later increment must add.** In the hub: the grant catalogue with
authoring, revision, withdrawal, removal, derivation and the entry check for
an external grant's offline bound; `payload.grants` and
`payload.grants_digest` in the envelope it serves, with a refreshed coverage
snapshot carried in a same-revision renewal; the resolution above in
place of `503 entitlement_authority`; the binding's `entitlements` and
`operator_identifiers`, with a binding schema version if its readers refuse
unknown members; the mapping from a grant to a content owner and to a limit
account; a reason or state for an `issuer_batch` duty; period rollover of a
limit account. On the edge: the client's `entitlements` and `requires_entitlements`
arguments, verification of the binding's entitlements, and the evaluation the
grant contract defines.

## Expiry, revocation and shared limits

An instance is active only within its verified validity window and all applicable
authority checks. Expiry, closure or revocation stops new affected acquisition
and use; retaining cached content does not extend its permitted use. The host
must identify unobservable or unenforceable paths as gaps, not assert they stopped.
There is no automatic grace period. Failed renewal leaves only the previous
still-valid authority within its original bounds; after expiry it leaves none.
An expired, closed or revoked instance cannot renew back to active. A new explicit
registration may reference it as predecessor but retains shared limits and duties.

Child scope and expiry cannot exceed the parent's delegated authority. Parent
expiry, closure or revocation disables its dependent descendants; their evidence
and debts remain. Creating a child, restarting a process, retrying a request or
registering a replacement never resets consumption or releases an outstanding
reservation without reconciliation against evidence.

Online checks are required unless the relevant issuer explicitly permits bounded
offline use. A cached response cannot conceal a known revocation; an unavailable
required check returns `unavailable` and refuses affected work. Clock uncertainty
that prevents establishing validity also makes authority unavailable. The maximum
revocation delay and any clock tolerance need the owner choices below; clients
cannot inherit the more permissive hosted-token leeway as an authority extension.

A stale cached source policy remains enforced under the
[policy envelope contract](policy-envelope.md#activation-and-the-last-known-good).
That enforcement renews neither instance authority nor entitlements.

## Closure and surviving duties

The durable service accepts responsibility before the instance starts, so its
process may stop after completion, cancellation, failure or loss of connectivity.
If closure cannot reach the service, the local outcome is closure unacknowledged;
expiry still stops authority and the service reconciles the missing closure.
A missing final record leaves an evidence gap. It is never inferred to mean
nothing was acquired or nothing is owed. A process that stops without closing
leaves the instance active at the service until an operator closes it from the
same home or its window passes; the edge closes nothing on the operator's
behalf and infers no outcome. Either ends the instance for the purpose of a
duty's fulfilment read.

Closure and duty fulfilment are independent. The service retains admitted-work
and report references durably as work proceeds, before acknowledging custody;
a final upload alone cannot protect against process death. It keeps the private
source record separate from Content Telemetry egress, retries outstanding reports
under the agreed terms, and retains receiver acceptance receipts. A queued, held
or dead-lettered report remains undelivered. No automatic grace period follows
from an unavailable destination. Continued acquisition/use needs the publisher's
explicit outage allowance and stays within its duration and queue limits.

After exit the service supports authorised evidence retrieval, verification of
output hashes and provenance links for the agreed retention period, duty status,
recovery and an auditable transfer to a successor service. Transfer requires the
successor's acceptance; it cannot erase unresolved duties. Provenance establishes
recorded origin and associations, not factual correctness or hidden observations.
A service must report unavailable evidence honestly. Retention/deletion conflicts
need agreed terms before activation; registration cannot promise indefinite
retention or silently defeat an applicable deletion requirement.

## Session evidence reference

Built on the edge ([session evidence §Where](session-evidence.md#where) and
[§Instance registration](session-evidence.md#instance-registration)): one
optional `instance` reference on records attributable to a registered instance.
Its members are the registration issuer, instance id and binding revision. The
reference resolves to the retained authenticated binding, including organisation,
operator, edge, parent and work relationships; these private identities are not
copied into publisher telemetry. Revision identifies the authority at the event,
not whatever a later lookup happens to return.

Absent means no recorded registration reference; existing records remain valid
and cannot be retroactively described as registered. Unknown validity remains
unknown. This reference proves neither compliance nor fulfilment. The relay
sends it only to the operator's own hub, where that hub issued it, as an
event-level member the hub removes before onward delivery
([session evidence §Instance reference](session-evidence.md#instance-reference)).
It adds no Content Telemetry or SPUR field to what a publisher receives and
does not replace the existing edge agent id or telemetry session id. A reader that resolves the reference to the retained
binding and verifies it is planned.

## CM Attestation assertions

All assertions below are **planned**. Each needs its own issuer authority, subject,
validity and revocation check. A registration service may relay an assertion only
with its originating evidence; its signature alone cannot turn it into a licence
issuer. No token layout, programme recognition or commercial guarantee is specified.

| Assertion | Issuer and subject | Validity and independent verification |
|---|---|---|
| Accountable participation | Registration service authorised by the operator; subject is the instance and operator/delegation binding. | Registration window and standing; verify service trust, authenticated enrolment/grant, delegation, revision and revocation. Host labels remain declarations. |
| Scoped entitlement authority | Supplier, licensor or explicitly delegated issuer; subject is the instance's permitted resources/uses. | Underlying entitlement and delegation windows; verify issuer recognition, evidence digest, scope and current validation/revocation result. Registration alone establishes none of these rights. |
| Policy binding | Registration service records the required policy; host/edge separately declares its applied identity; subject is this instance/revision. | Applicable revision and event time; verify policy signer/digest and compare applied identity. Actual enforcement needs separately observed decisions and coverage evidence. |
| Reporting commitment | Operator or authorised reporting obligor, with durable-service acceptance; subject is this instance's named duties and destination. | Duty period, deadlines and terms, including duties surviving expiry; verify authority to commit, terms revision and service receipt. Receiver acceptance separately establishes delivery. |
| Payment commitment | Party authorised under the relevant agreement; subject is the instance's scoped payment obligation and responsible account reference. | Agreement scope, period and standing; verify issuer/delegation and agreement evidence. A settlement receipt is separate; absent payment evidence is unknown. |
| Surviving evidence custody | Durable service accepted by the operator and applicable terms; subject is the evidence/duties it accepted for the instance. | Agreed custody/retention period, including after exit; verify authenticated custody receipts and authorised successor chain. Retrieval, digest checks and delivery receipts establish later performance separately. |

No assertion certifies future behaviour, truthful reporting, factual accuracy,
payment completion, SPUR accreditation or complete observation of the host.
An unsupported assertion is unavailable; it is never filled from registration
success or an installed processor list.

## Owner choices before implementation

1. **Hosted token path.** Options: a registration-resource token issued through
   explicit OAuth consent/exchange, or an authenticated hosted-edge facade that
   delegates to Hub. Recommend the dedicated audience and lifecycle scope for
   native interoperability; never reuse MCP-audience tokens at Hub. Fix routes,
   signature canonicalisation and shared vectors once this boundary is selected.
2. **Operator sponsorship.** Options: require a current named Hub member, or permit
   an organisation-approved durable sponsor for service/native/consumer instances.
   Recommend both with explicit delegation records; legacy memberless keys and
   local token labels get no inferred authority. Owner approval mechanics remain open.
3. **Authority windows and offline use.** Options: online validation for affected
   work, or issuer-agreed bounded caching. Recommend online validation for the
   first route and no implicit grace; select numerical lifetime, renewal lead time,
   clock bound and revocation deadline with that supplier before acceptance tests.
4. **Shared accounting.** Options: central reservations per authorised account, or
   issuer-approved partitioned allowances. Recommend central reservations for the
   first multi-edge route. Currency, use units and reconciliation rules must follow
   the agreement; a new universal payment model is not required.
5. **Durable service and retention.** Options: operator service, optional Hub service
   or another authorised provider. Recommend one explicitly accepted service for
   the first route, with crash recovery, retention, deletion, access and succession
   terms agreed before activation. Choose reporting outage allowances with the
   publisher; absence supplies no allowance.
6. **Recognised assertions and recourse.** Options: supplier-specific acceptance
   first, or a broader programme. Recommend one scoped supplier agreement first.
   Recognised issuers, payment commitments, evidence access, deadlines, remedies,
   independent dispute decisions and appeal are owner/counterparty matters, recorded
   outside this product repository. No liability or accreditation rule is adopted here.

## Verification required

Every item is **planned** unless it says otherwise; documentation checks establish links and consistency only.

- Server/client contract vectors: identity mapping, wrong organisation/member/edge,
  invalid token audience/scope, body tampering, nonce replay, idempotent retries,
  revision conflicts and renewal racing revocation or closure.
- Entitlements: the [grant contract's pinned examples](grant.md#examples)
  through the production edge path, and each refusal of §Entitlement binding
  against the real hub handlers.
- Lifecycle and limits: expired authority with stale enforced policy, offline and
  issuer outage bounds, child narrowing and cascade, restart/replacement without
  replenishment, and cross-edge reservation concurrency.
- Real Hub handlers/database and the production edge/native transport through
  loopback: registration, policy activation, scoped work, renewal and closure.
  Focused doubles establish failure handling only; they do not verify integration.
- Kill the instance after durable custody with reporting outstanding, restart the
  responsible service, recover receiver-accepted delivery, and verify retained
  output provenance after exit. Also lose the final append and expose the gap.
  Run on loopback for the first route on 18 September 2026, and repeated on
  21 September 2026, with the hub as the durable service and a test double as
  the onward receiver; the lost final append was
  shown and stays open, since nothing repairs the cut-short record.
- Verify directory consent and private-record isolation through projection and
  delivery; demonstrate an independent native implementation using the interface.
- Verify each accepted attestation assertion separately from reporting/payment
  receipts. Live supplier authority and programme acceptance require named external
  evidence; neither a fixture nor this contract establishes them.
