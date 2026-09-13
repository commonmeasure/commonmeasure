# Connecting data sources in Orchestrator 3.x

Applies to: Fictive Systems Orchestrator, release line 3.x.
Integration path: Flux Connector. Support status: supported.
Effective date: 15 April 2025. Entitlement: Standard.

In Orchestrator 3.x, the Flux Connector is the supported way to connect an
external data source. Declare the source in `flux.toml`, grant the pipeline
the `flux:read` capability, and the connector polls the source on the
pipeline's schedule.

The Flux Connector on the 3.x line handles credential rotation itself:
rotate the secret in the source system and the connector picks it up on the
next poll without a pipeline restart.

Fictive Systems supports the Flux Connector for the life of the 3.x
long-term support line. Customers planning a move to 4.x should read the
4.x connectivity guidance before migrating, because the recommended path
differs.
