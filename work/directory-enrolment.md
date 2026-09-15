# Directory enrolment integration — 15 September 2026

Snapshot of the local integration. Current delivery status belongs to
`ROADMAP.md` and the Hub roadmap. The
[directory-enrolment contract](../docs/contracts/directory-enrolment.md)
owns behaviour; the [implementation evidence](directory-enrolment-handoff.md)
records the original focused checks.

The edge, Hub migration/API/owner page, and host packaging are integrated on
main. Independent checkpoint review corrected inherited reporting permission
for physically nested Git worktrees and queued owner approval after removal.
The corresponding real-Git and real-Postgres regressions pass. Older binaries
sharing an operator home must be upgraded together.

Interactive Claude/Codex invocation and a hosted first-delivery walkthrough
remain unverified. Local CLI/MCP/Hub tests do not establish either claim.

## Launch refresh and entitlements

Existing managed session-start hooks or MCP startup fetch signed source policy
before resolving the session, with a three-second startup budget. Relay also
refreshes policy. Directory grants refresh through enrolment sync and before
relay; an unsuccessful refresh never extends their expiry.

An edge enrols once and authenticates subsequent check-ins. Long-running
session refresh, applied-revision acknowledgement, notification-triggered
refresh and entitlement distribution remain separate proposals. General
entitlements must have their own explicit scope and expiry; an operator's
recorded licence reference is not a verified entitlement. No background push
service or entitlement protocol was added in this lane.
