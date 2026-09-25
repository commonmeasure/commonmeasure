---
title: Onward delivery contract
domain: network
audience: integrator
section: reference
---

# Onward delivery contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

- **Owner:** Network.
- **Producer:** the hub, from the Content Telemetry events its organisations'
  edges deliver to it ([telemetry projection](telemetry-projection.md)).
- **Consumer:** a content owner's telemetry endpoint: any receiver that
  accepts a Content Telemetry `event_batch`.
- **State:** built in the hub. Tested in the hub's own suite through its
  production delivery path against receivers on loopback over plain HTTP
  (`fixture-tested`). HTTPS and address pinning are not exercised by those
  tests. No delivery to a public receiver is recorded.
  Deliveries are unsigned; signing is planned (NET-5).

The hub receives each edge's events under the organisation that enrolled the
edge. This contract says how the hub decides which content owner an event
belongs to, which endpoints it owes the event to, what it sends and how it
retries. Terms such as content owner, receiver and hub are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

## Owners and hosts

An organisation registers content owners and the hosts each owner holds. A
host is lower case, contains a `.`, uses only `a-z`, `0-9`, `.` and `-`, and
may carry a numeric port. One host belongs to at most one owner in an
organisation; a second claim is refused (`409`).

An event's content host is the host of its `content_url`: lower-cased, user
information removed, port kept. Its parent host is the content host with the
first label removed and the port dropped. It exists only where the host has
three or more labels and its last label is not numeric. There is one parent
level, and no public suffix list is applied: the parent of `bbc.co.uk` is
`co.uk`.

An event belongs to the owner that registered its exact host, or else to the
owner that registered its parent host. An exact registration wins over a
parent registration by another owner.

Reports and onward delivery resolve owners at different times:

- **Reports** use the registrations in force now. A host registered later
  claims its earlier events in reports.
- **Onward delivery** uses the registration in force when the event happened:
  the earlier of its `timestamp` and the time the hub received it. A host's
  registration periods for one organisation never overlap, so an event is owed
  to at most one owner. An owner registered after an event is sent none of the
  earlier events, and a replaced owner's events do not reach its successor.

An event that no owner holds is kept and listed as unattributed, up to the 50
hosts with the most events. It can still be delivered to an endpoint its source
declares (§Declared endpoints).

An organisation owner may back-fill an owner: send it the events of a past
window (`[from, until)`, by the earlier of timestamp and receipt, `until` not
in the future) that resolve to the hosts it holds now but that it was not owed
when they happened. A back-fill is audited, runs once and uses the owner's
saved destination only.

## Networks of owners

A network is a label on owners, not an account: a name (1 to 64 characters of
`a-z`, `0-9` and `-`), each owner's external identifier and the basis of its
designation. An organisation owner imports up to 500 owners at once. A
re-import matches owners by network and external identifier; it adds hosts
and a missing destination and does not change an existing owner's name or
basis. A host another owner already holds is reported as a conflict and not
moved. The import body is at most 1 MiB.

An import may name a shared destination. It is saved for each imported owner
that has none, as that owner's operator registration (§Routes), and a shared
destination at a private address refuses the whole import before any owner is
registered. A destination set by import does not requeue dead rows.

Delivery is per owner. Owners that share a destination receive separate
batches, one set per owner.

## Routes

An event is owed to the endpoint of every route that names one. Routes add to
each other: none replaces another, and none takes precedence.

| Route | Endpoint | Credential |
|---|---|---|
| `registration` | the destination an organisation owner saved for the owner | sent as `X-API-Key` where one was saved |
| `licence` | the telemetry `<reporting>` endpoint of the RSL licence the event names | none |
| `manifest` | the telemetry endpoint of the Content Telemetry manifest at the event's host | none |

The first is the owner's **saved destination**; the other two are **declared
endpoints**, declared by the source. A saved destination set from a manifest
(below) is labelled `manifest` but follows the saved-destination rules: per
owner, attributed at the event's time, back-filled.

### Saved destination

An organisation owner sets it. It must be `https`, with a host and without
user information or fragment, and must not resolve to a private address; a
name that does not resolve yet is accepted and checked again at each
delivery. Setting or changing it queues the owner's owed events. Clearing it
pauses the route and drops the events not yet delivered, except those in
flight.

An organisation owner can also ask the hub to read the manifests of every
host registered to the owner; an owner with more than 50 hosts is refused
(`400`). Where they name exactly one endpoint
and the owner has no destination, or has one taken from a manifest, that
endpoint becomes the saved destination with no credential, labelled
`manifest`. A destination an organisation owner entered is never replaced
this way.

### Declared endpoints

Declared endpoints need no registered owner. For each event with a content
host the hub reads, once, what the source declares:

- **Licence.** The hub fetches the licence the event names in `license_ref`
  (the edge records it from the source, [session evidence §Source
  declarations](session-evidence.md#source-declarations)) and takes the
  `endpoint` and `profile` of its `<reporting type="telemetry">` element. More
  than one distinct binding is ambiguous and names no endpoint. A document
  type declaration is refused. The fetch is `https` only, follows no
  redirect, reads at most 1 MiB and waits at most 10 seconds; a `license_ref`
  with user information or a fragment is not fetched and names no endpoint,
  and any answer but `200` is a refusal. The endpoint must be `https`,
  without user information or fragment, and resolve to a public address. A result is
  kept for a day where a binding was found and for an hour otherwise.
- **Manifest.** The hub fetches
  `https://<content host>/.well-known/content-telemetry.json` for the
  event's exact host, not its parent. The manifest is accepted where the
  answer is `200` (a redirect counts as absent), `schema_version` is `1.`
  followed by digits, `id` equals the URL fetched, `roles` includes
  `content_owner`, and `telemetry.endpoint` is `https`, without user
  information or fragment, and does not resolve to a private address. Limits
  and caching are the licence's.

A licence may name the endpoint only for events whose host is the licence's
host or shares its registrable domain under the public suffix list. For any
other event the delivery is recorded `withheld` (`endpoint_off_origin`),
unless the host's manifest names the same URL; a later declaration of that
URL from the event's own origin lifts it. A manifest has no such check: it is the host's own declaration.

An event's declared endpoints are fixed from what was read when it arrived.
A licence read that failed to connect or complete, or whose endpoint did not
resolve, and a manifest read that could not reach the host or was answered
`429` or `5xx`, is tried again for 24 hours after the event arrived, and the
event waits for it. A failed read is kept for an hour, so the retries are
hourly. Any other licence answer is final. Each route settles on its own read.

Where a licence and a manifest name the same URL, the event is delivered to
it once, recorded under both routes. A declared endpoint is not sent an event
the saved destination already delivered to the same URL, and acceptance at a
URL by either route marks the event delivered at that URL for both. URLs are
compared as the URL parser serialises them (host case and default ports
normalised), so URLs that still differ, by a trailing slash or a query, are
different endpoints.

## Suppression

An organisation owner may stop delivery to a declared endpoint's origin
(scheme, host and port, the default port and a trailing dot dropped),
whatever declares it, with an optional reason. A suppression may be made
under an impersonated session, and records both people; it may not be lifted
under one. A suppression:

- stops declared routes only; a saved destination at the same origin keeps
  delivering until it is cleared;
- turns the queued deliveries at that origin `suppressed`
  (`endpoint_suppressed`), and is checked again when endpoints are fixed and
  when a delivery is claimed; deliveries in flight are counted and not
  recalled;
- is recorded once: a second request for the origin answers
  `already_suppressed`;
- when lifted, queues what it held with a fresh attempt count.

Nothing else suppresses a declared endpoint. There is no owner opt-out, and
the hub does not suppress an endpoint because an edge also delivers there.

## Outbox and retry

Each owed pair of event and endpoint is a row in an outbox: one for saved
destinations, keyed by owner and event, and one for declared endpoints, keyed
by event and URL. States are `queued`, `delivered` and `dead`, and for
declared endpoints also `suppressed` and `withheld`. Each row keeps its
attempt count, next attempt time, last attempt time, last error and delivery
time.

A worker runs every 15 seconds. It claims up to 500 events at a time and at
most 20 claims per owner, or per declared endpoint, per run. A claim
increments the attempt count, sets the next attempt time and commits before
the request is sent, so a hub that stops mid-delivery retries the claim at
that time. Completion is matched to the claim, so a late answer cannot
overwrite a later attempt. A row claimed in the last 120 seconds counts as in
flight.

Events are batched by session, route and URL for a saved destination, and by
session, content host and URL for a declared endpoint, so one owner's
endpoints never see another host's events.

Each claim sets the next attempt 2^(n−1) minutes after the claim, n being
that attempt's number, capped at 60 minutes: 1, 2, 4, 8, 16, 32, then 60
minutes. A failure at
the tenth attempt makes the row `dead`, keeping its last error.

An organisation owner requeues dead rows: for one owner (its saved-route rows
and the declared rows of events it held) or all declared dead rows in the
organisation. Requeue resets the attempt count, makes the row due at once and
keeps the last error. Saving a destination or applying a discovered manifest
requeues the owner's rows as well. A dead row at a suppressed origin is not
requeued.

Deleting an owner deletes its saved-route rows. Declared-endpoint rows are
keyed to the event and URL, not the owner, and are not removed with it.
Retention removes the undelivered rows of the events it deletes, except rows
in flight.

## What is sent

The hub rebuilds each batch from the events it stored. It does not forward
the bytes the edge sent.

- Document: `document_type` `event_batch`, `schema_version` `1.0`,
  `session_id` and `agent_id` as received, `started_at` in UTC milliseconds
  with `Z`.
- Each event: `id`, `type`, `timestamp` (the same form), `source_role`,
  `content_url`, `data` as received (`{}` where absent), and `license_ref`, `terms_ref` and
  `content_telemetry_id` where present.
- Not sent: the session's `refused` count, the `instance` member (the hub
  does not store it on the event), turn events, and top-level members the
  hub's ingest does not keep. The edge's ingest key is never forwarded.

`content_telemetry_id` is the identifier the edge sent as the
`Content-Telemetry-ID` request header of the fetch
([telemetry projection](telemetry-projection.md)). The hub carries it on the
event so an owner can match the event to its own log line. It is not sent as
a header and is not used for routing or deduplication.

The request is a JSON `POST` with `User-Agent: commonmeasure-hub/<version>`.
`X-API-Key` is sent on the `registration` route only, and only where a
credential was saved. The hub follows no redirect and uses no proxy. It
resolves the endpoint's name again at each delivery, connects only to the
address it resolved, and fails the attempt where that address is not public.
Each of resolution and the request is bounded by 30 seconds.

The endpoint accepts a batch by answering any `2xx`; any other status is a
failure. A body is optional. When it is JSON with an unsigned
`events_created`, the delivery records that count of newly created events;
otherwise the count is unknown, never zero, wherever it is reported. Zero is
valid for a redelivery.

The request is not signed. NET-5 plans a signature under Web Bot Auth from a
relay origin with its own rotatable key, separate from the edges' keys
([bot identity](bot-identity.md)). Until then an endpoint that authenticates
senders by signature refuses the hub's deliveries, and the rows dead-letter
with the endpoint's answer.

## The network report

For a network the hub reports, per owner, the event counts by type, the
number of sessions (null where unknown), the first and last event times, the
counts by supplier (`commonmeasure-supplier`), the enrolled edge key ids seen
as `agent_id`, whether a destination is saved, and delivery counts `queued`,
`delivered` and `dead`. The report states its resolution rule and its window,
which widens to UTC midnight where a daily aggregate straddles a bound. Any
member of the organisation, or an API key with `telemetry:read`, can read it.

The delivery counts cover the saved destination only; deliveries to declared
endpoints are not in the report. A network has no view of its own: the report
is the organisation's.

## Development setting

A hub started in development mode on a loopback listener accepts
`DEV_ONWARD_LOOPBACK`, a list of loopback IP addresses with a port or `*`.
Destinations at those addresses may be `http` and skip the public-address
check; setting one and each successful delivery to one are audited. A hub
refuses to start with the setting otherwise. The edge's end-to-end tests use
it to deliver to a loopback receiver
([session evidence §Instance registration](session-evidence.md#instance-registration)).

## Known gaps

- An event received while an owner's registration is being committed can be
  owed and never queued; nothing reconciles it.
- Suppression is per origin, so a source can move its declared endpoint to a
  sibling host.
- The owners' documentation page states acceptance as `200` or `201`; the hub
  accepts any `2xx`.
