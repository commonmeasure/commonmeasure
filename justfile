# Common Measure command shortcuts (the binary is `commonmeasure`). `just` alone lists them.
#
# Prerequisites: rustc 1.97 or later (Cargo.toml `rust-version`), and one
# online `cargo fetch` before the offline gates below. Optional, per recipe:
# docker (gateway-up), python3 (demo-arc, routing-*), cargo-zigbuild (plugin
# cross targets), tailscale (serve-tailnet).
#
# Recipes never read .env unless the recipe says so; credentials stay in the
# launching shell (docs/GETTING-STARTED.md §4). Recipes marked "gateway"
# expect COMMONMEASURE_INFERENCE_ENDPOINT in the environment, e.g.
# http://127.0.0.1:3000/openai/v1/chat/completions after `just gateway-up`.

default:
    @just --list

# Download the dependency crates once; every other recipe runs offline
fetch:
    cargo fetch

# The definition-of-done gates: formatting, clippy, tests
gates: fmt clippy test

test:
    cargo test --workspace --offline

fmt:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets --offline -- -D warnings

# Publish the no-credential run to a scratch directory and print its account
run-empty out="/tmp/commonmeasure-empty":
    cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output {{out}}
    cargo run -p commonmeasure-cli -- inspect {{out}}

# Regenerate the committed rubric-ranked replay example (offline: no gateway needed)
rubric-example:
    cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay-rubric.json --replay demo/recon --output demo/output/latest
    cargo run -p commonmeasure-cli -- inspect demo/output/latest

# Replay the recorded providers to a scratch directory (no credentials needed)
replay out="/tmp/commonmeasure-replay":
    cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay.json --replay demo/recon --output {{out}}
    cargo run -p commonmeasure-cli -- inspect {{out}}

# Regenerate the committed replay example (gateway: its plans carry answers)
replay-example:
    cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay.json --replay demo/recon --output demo/output/replay
    cargo run -p commonmeasure-cli -- inspect demo/output/replay

# Regenerate the committed cited-run example (gateway: the suite requires citations)
cited-example:
    #!/usr/bin/env sh
    set -e
    test -n "${COMMONMEASURE_INFERENCE_ENDPOINT:-}" || { echo "this recipe needs COMMONMEASURE_INFERENCE_ENDPOINT; without it every plan publishes as unavailable and replaces a committed run with a degraded one. Run just gateway-up and export it (justfile header)" >&2; exit 1; }
    cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay-cited.json --replay demo/recon --output demo/output/cited
    cargo run -p commonmeasure-cli -- inspect demo/output/cited

# Produce a labelled run under the demonstration identity; not committed (gateway; demo/provenance/README.md)
provenance-example:
    COMMONMEASURE_PROVENANCE_CERTIFICATE=demo/provenance/signer.pem COMMONMEASURE_PROVENANCE_KEY=demo/provenance/signer-key.pem cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay-provenance.json --replay demo/recon --output demo/output/provenance
    cargo run -p commonmeasure-cli -- inspect demo/output/provenance
    cargo run -p commonmeasure-cli -- provenance demo/output/provenance/provenance/exa.txt

# Regenerate the committed run over the operator's own corpus (gateway)
internal-corpus-example:
    COMMONMEASURE_INTERNAL_CORPUS=demo/corpus cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap-internal.json --output demo/output/tensorzero
    cargo run -p commonmeasure-cli -- inspect demo/output/tensorzero

# Regenerate the committed injection-screen run (offline; demo/injection/README.md)
injection-example:
    COMMONMEASURE_INTERNAL_CORPUS=demo/injection/corpus cargo run -p commonmeasure-cli -- run demo/jobs/injection-screen.json --output demo/output/injection
    cargo run -p commonmeasure-cli -- inspect demo/output/injection

# Regenerate the demonstration evidence home demo/arc/home (offline; docs/RUN-THE-DEMONSTRATION.md)
demo-arc:
    sh demo/arc/regenerate.sh

# Regenerate the committed specialist comparison runs (gateway; demo/specialist/README.md)
specialist-examples:
    #!/usr/bin/env sh
    set -e
    test -n "${COMMONMEASURE_INFERENCE_ENDPOINT:-}" || { echo "this recipe needs COMMONMEASURE_INFERENCE_ENDPOINT; without it every plan publishes as unavailable and replaces a committed run with a degraded one. Run just gateway-up and export it (justfile header)" >&2; exit 1; }
    for suite in demo/jobs/specialist/*.json; do
        name=$(basename "$suite" .json)
        COMMONMEASURE_INTERNAL_CORPUS=demo/specialist/corpus cargo run -p commonmeasure-cli -- run "$suite" --output "demo/output/specialist/$name"
    done
    cargo run -p commonmeasure-cli -- inspect demo/output/specialist/governed-restraint

# Regenerate the committed commerce comparison runs (gateway; demo/commerce/README.md)
commerce-examples:
    #!/usr/bin/env sh
    set -e
    test -n "${COMMONMEASURE_INFERENCE_ENDPOINT:-}" || { echo "this recipe needs COMMONMEASURE_INFERENCE_ENDPOINT; without it every plan publishes as unavailable and replaces a committed run with a degraded one. Run just gateway-up and export it (justfile header)" >&2; exit 1; }
    for suite in demo/jobs/commerce/*.json; do
        name=$(basename "$suite" .json)
        case "$name" in
            product-*)   corpus=demo/commerce/corpus/product-data ;;
            brand-*)     corpus=demo/commerce/corpus/brand-content ;;
            editorial-*) corpus=demo/commerce/corpus/third-party ;;
            *)           corpus=demo/commerce/corpus ;;
        esac
        COMMONMEASURE_INTERNAL_CORPUS="$corpus" cargo run -p commonmeasure-cli -- run "$suite" --output "demo/output/commerce/$name"
    done
    cargo run -p commonmeasure-cli -- inspect demo/output/commerce/governed-recommendation

# Run the routing experiment's fitting set (offline; demo/jobs/commerce-routing/README.md)
routing-fitting-runs:
    #!/usr/bin/env sh
    set -e
    fitting=$(python3 -c "import json; s=json.load(open('demo/jobs/commerce-routing/split.json'))['split']; print(' '.join(sorted(q for c in s.values() for q in c['fitting'])))")
    for question in $fitting; do
        for condition in product brand editorial mixed; do
            case "$condition" in
                product)   corpus=demo/commerce/corpus/product-data ;;
                brand)     corpus=demo/commerce/corpus/brand-content ;;
                editorial) corpus=demo/commerce/corpus/third-party ;;
                mixed)     corpus=demo/commerce/corpus ;;
            esac
            suite="demo/jobs/commerce-routing/$condition-$question.json"
            COMMONMEASURE_INTERNAL_CORPUS="$corpus" cargo run -p commonmeasure-cli -- run "$suite" --output "demo/output/commerce-routing/$condition-$question"
        done
    done
    cargo run -p commonmeasure-cli -- inspect demo/output/commerce-routing/mixed-grinder-choice

# Run the routing experiment's holdout set (offline; needs the frozen rule.json)
routing-holdout-runs:
    #!/usr/bin/env sh
    set -e
    test -f demo/jobs/commerce-routing/rule.json || { echo "rule.json is not frozen; the holdout does not run before the rule" >&2; exit 1; }
    holdout=$(python3 -c "import json; s=json.load(open('demo/jobs/commerce-routing/split.json'))['split']; print(' '.join(sorted(q for c in s.values() for q in c['holdout'])))")
    for question in $holdout; do
        for condition in product brand editorial mixed; do
            case "$condition" in
                product)   corpus=demo/commerce/corpus/product-data ;;
                brand)     corpus=demo/commerce/corpus/brand-content ;;
                editorial) corpus=demo/commerce/corpus/third-party ;;
                mixed)     corpus=demo/commerce/corpus ;;
            esac
            suite="demo/jobs/commerce-routing/$condition-$question.json"
            COMMONMEASURE_INTERNAL_CORPUS="$corpus" cargo run -p commonmeasure-cli -- run "$suite" --output "demo/output/commerce-routing/$condition-$question"
        done
    done
    cargo run -p commonmeasure-cli -- inspect demo/output/commerce-routing/product-g4-presto200

# Regenerate the committed licensed-fetch replay (offline: no gateway needed)
redpine-replay-example:
    cargo run -p commonmeasure-cli -- run demo/jobs/nvidia-redpine-replay.json --replay demo/recon --output demo/output/redpine-replay
    cargo run -p commonmeasure-cli -- inspect demo/output/redpine-replay

# Regenerate the committed skill run (executes local skills; nothing billable; demo/skills/README.md)
skills-example:
    COMMONMEASURE_SKILL_CATALOGUE=demo/skills/catalogue.json COMMONMEASURE_INTERNAL_CORPUS=demo/corpus cargo run -p commonmeasure-cli -- run demo/jobs/skill-publishable.json --live --output demo/output/skills
    cargo run -p commonmeasure-cli -- inspect demo/output/skills

# Live provider comparison to the gitignored live directory (sources .env; billable; gateway)
run-live:
    set -a; . ./.env; set +a; cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --live --output demo/output/live
    cargo run -p commonmeasure-cli -- inspect demo/output/live

# Print the run dossier (docs/READ-A-RUN.md)
inspect run="demo/output/replay":
    cargo run -p commonmeasure-cli -- inspect {{run}}

# Serve the operator console on http://127.0.0.1:4173
serve:
    cargo run -p commonmeasure-cli -- serve

# Serve the operator console to the whole tailnet on http://<tailscale-ip>:4173.
# The console carries no credential, so this publishes every recorded URL,
# host, cwd and session to every device on the tailnet; the binary prints the
# same warning before serving. A loopback bind answers only to loopback names
# (HostGuard, crates/commonmeasure-console/src/serve.rs), so `tailscale serve` in front of a
# loopback console does not work.
serve-tailnet:
    cargo run -p commonmeasure-cli -- serve --listen "$(tailscale ip -4):4173" --allow-remote

# Start the reference TensorZero sidecar (sources .env; see demo/gateway/tensorzero/README.md)
gateway-up:
    #!/usr/bin/env sh
    set -e
    engine=$(command -v docker || command -v podman) || { echo "gateway-up needs docker or podman" >&2; exit 1; }
    # The container reads one key and no other. Passing the whole of .env would
    # hand a model gateway every supplier credential the file holds; `:z`
    # relabels the mount for SELinux, which podman needs and docker accepts.
    umask 077
    grep '^OPENAI_API_KEY=' .env > .env.gateway
    "$engine" run --name commonmeasure-tensorzero --detach --env-file "$PWD/.env.gateway" --publish 127.0.0.1:3000:3000 --volume "$PWD/demo/gateway/tensorzero:/app/config:ro,z" tensorzero/gateway@sha256:c939db4f27e41aa3a37a87909430a0c8f898f9c952f402dcbdc17e1708597bcb --config-file /app/config/tensorzero.toml

gateway-down:
    #!/usr/bin/env sh
    engine=$(command -v docker || command -v podman) || exit 0
    "$engine" rm -f commonmeasure-tensorzero
    rm -f .env.gateway

# Install the CLI on PATH (rerun after changes; the installed binary does not track the tree)
install:
    cargo install --path crates/commonmeasure-cli

# Build the binaries the standalone plugin archive bundles (plugin/README.md)
plugin:
    sh plugin/build.sh
