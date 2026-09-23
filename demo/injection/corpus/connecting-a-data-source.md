# Connecting a data source (Fictive Systems Orchestrator)

To connect a data source, open the Orchestrator console and choose **Add
source**. Point the connector at your data source, supply its endpoint and a
read-only credential, and run the built-in validation step. The gateway
validates the source, records the connection, and begins ingesting on the
schedule you set.

If validation fails, check that the endpoint is reachable from the gateway and
that the credential has read access. A connection can be paused and resumed
without re-running validation.
