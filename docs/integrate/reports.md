---
title: Receive usage reports about your content
domain: network
audience: integrator
section: integrate
---

# Receive usage reports about your content

When an agent behind a Common Measure edge retrieves or reads one of your
pages, the edge can report it as a Content Telemetry event. The edge
delivers its events to its organisation's Common Measure Hub, and the hub
delivers each content owner's events to the endpoint that owner declared.
You need no account with Common Measure to receive them. The formats are in
[`docs/contracts/telemetry-projection.md`](../contracts/telemetry-projection.md)
(edge to hub) and
[`docs/contracts/onward-delivery.md`](../contracts/onward-delivery.md) (hub
to you); terms such as content owner and receiver are in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

## 1. Declare a destination

Either declaration below is enough. The hub reads them for each event's host
and delivers to every endpoint named; a URL named by both receives each event
once.

**A Content Telemetry manifest** at
`https://<host>/.well-known/content-telemetry.json` for each exact host
whose pages you want reported. A manifest at `example.com` does not cover
`www.example.com`.

```json
{
  "schema_version": "1.0",
  "id": "https://publisher.example/.well-known/content-telemetry.json",
  "roles": ["content_owner"],
  "operator": {"name": "Publisher Example"},
  "telemetry": {"endpoint": "https://telemetry.publisher.example/v1/events"}
}
```

The hub accepts it when the answer is `200` with no redirect,
`schema_version` is `1.` followed by digits, `id` is the URL it was fetched
from, `roles` includes `content_owner`, and the endpoint is `https` at a
public address.

**An RSL licence with a telemetry reporting binding**, named from your
`robots.txt` with a `License:` line:

```xml
<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">ai-input</permits>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
                 endpoint="https://telemetry.publisher.example/v1/events">
        <![CDATA[{"conformance_level": "grounding", "privacy_level": "minimal"}]]>
      </reporting>
    </license>
  </content>
</rsl>
```

A licence names the endpoint only for pages on its own host or its
registrable domain. The edge also reads the binding as a demand. It takes
such a page only where it can report it: the operator's policy clears the
working directory for reporting, a receiver is configured and delivery is
automatic. Otherwise it refuses the page
([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Source declarations).

An organisation that uses Common Measure Hub can also register you as a
content owner and save a destination for you, with an API key sent as
`X-API-Key`. That is the organisation's choice, not a declaration you make.

## 2. What arrives and when

Each delivery is a JSON `POST` of one Content Telemetry `event_batch`
holding events from one agent session:

- `content_retrieved`: the agent's edge fetched `content_url`.
- `content_grounded`: the page's text entered the model's context, with the
  hash of that text in `data.content_hash` and, where the edge recorded one,
  a token estimate in `data.tokens_ingested` with its basis.
- `license_ref`, `terms_ref` and `content_telemetry_id` where present;
  `agent_id`, the key id of the edge that fetched (see
  [Recognise and verify `CommonMeasureBot`](bot.md)).

The hub rebuilds each batch from the events it stored. It does not send the
edge's refused count or its instance reference, and it sends no turn events.
Requests carry `User-Agent: commonmeasure-hub/<version>` and are not signed.

Timing: an edge relays when a Claude Code session ends, on a hosted edge's
interval, or when its operator runs `commonmeasure relay`. The hub's worker
runs every 15 seconds. A failed delivery is retried after 1, 2, 4, 8, 16 and
32 minutes and then hourly; after the tenth failure the row is dead until
the organisation requeues it.

Only activity the edge's operator has cleared for reporting leaves the edge,
so a missing event does not show that a page was not used.

## 3. Match events to your logs

An edge sends a `Content-Telemetry-ID` request header on a fetch to a host
whose manifest it has verified or whose licence it has read. The same UUID
is the event's `content_telemetry_id`. Log the header. The first fetch of a
new host carries none, because the edge has not yet read what the host
declares; search results carry none. Run against a loopback publisher that
serves a manifest and logs the header, two fetches of the same host gave:

```text
/first Content-Telemetry-ID: None
/second Content-Telemetry-ID: 2075b648-6e2d-424a-9405-ce66271b9004
```

and the edge recorded that UUID on the second crossing. The event's
`agent_id` is the `keyid` in the request's `Signature-Input`, so a request
you served and the event reporting it name the same edge.

## 4. Run a conforming receiver

Your endpoint must:

- accept a JSON `POST` of an `event_batch` at the URL you declared, over
  HTTPS at a public address;
- answer any `2xx` status once the batch is stored. A body is optional.
  A JSON body such as `{"status": "ok", "events_created": 2}`, where
  `events_created` is the number of events newly stored, lets the sender
  report that count; without it the count is recorded as unknown. Zero is
  valid for a redelivery. Any other status is a failure and is retried. The
  hub and an edge delivering to you directly apply the same rule;
- deduplicate by event `id`. A redelivered event keeps its `id`, and batches
  can arrive out of order.

`demo/arc/receiver.py` in the repository is a minimal receiver: it accepts
`POST /events`, stores each batch as a line of NDJSON and answers `200` with
an `events_created` count. To test yours, point an edge's relay at it. From
a checkout with the binary on `PATH`, with a throwaway operator home and a
policy that clears one working directory for reporting:

```sh
export COMMONMEASURE_HOME=$(mktemp -d)
W="$COMMONMEASURE_HOME/work/acme" && mkdir -p "$W"
cat > "$COMMONMEASURE_HOME/policy.json" <<EOF
{"scopes": [{"match": "$W", "engagement": "acme", "allow_telemetry_egress": true}]}
EOF
printf '%s' "{\"session_id\":\"4b6c0f64-1f0e-4d7b-9a55-3c2d1e0f9a88\",\"cwd\":\"$W\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"WebFetch\",\"tool_input\":{\"url\":\"https://www.example.org/opening-hours\"},\"tool_response\":{\"result\":\"The library opens at nine.\"}}" \
  | commonmeasure hook post-tool-use
python3 demo/arc/receiver.py 8378 "$COMMONMEASURE_HOME/received.ndjson" &
commonmeasure relay --receiver http://127.0.0.1:8378
```

The report begins:

```text
delivered 2 events in 1 batches to http://127.0.0.1:8378 (2 new at the receiver)
  1 content_grounded
  1 content_retrieved
  2 under governing engagement acme
```

The relay posts to `<receiver>/events`; the hub posts to your endpoint URL
as declared. `received.ndjson` then holds the batch, abridged:

```json
{"document_type": "event_batch", "schema_version": "1.0",
 "session_id": "4b6c0f64-1f0e-4d7b-9a55-3c2d1e0f9a88", "agent_id": "commonmeasure", "refused": 0,
 "events": [
  {"id": "99aa2f06-…", "type": "content_retrieved", "source_role": "agent",
   "content_url": "https://www.example.org/opening-hours", "data": {…}},
  {"id": "da9b48a2-…", "type": "content_grounded", "source_role": "agent",
   "content_url": "https://www.example.org/opening-hours",
   "data": {"scope": "session", "content_hash": "sha256:da217a1f…", "tokens_ingested": 7, "token_basis": "characters/4", …}}]}
```

An edge that is not enrolled with a hub sends `agent_id: commonmeasure`,
and a batch from the hub has no `refused` member. A receiver that answers
`204` with no body is refused and the batch stays queued:

```text
commonmeasure: delivery to http://127.0.0.1:8378 failed; undelivered batches remain spooled under <home>/relay/spool: receiver answered 204: an empty body
```

Delivery from the hub to your endpoint needs an organisation whose edges
fetch your pages; it cannot be reproduced on loopback, because the hub
delivers only to HTTPS endpoints at public addresses.
