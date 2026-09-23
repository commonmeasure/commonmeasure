# Vendored schema, pinned

These are pinned, read-only copies of the **Content Telemetry v1.0** JSON
Schemas. The standard is maintained outside Common Measure, in the
`SPUR-Coalition/telemetry` repository served at contenttelemetry.org, and is
consumed, not forked: nothing in this directory may be edited locally, and no
Common Measure type may add or redefine a wire field the standard does not carry.

| File | Upstream `$id` |
|---|---|
| `telemetry-session.v1.json` | `https://contenttelemetry.org/schema/v1/telemetry-session.json` |
| `telemetry-event.v1.json` | `https://contenttelemetry.org/schema/v1/telemetry-event.json` |
| `telemetry-event-batch.v1.json` | `https://contenttelemetry.org/schema/v1/telemetry-event-batch.json` |
| `manifest.v1.json` | `https://contenttelemetry.org/schema/v1/manifest.json` |

`manifest.v1.json` is the discovery manifest schema
(`/.well-known/content-telemetry.json`, standard section 8). The standard's
own manifest fixtures are vendored beside it, unedited, under
`manifest-tests/valid/` and `manifest-tests/invalid/`; the relay's conformance
test validates each against the schema, and the runtime's manifest reader
(`crates/commonmeasure-harness/src/manifest.rs`) is held to the same fixtures,
including the invalid ones that pass the schema and fail only the consumer
rules of section 8.7.

## Pin

Copied 2026-09-02 from a checkout of the `SPUR-Coalition/telemetry` repository
at commit `2df2240348290e121ff26266f04b7c9c8f9c10ab` (`origin/main`). The event and
batch schemas are byte-identical to tag `v1.0`; the session schema carries the
post-release review fix (`ac18250`) and is byte-identical to the copy served at
its `$id` URL (SHA-256
`4c890ebb9b4e75f4caaf1f2862605a4ecd6b51cf8fd3035a1d403e1a58820d0b`, fetched
2026-09-02). The pinned version is therefore **v1.0** as published.

The manifest schema and the manifest fixtures were copied from the same
repository at commit `a2c4fda390978dd40dca059d4e5cd36cbf1e4ac6`; the schema
file was last changed there in `ac18250` (SHA-256
`9653456a2ebefcc74ae406edb68f0f8f6e011795b5e957b51ddd6f975d846497`).

To refresh against a newer upstream version:

```sh
TELEMETRY_CHECKOUT=/path/to/SPUR-Coalition/telemetry
for f in telemetry-session telemetry-event telemetry-event-batch manifest; do
  cp "$TELEMETRY_CHECKOUT/$f.json" schema/$f.v1.json
done
cp "$TELEMETRY_CHECKOUT"/tests/valid/manifest-*.json schema/manifest-tests/valid/
cp "$TELEMETRY_CHECKOUT"/tests/invalid/manifest-*.json schema/manifest-tests/invalid/
git -C "$TELEMETRY_CHECKOUT" rev-parse HEAD   # record here
```

then update this file, the wire types in `crates/commonmeasure-relay/src/wire.rs`, and the
conformance test in `crates/commonmeasure-relay/tests/conformance.rs` in the same change.
The conformance test validates every projected document against these files with
no network access, so type drift is caught in CI.

The schemas define what a valid document is; the golden corpus in
`conformance/` (repo root) defines what this relay emits, and the hub replays
a pinned copy of it into its real receiver. `conformance/README.md` describes
the two gates.
