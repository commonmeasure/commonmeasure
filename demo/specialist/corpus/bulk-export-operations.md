# Bulk Export operations in Orchestrator 4.x (Premier)

Applies to: Fictive Systems Orchestrator, release line 4.x.
Integration path: Bulk Export. Support status: supported.
Effective date: 10 April 2026. Entitlement: Premier.

Bulk Export is available to Premier-tier customers only. It streams a
pipeline's full output history to an object store in daily partitions,
bypassing the per-request result limits that apply to the Standard tier.

Operational limits on the 4.x line: at most four concurrent export jobs per
workspace, partitions capped at 50 GB before rollover, and export credentials
scoped to a single destination bucket. Exceeding the concurrency limit queues
the job rather than failing it.

Bulk Export configuration lives in `export.yaml` beside the pipeline plan and
requires the `export:write` capability.
