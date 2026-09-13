# Connecting data sources in Orchestrator 4.x

Applies to: Fictive Systems Orchestrator, release line 4.x.
Integration path: Stream Gateway. Support status: supported.
Effective date: 1 March 2026. Entitlement: Standard.

From 1 March 2026, the Stream Gateway is the supported way to connect an
external data source to Orchestrator 4.x. It replaces the Flux Connector,
which is retired on the 4.x line: do not build new 4.x integrations on the
Flux Connector.

Declare the source in `gateway.yaml`, grant the pipeline the `gateway:ingest`
capability, and the Stream Gateway delivers change events as they occur —
there is no polling schedule to configure.

Unlike the Flux Connector, the Stream Gateway does not rotate credentials
itself: rotation is handled by the platform secret store, and a rotated
secret takes effect at the next pipeline restart.
