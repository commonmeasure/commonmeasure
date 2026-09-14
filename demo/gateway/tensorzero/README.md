# TensorZero demo sidecar

This is the gateway configuration used by the gateway acceptance run. It pins
one OpenAI model snapshot behind one named provider route. There is no second
provider, variant, fallback, relay or adaptive router. TensorZero's
pseudonymous usage analytics is disabled, and its optional database-backed
observability is not enabled.

The exact image identity and observed gateway version are in `image.json`.
`OPENAI_API_KEY` stays in the ignored repository-root `.env`; Docker receives
only that variable.

From the repository root, start the sidecar with (also `just gateway-up`):

```sh
set -a; . ./.env; set +a
docker run --name commonmeasure-tensorzero --detach --env OPENAI_API_KEY --publish 127.0.0.1:3000:3000 --volume "$PWD/demo/gateway/tensorzero:/app/config:ro" tensorzero/gateway@sha256:c939db4f27e41aa3a37a87909430a0c8f898f9c952f402dcbdc17e1708597bcb --config-file /app/config/tensorzero.toml
```

Launching by digest is the pin: a mutable tag like `:latest` can resolve to
a different image for two operators without either noticing, and a
different digest is a different integration input, not a reproduction of this
run. The digest is the one `image.json` records from the acceptance run.

The run uses
`http://127.0.0.1:3000/openai/v1/chat/completions` as
`COMMONMEASURE_INFERENCE_ENDPOINT`.

The suite behind the committed acceptance run is
`demo/jobs/energy-price-cap-internal.json`, which compares the operator's own
corpus against the open web on one question; with no `--live` authorisation
the open-web plan reports `unavailable` and the corpus plan answers.
`just internal-corpus-example` republishes it to `demo/output/tensorzero/`,
with `COMMONMEASURE_INTERNAL_CORPUS=demo/corpus` naming the corpus the query
plan reads and the gateway above answering.
