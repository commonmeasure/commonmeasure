---
title: The hub
description: What Common Measure Hub is, what leaves a machine and what never does, how a machine joins an organisation and how the organisation's policy reaches it.
---

# The hub

Common Measure Hub is the organisation service an edge can enrol with.
Common Measure Ltd runs one at hub.commonmeasure.ai, and whether an
organisation may run its own is not decided. An edge is complete without
it: the hub is never in the path of a crossing, and a hub that is
unreachable, closed or wrong never relaxes the policy on a machine. What it
adds is coordination across an organisation's machines: one policy an
owner publishes once and every enrolled machine applies, and one view of
the cleared evidence those machines deliver.

What leaves a machine is the relay's projection in the Content Telemetry
standard: the source address, the content hash and estimated token count of
what was grounded, the licence the source declared, the time, the supplier
and the host tool, under a session id and the edge's key id. A count of the
session's refused crossings leaves with it, so an owner sees policy
enforced across the organisation without seeing what was refused. A
session's crossings leave only when the engagement whose policy scope
matched them carries `allow_telemetry_egress: true`; the admitted sources
of a published run the operator names to the relay leave without that
clearance, because a run carries no engagement. What never leaves is
everything else: prompts, answers, page content, the addresses and reasons
of refused crossings, reconstructed crossings, the engagement name, spend
and quotes, and the edge's private signing key.

A machine joins when an owner mints an enrolment token on the hub's API
keys page, which prints the `commonmeasure connect` command for that
machine, and the person whose machine it is runs it: the edge mints its
signing key and keeps the private half on the machine, and the hub
registers the public half under the organisation and issues the ingest key
the relay delivers under. `commonmeasure disconnect` removes both from the
machine and revokes them at the hub when it can reach it; otherwise the
owner revokes the key there.

Policy arrives when an owner publishes the organisation's policy on the hub
as a signed revision and a machine in managed mode fetches it with
`commonmeasure policy sync`, which checks the signature against the signer
pinned in the machine's `deployment.json`, validates the policy through
the ordinary loader, and replaces `policy.json`, or keeps the last accepted
policy when any check fails
([`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md)).

On the hub an owner sees each machine's delivered sessions by the edge
name chosen at enrolment, registers the content owners whose pages were
read and downloads a report for each with no person, session or engagement
in it, sets how long delivered records are kept, and can revoke any
machine's key. The hub's own documentation, for an owner and the people an
owner invites, is served on the hub: Start here at `/docs/start-here`, and
policy distribution at `/docs/policy-distribution`.

## History before enrolment

A machine's first relay run after it enrols reads every session log the
machine holds, including sessions recorded before it enrolled, and decides
each crossing against the policy in force at that run. A crossing leaves
when it was witnessed (observed by a hook, or mediated by Common Measure's
own tools) and its recorded working directory matches a scope with
`allow_telemetry_egress: true`. Each event carries the time the crossing
happened, not the time it was delivered. A session recorded before the
machine enrolled names no key id, so its events leave under the agent id
`commonmeasure`. Scopes are tried in order and the first whose `match`
appears in the directory decides, so a narrower scope that keeps work on
the machine must be listed before a broader scope that clears it
([`docs/contracts/source-policy.md`](contracts/source-policy.md) §Scopes and
principals).

What stays on the machine from that history: every crossing whose
directory matches no cleared scope, which the relay reports as withheld
sessions; crossings to private addresses or named internal prefixes;
mediated fetches that failed or that the origin answered outside 2xx;
refused crossings, which leave only as a count on a session that sends at
least one event; and every reconstructed crossing. A later relay run under
a wider policy sends what the wider policy clears, and sends nothing
already delivered a second time. A session recorded before enrolment and
resumed after its first delivery names the key id on its next batch, and
the hub refuses a session that changes agent id.

`commonmeasure import` reconstructs work done before Common Measure was
installed, from Claude Code transcripts: a `WebFetch` with a recorded
result becomes one grounded crossing, with a hash and token estimate taken
from the transcript text, and a `WebSearch` becomes one ungrounded crossing
per result link. A reconstructed crossing records no working directory and
no licence, and the relay projects none, so imported history stays on the
operator's own record and never reaches the hub. Codex and Pi transcripts
are reported as not importable
([`contracts/session-evidence.md`](contracts/session-evidence.md)
§Importing history).
