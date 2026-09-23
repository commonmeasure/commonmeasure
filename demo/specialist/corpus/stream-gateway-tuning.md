# Tuning Stream Gateway throughput in Orchestrator 4.x

Applies to: Fictive Systems Orchestrator, release line 4.x.
Integration path: Stream Gateway. Support status: supported.
Effective date: 20 May 2026. Entitlement: Standard.

The Stream Gateway's default settings favour delivery order over throughput.
Three settings in `gateway.yaml` govern the trade-off:

- `batch_window_ms` (default 250): how long the gateway coalesces change
  events before delivery. Raise it towards 1000 for high-volume sources;
  lower it towards 50 when end-to-end latency matters more than throughput.
- `max_inflight_batches` (default 4): how many delivered batches may await
  acknowledgement. Raising it increases throughput and weakens ordering
  guarantees across batches; within a batch, order is always preserved.
- `backpressure_mode` (default `pause`): `pause` stops reading from the
  source when the pipeline falls behind; `spill` writes overflow to local
  disk and replays it. Use `spill` only when the source cannot be paused.

Measure before and after with the gateway's `delivery_lag_seconds` metric;
tuning without that baseline is guesswork.
