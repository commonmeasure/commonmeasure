---
domain: extensions
audience: contributor
---

# Write a sidecar checker

## 1. What a sidecar does

A sidecar supplies the optional `display-integrity-model` verify stage behind
`context_show`. It estimates whether a passage supports or contradicts a
sentence. The deterministic integrity stages and the display rule still
apply. A sidecar is a processor, not a harness plugin.

The normative interface is [processor §Sidecar](../contracts/processor.md#sidecar).
Use that contract for message shapes, validation bounds and supervision
limits; this guide walks the committed example.

One process completes per invocation: hello, ordered replies, bye and exit.
The edge sends each case's passage text and one sentence, with an id. It
sends no markers, source policy, session log or rest of the reply. This is
input scoping, not filesystem isolation. The boundary is **declared, not
confined**: the edge does not sandbox sockets, filesystem access or `HOME`.
Review the script before choosing its digest.

As an author, keep input text out of all output, including identity strings,
errors and diagnostics. Use fixed `error` strings such as
`provider_unavailable`, never a provider response body. Use `detail` only
for measurements admitted by the contract's
[allow-list](../contracts/processor.md#calibration-and-the-no-text-boundary):
numeric or null measurements, numeric arrays and the specified letter enums
and probability maps. Do not add free-form explanations or unknown keys.
The edge supplies fixed answer reasons and discards stderr, including uv
diagnostics. Keep stdout exclusively for protocol messages.

The output filter catches cooperative echoes; it is not protection against
a hostile script encoding text in identifiers or numbers. Hello identity
strings are script declarations that can enter the record. Use non-empty
hello `model` and `family` identifiers of at most 64 bytes from
`[A-Za-z0-9_./:~+-]`, not starting with `/`, `~` or `.`, and a
`device` of at most 32 bytes from that set without `/` or `~`;
`p_per_chunk` and `decisions` allow at most the same object's
non-negative integer `chunks` elements (otherwise 64), with no nested
`decisions`. Never put a path, credential or earlier input in identity
strings. A missing or refused `model` or `family` fails hello with
`sidecar_hello_invalid` and `sidecar_output_dropped`; a refused `device`
is omitted with `sidecar_output_dropped` and the batch still runs.

## 2. Start in an empty repository

You need a Common Measure build with `sidecar check`, Unix process-group
supervision, `uv` on `PATH` and locally available Python 3.11 or later.
The example uses no third-party packages and makes no network calls.

In your empty checker repository, copy the two example files from your edge
checkout. Set `EDGE` to that checkout's absolute path first:

```sh
cp "$EDGE/sidecars/example/fixed_probabilities.py" .
cp "$EDGE/sidecars/example/sidecar.json" .
uv run --script fixed_probabilities.py < /dev/null
```

The hand run warms uv's cache and, with empty stdin, prints hello then bye
with zero requests. For a real checker, declare dependencies in its PEP 723
header and hand-run it once before using the edge. The edge launches uv
with `--offline --no-config`; it downloads neither dependencies nor an
interpreter. The script must be standalone: the edge runs a verified
snapshot in a temporary directory, without copying sibling modules.
Dependencies in the uv cache are not pinned by the script's digest.

Here is `sidecars/example/fixed_probabilities.py`, unchanged:

```python
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Protocol example only: deterministic hash-based probabilities, no model."""

import hashlib
import json
import sys

print(json.dumps({"hello": {
    "model": "fixed-probabilities/1", "family": "example", "device": "cpu",
    "questions": ["supported", "contradicts"],
}}), flush=True)
requests = 0
for line in sys.stdin:
    case = json.loads(line)
    # This deliberately does not judge support. It exercises both display
    # outcomes reproducibly without shipping an evaluation model.
    supported = hashlib.sha256(case["summary"].encode()).digest()[0] < 128
    print(json.dumps({
        "id": case["id"],
        "p_supported": 0.99 if supported else 0.01,
        "p_contradicts": 0.01 if supported else 0.99,
        "latency_ms": 0,
    }), flush=True)
    requests += 1
print(json.dumps({"bye": {"requests": requests}}), flush=True)
```

Read it in order:

- The four header lines declare Python and the empty dependency list to uv.
  The docstring states the example's limitation.
- The imports provide hashing, JSON and stdin; no edge Rust API is needed.
- The first `print` announces model identity, family, device and the two
  questions. `flush=True` makes hello available before reading stdin.
- `requests = 0` starts the usage count for this invocation.
- The loop parses one case per line. The `supported` assignment uses the
  first byte of the sentence's SHA-256 digest to choose an outcome. It does
  not read the passage or measure factual support.
- The reply copies the id and supplies both promised probabilities, a
  non-negative integer latency and a newline. The zero latency is a fixture
  value; a real checker measures it rather than filling unknowns with zero.
- The counter increments after each reply. EOF ends the loop; the final
  print emits bye usage and the script exits successfully.

Replace the hash choice with your checker call, retaining the lifecycle
and request order. Reply with every probability promised in hello. Do not
print progress banners or generated explanations. An item failure should
use your fixed error vocabulary; the edge records `sidecar_item_error`
instead of that string.

## 3. Declare the manifest

The copied `sidecar.json` is `sidecars/example/sidecar.json` on the edge:

```json
{
  "protocol": "commonmeasure-sidecar/1",
  "name": "fixed-probabilities",
  "version": "0.1.0",
  "stage": "verify",
  "capability": "display-integrity-model",
  "artefact": {
    "path": "fixed_probabilities.py",
    "sha256": "fde91e9a9d599b93ba74cd4131a6028885113029f38201f6fb8c21356e685bcf"
  },
  "runtime": {"command": ["uv", "run", "--script"]},
  "model": true,
  "network": false,
  "determinism": "deterministic",
  "timeout_seconds": 10,
  "questions": ["supported", "contradicts"]
}
```

Fill each member as follows, using the
[declaration rules](../contracts/processor.md#declaration-and-activation)
for exact bounds:

| Member | What to put there |
|---|---|
| `protocol` | Keep `commonmeasure-sidecar/1`; other versions are unavailable. |
| `name` | Your checker's stable identifier; this also selects its evaluation summary and egress permission. |
| `version` | Your extension version identifier. |
| `stage` | Keep `verify`. |
| `capability` | Keep `display-integrity-model`. |
| `artefact.path` | The standalone script, relative to the manifest; no absolute path or parent traversal. |
| `artefact.sha256` | The script bytes' SHA-256, as 64 lower-case hex digits without a prefix. |
| `runtime.command` | Keep `["uv", "run", "--script"]`; arbitrary launchers are not supported. |
| `model` | Keep `true`, including for this protocol fixture. |
| `network` | `false` for a local checker, or the declaration in section 6. |
| `determinism` | Declare `deterministic`, `seeded` or `non_deterministic` according to your implementation. |
| `timeout_seconds` | Your positive start-up and batch deadline, within the contract's limit. |
| `questions` | Support alone, or support then contradiction; match hello and every successful reply. |

Compute the digest after saving the script:

```sh
sha256sum fixed_probabilities.py
```

Copy only the digest into `artefact.sha256`. A changed script requires a
changed manifest. The edge verifies the bytes before launch and executes
a snapshot of those bytes. Keep the manifest beside the script; do not add
extra manifest members.

## 4. Check the lifecycle

From the directory whose source policy should apply, run:

```sh
commonmeasure sidecar check sidecar.json
```

This command verifies the declaration and script digest, checks network
admission if applicable, and exercises one fixed synthetic case through
the production supervisor. It prints filtered manifest and lifecycle
evidence, including hello, answers and bye. No threshold is needed.
A passing check establishes protocol behaviour, not model accuracy,
calibration, origin confinement or a working host integration.

On failure the command exits non-zero with named gaps. The fault scripts in
`crates/commonmeasure-cli/tests/support/sidecar_display.rs` illustrate these
cases; they are test fixtures, not separate installed commands:

| Gap | What to inspect |
|---|---|
| `sidecar_manifest_unavailable` / `sidecar_manifest_invalid` | Manifest readability, JSON, required members and allowed values. |
| `sidecar_protocol_unsupported` / `sidecar_runtime_unsupported` | Protocol version and exact runtime prefix. |
| `sidecar_artefact_unavailable` / `artefact_digest_mismatch` | Script path and saved bytes; the mismatch fixture starts nothing. |
| `sidecar_network_invalid` | Provider, origins and credential-variable declaration. |
| `processor_egress_refused` / `sidecar_credential_unset` | Effective policy term and non-empty credential in the edge's environment. |
| `sidecar_policy_unavailable` | Whether the command can load the effective source policy. |
| `sidecar_timeout_invalid` / `sidecar_configuration_invalid` | Timeout override and numeric environment settings. |
| `sidecar_process_groups_unavailable` | Platform support for process-group supervision. |
| `sidecar_start_failed` | uv/Python availability, cached dependencies, or exit before hello. Stderr is discarded. |
| `sidecar_hello_timeout` / `sidecar_batch_timeout` | Start-up or complete-batch deadline; the sleeping fixtures prove process-group termination. |
| `sidecar_protocol_malformed` | Non-JSON output, as in the malformed-output fixture. |
| `sidecar_hello_invalid` | First message, identity and promised questions. |
| `sidecar_answer_malformed` | Missing promised probabilities, out-of-range values or invalid latency. |
| `sidecar_answer_order_invalid` | Replies must retain request ids and order; the fixture returns a wrong id. |
| `sidecar_item_error` | The checker reported a fixed item error; provider text is not retained. |
| `sidecar_bye_invalid` | Missing or malformed final bye. |
| `sidecar_protocol_extra_output` | Output after bye, including diagnostic prints. |
| `sidecar_exit_failed` | Non-zero exit invalidates even otherwise valid answers. |
| `sidecar_output_dropped` | The echo fixture supplies prohibited detail; remove it rather than expecting it in the record. |
| `sidecar_output_limit` / `sidecar_batch_too_large` | Protocol output or case-count limits. |
| `sidecar_write_failed` / `sidecar_exited` | Premature child or pipe closure. |

`sidecar_check_failed` is the command's failure summary; read the lifecycle
gaps for the cause. `sidecar_cost_unknown` is an evidence gap for network
usage, not a price inferred from tokens. A check of an admitted network
manifest executes the script and can incur a provider charge; provision
access and approve that use before running it.

## 5. Run through a host and read the record

For the dependency-free example in your checker directory:

```sh
COMMONMEASURE_CHECKER="$PWD/sidecar.json" COMMONMEASURE_CHECKER_THRESHOLD=0.9 commonmeasure mcp --host pi
```

This starts the stdio edge; a host must connect to its MCP transport. For
normal use, set those variables in the environment that starts your host's
stdio edge and start a new session. See
[host integration](../integrate/host.md). The hosted edge does not activate
sidecars.

Through any connected host, fetch a release that supports display, then
call `context_show` with a cited `from_source` sentence and its passage ids.
Use the [display interface](../contracts/host-integration.md) for the input
shape. Quote-only replies or replies without scoped model cases start no
child. Ask the host to call `context_status` afterwards.

Choose calibration settings explicitly:

- `COMMONMEASURE_CHECKER_SUMMARY` names your evaluation summary;
  `COMMONMEASURE_CHECKER_TARGET` selects its false-pass target (default
  0.05). The edge reads the temperature and matching threshold under
  `checkers[manifest.name].per_set.all`.
- Without a summary, `COMMONMEASURE_CHECKER_THRESHOLD` must be in [0,1].
  The record says `uncalibrated` and uses temperature 1. This is an operator
  choice, not evidence that a false-pass target was measured.
- `COMMONMEASURE_CHECKER_CONTRADICTION_THRESHOLD` sets the raw contradiction
  threshold (default 0.9).
- `COMMONMEASURE_CHECKER_TIMEOUT_SECONDS` can shorten the manifest's hello
  and batch deadlines, never extend them.

An unavailable/invalid summary, missing target threshold or absent explicit
threshold refuses the model stage with `sidecar_summary_unavailable`,
`sidecar_summary_invalid`, `sidecar_summary_threshold_missing` or
`sidecar_threshold_required`. The edge cannot compare checker and chat-model
families because it has no host chat-model identity.

Run `commonmeasure inspect <session>` with the session's source-record
file path. The integrity stage shows sidecar identity, calibration, answers,
lifecycle gaps and egress. In the source record, inspect `detail.model` and
`detail.items`; see [sidecar model evidence](../contracts/session-evidence.md#sidecar-model-evidence).
`context_status.checker` shows configuration, digests, last hello and outcome,
and reachability without starting a checker. Reachability is unknown before
an invocation; it is not a background health probe.

A protocol failure never supplies a model confirmation or contradiction.
Unconfirmed sentences are labelled under `labelled` and withheld under
`strict`; deterministic confirmations still stand. The fixed-probability
example demonstrates those paths, not a useful judgement of your sentence.

## 6. Declare a network checker

Replace `network: false` with a provider declaration. For example, with your
own provider identifier, origin and provisioned variable:

```json
{"provider":"checker-provider","origins":["https://checker.example"],"credential_env":"CHECKER_API_KEY"}
```

Supply the matching term in the effective source policy's `constraints`:

```json
{"kind":"processor_egress","processor":"fixed-probabilities","provider":"checker-provider"}
```

Replace `processor` with your manifest's name if you renamed it. Use the
[source policy loader and scopes](../contracts/source-policy.md) to select
the policy for the directory where the edge runs. Running the check from
another directory can select a different scope. A search/fetch permission
does not admit processor egress, in any policy mode.

`credential_env` names the one credential variable passed to the script;
it does not contain the credential. Provision it only in the environment
that starts the edge, never in the manifest, command arguments or record.
The launcher clears other environment variables except its fixed runtime
allow-list. An absent term or empty credential refuses launch.

The record carries the provider, declared origins, model id, admitting term,
question digest, request/passage/sentence hashes, case count and filtered
usage. It carries no passage or sentence text from a cooperative checker;
keep credentials and text out of hello and all other output too. Missing
cost remains unknown. Candidate hashes/counts do not prove remote receipt;
bye usage is the sidecar's report.

The edge does not confine the script's sockets. Origins and `network: false`
are declarations, with `confinement: "declared"` in the evidence. The
operator's script review, pinned digest, explicit admission and source
record are the controls; this runtime does not prove which remote origin
received the text.

## 7. Keep calibration and evaluation with the extension

Your extension owns its test sets, scoring method, calibration, model
identity and evaluation results. Keep them with the script and manifest,
using licensed or synthetic fixtures. A protocol check cannot establish
accuracy. The example intentionally has no evaluation model.

The reference extension's
[checker evaluation](https://github.com/commonmeasure/trusted-news/blob/main/docs/evaluation.md)
explains its sets, operating points and calibration. That is a separate
repository, access-controlled during the pilot, not a file in this checkout.
Its summary supplies the temperature and per-target thresholds the edge
reads. Keep measured numbers there rather than copying them into the edge
guide. A summary's `calibrated` record state identifies the supplied summary;
it does not independently validate the evaluation method.

## 8. Build in your own repository

The edge is source-available and private during the pilot. The contracts
under `docs/contracts/` are CC-BY-4.0 and ship with each release, so a checker
can implement the interface without depending on the edge's Rust internals.

Keep your checker in its own repository with its standalone script,
manifest and evaluation. The trusted-news checker set is the reference
extension; the edge carries only the dependency-free protocol example.
Choose or replace a checker by its manifest, then check it and inspect a
real invocation before relying on its answers.
