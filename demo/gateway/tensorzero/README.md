---
domain: edge
audience: contributor
---

# TensorZero demo sidecar

The reference inference gateway for the demonstration runs. `tensorzero.toml`
pins one OpenAI model snapshot (`gpt-4o-mini-2024-07-18`) behind one named
provider route. There is no second provider, variant, fallback, relay or
adaptive router. TensorZero's pseudonymous usage analytics is disabled, and
its optional database-backed observability is not enabled.

`image.json` records the image digest and the gateway version observed when
it was pinned.

From the repository root, with `OPENAI_API_KEY` in the ignored
repository-root `.env`:

```sh
just gateway-up
```

The recipe runs the image by digest with docker or podman, publishes it on
`127.0.0.1:3000` and mounts this directory read-only as the configuration.
The container receives `OPENAI_API_KEY` and no other line of `.env`: the
recipe copies that one line to `.env.gateway` (ignored, mode 600) and passes
it as the container's environment file. `just gateway-down` removes the
container and `.env.gateway`.

Launching by digest is the pin: a mutable tag like `:latest` can resolve to
a different image for two operators without either noticing, and a
different digest is a different integration input, not a reproduction of a
recorded run. The digest in the `justfile` is the one `image.json` records.

Runs use `http://127.0.0.1:3000/openai/v1/chat/completions` as
`COMMONMEASURE_INFERENCE_ENDPOINT`.

With that variable exported, `just internal-corpus-example` runs
`demo/jobs/energy-price-cap-internal.json` through this gateway, with `COMMONMEASURE_INTERNAL_CORPUS=demo/corpus` naming
the corpus the query plan reads. The suite compares the operator's own corpus
against the open web (Tavily) on one question; without `--live`, which the
recipe does not pass, the open-web plan reports `unavailable` and the corpus
plan answers. The recipe writes to `output/tensorzero` under
`COMMONMEASURE_PRIVATE_EVIDENCE` and refuses to run without it
(`CONTRIBUTING.md`).
