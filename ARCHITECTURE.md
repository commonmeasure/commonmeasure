# Architecture

Common Measure sits between an agent and everything it reads. For each
crossing it applies the operator's rules before content moves, records what
moved and what it cost, and measures whether it helped. The definition, the
buyer and the boundary are in `PRODUCT.md`; the defined terms (crossing,
admission, manifest, evidence log, source record and the rest) are each one
line in `docs/GLOSSARY.md`.

## Core objects

**ContextJob** declares the task, the required evidence, the policy, the
budget and the outcome contract. It does not contain provider instructions.

**SupplyPlan** is a versioned graph of content acquisition and skill selection
or invocation, with fallbacks, combination rules and stop conditions.

**ModelPlan** declares the answer model, the eligible inference providers, the
fallback policy and the model budget. It stays separate from the SupplyPlan.

**ContextEnvelope** (`commonmeasure_types::ContextEnvelope`) is supply in one
normalised shape: content or a skill reference, canonical source,
provenance, declared terms, freshness/version, observed cost, token or
execution footprint and transformation history. Every adapter produces it,
and it is the only shape policy and inference ever see, so no provider's
wire format reaches a decision.

**RunManifest** freezes the job, supply plan, model, prompts, evaluator versions,
clock policy and environment required for a comparison.

**DecisionRecord** stores every planned, admitted, refused, retried and failed
crossing, with its policy reason and assurance basis.

**Evaluation** attaches measurements to one run without rewriting its evidence.

**Principal** is the authenticated identity exercising delegated authority at
a crossing. A working directory can select an engagement overlay, but it is
not an identity: an autonomous process can choose where it runs. Principal
identity therefore comes from a basis the process cannot freely rewrite, such
as its operating-system account or a launcher-held key. An environment label
alone is attribution, not authentication.

**Allowance** is a cumulative limit assigned to a principal for a declared
period. It is stateful policy, distinct from a `ContextJob`'s per-job maximum:
the edge reserves a known price atomically before purchase and reconciles
the reservation with the observed receipt.

## Components

The runtime is one binary with the components below. They share one policy
engine, one envelope shape and one evidence format.

### Host integration

`crates/commonmeasure-harness` attaches the product to a host in two ways, and they are
not equal. **Observed** capture reads lifecycle hooks after a crossing has
happened. The **mediated** tools (`context_fetch`, `context_search`) are
called instead of the host's own and can refuse before the crossing happens.
What each grade of evidence means, and why nothing merges them, is
`docs/contracts/session-evidence.md` §Crossing.

Neither needs the inference component: the model is the host's.
`commonmeasure install` registers both with Claude Code, Cursor and the
Copilot CLI, and the MCP server alone with Codex, Pi, Claude Desktop and VS
Code, whose crossings are mediated or nothing; `install chrome` registers the
binary for the browser extension in `browser/`, which observes the sources
three browser answer surfaces show (`docs/contracts/host-integration.md`);
`plugin/` is the marketplace registration for Claude Code
(`plugin/README.md`). Any host that speaks MCP can attach the
server the same way, and a Rust harness can link `commonmeasure-runtime` and
`commonmeasure-harness` directly.

Before a mediated fetch the runtime reads what the source declares about AI
use (`robots.txt` for the `CommonMeasureBot` group, an RSL licence it names)
and after it the response's own `Content-Usage` and `Link` headers; each
statement is recorded with its source, combined most-restrictive-wins, and
ruled on under the policy mode, with the operator's declared terms for a
host governing over a published preference. The prompt hook records which
URLs the user named, as hashes, and the statements pasted material carries,
so each fetch records who named the source. Discovery of the source's
Content Telemetry manifest follows the fetch and never delays it
(`docs/contracts/session-evidence.md` §Source declarations, §Prompt sources,
§Manifest discovery).

Session evidence is `docs/contracts/session-evidence.md`. The operator's
standing policy for a session uses the same `Constraint` vocabulary and the
same decision functions as a job, so there is one policy engine. Resolution
order for a session: the principal first, from a basis the process cannot
rewrite; then the policy scope matched by working directory, which names
the governing engagement recorded on each mediated crossing and holds the
telemetry clearance the relay reads. The governing engagement is distinct
from the reported engagement the console counts work under; the rules for
both are `DECISIONS.md` §Session policy and egress and the terms are
`docs/GLOSSARY.md`.

### Decision

The router receives a ContextJob and the available provider capabilities. It
filters out ineligible routes, checks the job maximum, reserves any applicable
principal allowance and executes a SupplyPlan. The allowance ledger stays at
the edge and serialises concurrent reservations from the same principal. A
strict monetary allowance cannot authorise an unknown price or an incomparable
currency; provider-native units remain separate unless policy declares a limit
in that unit. The router is deterministic. A later optimiser may rank eligible
plans, but its objective and features must be inspectable and versioned.

### Supply

Adapters implement individual capabilities, not one universal endpoint:

```text
search(query, constraints) -> candidates
fetch(candidate, constraints) -> context envelope
query(corpus, objective, constraints) -> context envelope
quote(request) -> commercial terms       [when supported]
discover_skills(requirement, constraints) -> skill candidates
invoke_skill(candidate, input, constraints) -> result envelope
```

A provider may implement only a subset, and an adapter declares only what it
implements, never what its vendor documents. `crates/commonmeasure-supply` holds
fifteen provider adapters, named in `README.md` §What it does: `search` for
the twelve open-web providers, six of which also declare `fetch` (a named
URL, dispatched only when a job declares a `fetch_target`); `query` for the
operator's own internal corpus; `search` and `quote` for Redpine, a licensed
supplier bought by quote then confirm; and `search` and `fetch` for Ozone
Live, retrieval over a licensed publisher corpus. The skill adapter, `invoke` for catalogued local skills,
is separate from the provider list. No other
capability is declared. The runtime validates a plan against the declared
capabilities before execution, so a plan cannot discover during execution
that it planned an operation nobody offers. Each adapter's verification
state is `docs/knowledge-base/provider-verification.md`.

### Inference

An external gateway provides an OpenAI-compatible API, a provider catalogue,
credentials, fallback and low-level load balancing. TensorZero is the first
gateway (`DECISIONS.md` §Product), run as a local sidecar through its
OpenAI-compatible Chat Completions endpoint, with the committed configuration
pinning one model snapshot to one provider route and no hidden fallback
(`demo/gateway/tensorzero/`). Common Measure sits above that machinery: it
constrains the ModelPlan, supplies the context envelope, records the exact
route executed, and joins model cost, latency and output to the context plan.

`commonmeasure-inference` provides this small interface. It records what the gateway
said it executed, never what was requested, and marks every field the gateway
omitted as unknown. When no gateway is configured there is no backend and no
answer: an unconfigured component produces an explicit unavailable result,
with no silent fallback.

The gateway's adaptive routing optimises inference-provider performance.
Common Measure optimises job outcomes across context and model choices. The
two loops may later exchange signals, but they remain separately observable
and governable.

### Evidence

Append-only local records hold the job, plan, crossing, policy, price,
content or skill identity, provenance, token or execution footprint, latency,
gaps and evaluation references. Raw provider responses are sealed beside the
run so a reviewer can re-derive every content hash from the bytes that
produced it; they are kept out of git apart from one stated exception
(`docs/contracts/run-output.md` §Directory). A failed write becomes an
explicit gap record, written by the next successful write, and a run is
published atomically or not at all (`docs/FAIL-POLICY.md`).

Eight add-on processors run in-process (`docs/contracts/processor.md`
§Status). Five run at the crossings the runtime carries, all deterministic: a PII detector, an injection screen and a
support-status governor at the admit stage, and an HTML text extractor and a
context optimiser at the transform stage. The extractor runs on every
mediated fetch that received a body, before the admit screens, so the
screens rule on the text the agent would read; its record carries the hash
of the bytes the origin served beside the hash of the text delivered, and
the crossing carries both. The governor runs only when a job declares a `governance`
block, a sealed support-status rule set and entitlement grant
(`docs/contracts/run-output.md` §Manifest), demonstrated by the governed
specialist slice (`demo/specialist/README.md`). Each invocation writes its
own evidence record (processor identity and configuration digest,
input/output hashes and token counts, decision and method, assurance basis
and blind spots: `docs/contracts/processor.md`), and findings stay
namespaced to the processor that produced them, never normalised into a
universal score.

### Experiment

The runner creates comparable manifests, executes the matrix and keeps each
plan's execution from contaminating another's
(`docs/contracts/experiment.md`). Provider caches, freshness windows, retries
and concurrency are explicit experimental variables. Evaluators run after
evidence is sealed.

When an experiment varies anything downstream of acquisition, the supply
itself is held fixed by replaying recorded provider responses through the
same adapters, parsers, policy and evidence storage a live run uses
(`commonmeasure run --replay <dir>`, `docs/contracts/run-output.md`). Inference
is never replayed, so an answer can only come from a real gateway call at run
time.

### Console and relay

`commonmeasure serve` (`crates/commonmeasure-console`) serves the console on loopback: a
maud-rendered app shell (`crates/commonmeasure-console/src/console/app.rs`) with five
sections rendered server-side over an SQLite index derived from the session
evidence logs, from the same JSON values the `/api/*` routes serve, with
vendored htmx as the only client script. A published run directory is read
with `commonmeasure inspect`, not served. The logs are authoritative: the index
can be rebuilt from them, and witnessed and reconstructed evidence stay
separate in every aggregate. Attribution rules
(`~/.commonmeasure/attribution.json`) are applied at query time as a projection
over the unchanged store. The Compare section runs live provider searches on
the operator's submission and records nothing. The Policy section edits the
policy mode, a scope's denied hosts and the attribution rules, each
revision-checked and saved through the artefact's own loader, and forecasts
a draft rule over the recorded history with the runtime's own admission
check before any save; what the console may and may not write is
`DECISIONS.md` §Execution and evidence. The console is the one component
that reads both engagement identities, so its Policy section names every
scope where they differ.

The relay (`crates/commonmeasure-relay`, driven by `commonmeasure relay`) builds a
purpose-limited projection at the operator boundary: witnessed retrieval and
grounding facts only, as wire types proven against the schemas pinned in
`schema/` (pinned copies from the standard's own repository, consumed, not
forked). Projected batches are spooled durably before any delivery attempt
and delivered only to an explicitly configured receiver; event identity is
derived from the evidence record each event projects, so redelivery after a
crash cannot double-count. No console row is assumed safe for egress:
reconstructed crossings, refusals and rejected sources are never projected,
and of the refusals only a count per session crosses, as an integer on the
batch (`docs/contracts/session-evidence.md` §The refused count on the wire).
The console's egress block, and the Overview's hub card that renders it,
report the configured receiver and delivered counts, or the absence of
both, and the enrolled key id with its standing.

Enrolment (`commonmeasure connect`, `crates/commonmeasure-relay/src/enrolment.rs`)
is how an edge joins the hub in one command: it mints an Ed25519 key pair,
exchanges the owner's short-lived token for an org-scoped ingest key and
registers the public key in the same call, with a proof of possession. The
private key stays in `~/.commonmeasure/edge-key.json`; the ingest key is
written into `relay.json` without passing through a shell; the public
facts, the hub, the organisation, the key id the hub assigned, go to
`enrolment.json`, from which every session records an `edge_identity`
line. `connect` also signs and uploads the directory proof that lists the
key in the hub's key directory, and what the hub then holds goes to
`directory-listing.json`, a file of its own so that `enrolment.json` keeps
the shape released binaries sharing the home read. Each relay run asks the
hub for the key's standing, records a revocation and renews the proof once
it is a day old; `commonmeasure disconnect` revokes both credentials at the
hub and removes the four files.

The relay's own report states which clearance was used when events left. It
names each governing engagement whose declared clearance let events leave,
with that engagement's delivered count; it states the sessions withheld
because nothing in them was cleared; and it reports separately two absences
that a single count would hide: a published run, which carries no engagement
to be cleared by, and a batch spooled by an earlier invocation, whose
clearance this run never resolved. It names only the governing identity,
because egress is enforced at capture time; the reported engagement is a
read-time projection that can be re-edited afterwards.

## What runs where

The edge is one static binary and files under the operator's home directory.
No daemon and no server:

- `install.sh` places a released, checksum-verified binary on `PATH`, or
  `cargo install --path crates/commonmeasure-cli` builds one from a checkout
  (`docs/RELEASE.md`); the `justfile` is the operator surface.
- `commonmeasure install <host>` writes the host's registration naming the
  binary by absolute path, `doctor` reads it back and `uninstall` removes
  it; the plugin declares the same hooks and MCP server for the marketplace
  route (`plugin/README.md`), and the standalone archive from
  `plugin/package.sh` installs with no repository and no toolchain.
- The console serves on loopback only; the inference gateway is a sidecar
  launched by image digest (`demo/gateway/tensorzero/`).
- State is `~/.commonmeasure/` plus the append-only evidence logs. Nothing
  leaves the machine by default; egress exists only toward an explicitly
  configured receiver, spooled durably first.

The hosted tier, Common Measure Hub, is one multi-tenant service run by
Common Measure Ltd at hub.commonmeasure.ai, which is also the identity
origin in every enrolled edge's signature. Whether an organisation may run
a hub of its own is an open decision (`DECISIONS.md` §Open decisions).
The split between edge and hub is fixed by the architecture:

- **Decisions stay at the edge.** The runtime refuses before bytes reach the
  model. A remote hop would add latency and a trust dependency to every
  crossing, so the runtime stays beside the agent wherever it runs.
- **Evidence goes up.** The relay spools durably and derives event identity
  from the record each event projects. The hub accepts the Content Telemetry
  v1.0 wire under an organisation-scoped ingest key and shows an
  organisation its fleet's evidence. It sees the projection, never the
  operator record, so the privacy floor survives the move. This is built.
- **Policy comes down.** The hub distributes a policy file in a signed
  envelope, and the edge applies it through the one policy engine that
  exists (`docs/contracts/policy-envelope.md`). The edge accepts it only in
  a deployment mode chosen locally: a managed edge pins the trusted signer,
  validates the signature, the binding and the schema through the ordinary
  loader, refuses rollback and expiry, and keeps the last-known-good policy
  across every failure; a local edge makes no management request; an
  invalid or unreachable hub never turns strict policy into observe. The
  edge refreshes the envelope itself at session start and before relay,
  and an expired one keeps enforcing while the record says it is stale. The
  edge side is built. The edge authenticates to the hub's endpoint by
  signing the request with its enrolled key, the same signature the
  mediated fetch presents to a publisher (`ROADMAP.md` §Verified fetcher
  identity); an edge
  with no key to sign with makes no request and records why.

Fleet status travels on a management contract separate from Content
Telemetry (`docs/contracts/fleet-status.md`). It reports the edge's key id,
the deployment mode, the desired and applied revisions, the digest of the
declared policy and the identity of the canonical effective policy,
software and resolver versions, principal identity and authentication
basis, the last enforcement time and a bounded allowance summary. It does
not put policy detail into the purpose-limited telemetry projection. A
self-reported digest establishes configuration drift evidence, not
hardware-backed proof of enforcement.

What the edge and the hub each own is `PRODUCT.md` §Edge and hub; the rule
that fixes the boundary is `DECISIONS.md` §Product.

## Selection objective

Three deterministic evaluators run per plan: grounding checks that the
answer's citations appear in the content the model saw, coverage measures the
admitted window against a rubric the job declared, and freshness measures
declared dates against an as-of reference the job declared
(`docs/contracts/run-output.md` §Plans). Coverage and freshness are unitless
fractions, so a weighted objective over exactly those terms is computable:
arithmetic on the declared weights, with the rubric identity named in the
selection. The router ranks on it. Everything else abstains, naming what it
lacked: no measure of answer quality exists (the fidelity verifier and judge
record per-claim support and their agreement beside the plan,
`docs/contracts/processor.md` §Status, and are not mapped onto quality),
and a fraction has no exchange rate against a currency or a millisecond. Substituting the terms a
run does have would publish a provider verdict it did not measure.

Weights, hard constraints and evaluator versions belong to the operator and
are recorded. Hard constraints filter before scoring; policy is a gate, not a
weighted term. Missing measurements increase uncertainty
(`docs/FAIL-POLICY.md` §7).

Model comparison is not built. When it is, the candidate is a joint plan
`(p, m)`: the runner estimates context-plan and model-plan effects separately
before testing interactions, so the console can explain why a route won.

## Content Telemetry boundary

Content Telemetry is the interoperable projection format, not the complete
operator record and not the optimiser. It supports the reporting some
licensed suppliers require while the local source record stays the source of
truth.

The projection emitted is retrieval and grounding, as Content Telemetry v1.0
(`schema/SOURCE.md`, `crates/commonmeasure-relay/tests/conformance.rs`). The relay's
wire types (`crates/commonmeasure-relay/src/wire.rs`) have no variant for citation,
display, reproduction or engagement, because the evidence log witnesses
nothing for them. Evidence-backed reproduction and citation events, drawn
from the grounding evaluator's verdicts, are a planned capability
(`ROADMAP.md` §Telemetry projection of evaluator evidence). The boundary rules
hold for them: the evaluator's verdict vocabulary stays local, and only
positive claims the sealed evidence supports are projected.

The standard never receives the full prompt, the response, the supply
alternatives, the commercial objective, evaluator output, the workforce trace
or rejected private sources, unless a separate contract explicitly requires
them. It does receive the name of the supplier that served an admitted
source, as a namespaced custom field, and nothing about what the supplier
charged, and the number of crossings policy refused in the session, as an
integer on the batch and nothing else about them (`DECISIONS.md` §Session
policy and egress).

## Security and credentials

- adapter credentials are scoped per provider and environment;
- no secret appears in run manifests, logs or repository files;
- the model never receives raw credentials;
- provider calls originate in the mediated runtime;
- operator policy sets egress separately from capture;
- live paid calls require explicit budgets and idempotency.
