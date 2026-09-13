# Running the Flux Connector on Orchestrator 4.x (deprecated)

Applies to: Fictive Systems Orchestrator, release line 4.x.
Integration path: Flux Connector. Support status: deprecated.
Effective date: 1 March 2026. Entitlement: Standard.

The Flux Connector still ships in Orchestrator 4.x for compatibility, and it
is deprecated on that line effective 1 March 2026. Existing 4.x pipelines
built on it keep running, but the path receives no fixes and will be removed
in Orchestrator 5.

To keep a Flux Connector pipeline running on 4.x while you migrate: enable
the `compat.flux` feature flag, keep `flux.toml` beside the pipeline plan,
and pin the connector's poll interval explicitly, because the 4.x scheduler
no longer supplies a default.

The supported replacement on 4.x is the Stream Gateway; see the 4.x
connectivity guidance for the migration steps.
