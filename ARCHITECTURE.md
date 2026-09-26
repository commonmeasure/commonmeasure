---
domain: edge
audience: contributor
---

# Architecture

Common Measure mediates the acquisition paths integrated by the operator,
applies source policy before admission, and records origin and cost. Hooks
observe supported host activity after the event; transcript imports
reconstruct earlier activity. The record keeps those grades and unavailable
evidence explicit. Batch runs measure outcomes against their recorded inputs.
The defined terms (crossing, admission, manifest, evidence log, source
record and the rest) are each one line in `docs/GLOSSARY.md`.

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
ChatGPT on the web and Bing Copilot Search show (Google AI Overviews is
`planned` and records nothing: `docs/contracts/host-integration.md` §6);
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
from the reported engagement the console counts work under: the governing
engagement is fixed when the crossing is recorded and decides egress, while
the reported one is a read-time projection that can be re-edited. The terms
are `docs/GLOSSARY.md`.

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
sixteen provider adapters (`docs/contracts/provider.md`): `search` for
the twelve open-web providers, six of which also declare `fetch` (a named
URL, dispatched only when a job declares a `fetch_target`); `query` for the
operator's own internal corpus; `search` and `quote` for Redpine, a licensed
supplier bought by quote then confirm; `search` and `fetch` for Ozone
Live, retrieval over a licensed publisher corpus; and `search` alone for
Dataville, returning one Wikipedia or arXiv record per request. The skill
adapter, `invoke` for catalogued local skills, is separate from the provider
list. No other capability is declared. The runtime validates a plan against the declared
capabilities before execution, so a plan cannot discover during execution
that it planned an operation nobody offers. Each adapter's verification
grade is in `docs/contracts/provider.md`.

### Inference

An external gateway provides an OpenAI-compatible API, a provider catalogue,
credentials, fallback and low-level load balancing. TensorZero is the first
gateway, run as a local sidecar through its
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
§Status). Five run at the crossings the runtime carries: a PII detector, an
injection screen and a support-status governor at the admit stage, and an
HTML text extractor and a context optimiser at the transform stage. The
fidelity verifier and judge run after inference and the output provenance
labeller on the answer. The extractor runs on every
mediated fetch that received a body, before the admit screens, so the
screens rule on the text the agent would read; its record carries the hash
of the bytes the origin served beside the hash of the extracted text, and
the crossing carries both. A fetch result may carry only a part of that
text; the crossing's `delivered` names the part. The governor runs only when a job declares a `governance`
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

### Local artifact records

`commonmeasure artifact` is a thin CLI over `commonmeasure-runtime::artifact`.
The runtime stores immutable identity, association and snapshot JSON plus explicitly
selected evidence bytes. File snapshots hash all saved bytes; Git snapshots bind
a fixed tree's raw blob inventory with fixed provenance exclusions. Portable JSON
bundles contain the records and selected evidence, and the verifier requires an
explicit file or Git target. No network path, signing provider or Hub is required.
The [artifact contract](docs/contracts/artifact-association.md) defines hash scopes,
resource bounds and the limits of unsigned declarations.

### Console and relay

`commonmeasure serve` (`crates/commonmeasure-console`) serves the console on loopback: a
maud-rendered app shell (`crates/commonmeasure-console/src/console/app.rs`) with seven
sections, rendered server-side over an SQLite index derived from the session
evidence logs, from the same JSON values the `/api/*` routes serve, with
vendored htmx as the only client script. The logs are authoritative: the
index can be rebuilt from them, and witnessed and reconstructed evidence stay
separate in every aggregate. Attribution rules
(`~/.commonmeasure/attribution.json`) are applied at query time as a projection
over the unchanged store. The console's writes are the policy mode, a scope's
denied hosts and the attribution rules, each revision-checked and saved
through the artefact's own loader, and the record of each Compare
comparison, kept in `<home>/comparisons/` outside the relay's session scan.
It cannot start a run or modify existing evidence, and it sends no record
anywhere. It is the one component that reads both engagement identities, so
its Policy section names every scope where they differ. A published run
directory is read with `commonmeasure inspect`, not served. What each
section shows, its routes and its answers are `docs/CONSOLE.md`.

The relay (`crates/commonmeasure-relay`, driven by `commonmeasure relay`) builds a
purpose-limited projection at the operator boundary: witnessed retrieval and
grounding facts only, as wire types proven against the schemas pinned in
`schema/` (pinned copies from the standard's own repository, consumed, not
forked). Projected batches are spooled durably before any delivery attempt
and delivered only to an explicitly configured receiver, which may be scoped
to named suppliers so a supplier's own telemetry server receives its events
and nothing else (`relay.json` `suppliers`, a local narrowing of what the
supplier's grant allows); event identity is
derived from the evidence record each event projects, so redelivery after a
crash cannot double-count. The relay persists a claim before each HTTP
attempt, applies bounded retry deadlines (ten attempts, 60 seconds doubling
to an hour) and retains exhausted batches for explicit `relay requeue`. CLI
and service mode share an exclusive spool lock and the same schedule; status,
doctor and the console distinguish queued, held, dead and accepted batches.
Before each delivery the relay rechecks queued sessions against the current
source policy and, where directories have been selected, the local selection
and the hub's signed reporting approvals; a batch with nothing still cleared
is held, not dropped (`docs/contracts/directory-enrolment.md`). No console row is assumed safe for egress:
reconstructed crossings, refusals and rejected sources are never projected,
and of the refusals only a count per session crosses, as an integer on the
batch (`docs/contracts/telemetry-projection.md` §The refused count on the wire).
The console's egress block, and the Overview's hub card that renders it,
report the configured receiver and delivered counts, or the absence of
both, and the enrolled key id with its standing.

A session whose log cannot be read is skipped and named; every other session's
batches, those already spooled included, are delivered, and the run then fails
with the session named. The sessions last skipped are kept in
`relay/skipped-sessions.json` and named by `commonmeasure status`,
`commonmeasure doctor` and the console's egress block. A prune writes
`relay/spool/outbound.pruned` before the queue first loses a batch, so a
missing journal reads as an unknown delivered count where the queue shows no
gap (`docs/contracts/telemetry-projection.md` §Delivery state).

Enrolment (`commonmeasure connect`, `crates/commonmeasure-relay/src/enrolment.rs`)
is how an edge joins the hub in one command: it mints an Ed25519 key pair,
exchanges the owner's short-lived token for an org-scoped ingest key and
registers the public key in the same call, with a proof of possession. The
private key stays in the edge home; the ingest key goes into `relay.json`;
the public facts go to `enrolment.json`, from which every session records an
`edge_identity` line. `connect` also uploads the directory proof that lists
the key in the hub's key directory. Each relay run asks the hub for the
key's standing, records a revocation and renews the proof once it is a day
old; `commonmeasure disconnect` revokes both credentials at the hub and
removes the enrolment files. The files and the exchange are
`docs/contracts/enrolment.md`; the key directory and proof are
`docs/contracts/bot-identity.md`.

A hosted service that fetches supplier credentials from its hub keeps the
last release fetch, by name and never by value, in
`supplier-credentials.json` in the same home
(`docs/contracts/supplier-credentials.md` §Edge side, with its verification
state in §Status).

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

Edge is one binary with several execution modes and state under an Edge home
(`COMMONMEASURE_HOME`, default `~/.commonmeasure`). An enrolled home holds the
edge's signing identity. A host registration, runtime process, user, session
and registered agent instance are separate concepts; none automatically creates
a new enrolled home.

### Local integration

The host launches hooks or an MCP process, or a native harness links the runtime.
Local acquisition needs no continuously running Common Measure service:

- `install.sh` places a released, checksum-verified binary on `PATH`, or
  `cargo install --locked --path crates/commonmeasure-cli` builds one from a
  checkout (`docs/GETTING-STARTED.md` §1 and §8).
- `commonmeasure install <host>` writes the host's registration naming the
  binary by absolute path, `doctor` reads it back and `uninstall` removes
  it; the plugin declares the same hooks and MCP server for the marketplace
  route (`plugin/README.md`), and the standalone archive from
  `plugin/package.sh` installs with no repository and no toolchain.
- The console serves on loopback only; the inference gateway is a sidecar
  launched by image digest (`demo/gateway/tensorzero/`). On macOS,
  `commonmeasure service install console` keeps the console running as a
  LaunchAgent that starts this binary by absolute path with the installing
  shell's Edge home; acquisition does not use it (`docs/GETTING-STARTED.md`
  §5).
- State is `~/.commonmeasure/` plus the append-only evidence logs. Nothing
  from those records is reported by default; reporting goes to an explicitly
  configured receiver, spooled durably first. Supplier requests still send the
  query or URL needed for acquisition.

Multiple registered hosts may use that same home and enrolled identity, even
when each starts its own MCP process. Process separation alone does not create
separate credentials, records or tenant boundaries.

Every process using an Edge home runs the same release. Upgrade by stopping
all of them, including `commonmeasure hosted service`.

`commonmeasure update` enforces part of this. Before it stops or downloads
anything, it reads the process table and refuses while other processes of
the same user run the binary file it replaces (the path the kernel reports
for the executable is the file's canonical path, or the image is the same
inode), naming each one with its `COMMONMEASURE_HOME` where readable. A
process whose executable the kernel does not identify, such as one running
an old release whose last link was removed, is refused when its process
name is the file's name. A read of the process table that cannot be
completed refuses the update. It stops the console service it manages and
starts it again with the service's own Edge home. It does not detect:

- processes running another copy of the binary on the same Edge home;
- a copy under another name whose executable the kernel does not identify:
  on macOS one whose file has been removed, on Linux one that is not
  dumpable;
- a process started between the check and the rename that replaces the
  binary;
- other users' processes.

No lock is shared by the processes of a home, so these remain the operator's
to stop.

When the installer fails, `update` compares the binary's device, inode,
size and SHA-256 with those it recorded before the installer ran. Each read
hashes an open descriptor, then describes the descriptor again and looks
the path up again; metadata that moved, or a path that names another file than the
one read, makes the outcome unknown. This is a sequential check, not an
atomic snapshot: a writer that changes the binary and restores it between
the reads is not seen, and the comparison does not say which writer changed
it. A replacement is asked for its version only after the attempt to start
the console service again, whether or not that attempt succeeded. The
question runs as its own process group with standard error discarded; after
5 seconds, or past 4 KiB of output, the group is killed. The 5 seconds bound
the wait for its output and exit, not the system calls that start it and
reap it. A process that leaves the group is not killed and can hold the
output open; `update` does not wait for it. `service status` asks the
`commonmeasure` on `PATH` for its version within the same bounds.

Limits of the process inspection on macOS:

- The managed console is left out only when launchd reports the same pid
  before and after the scan, and the process at that pid is launchd's child
  and started before launchd was first asked. An orphaned process, whose
  parent is also launchd, that takes the console's pid after that is
  counted. Start times are wall-clock, so a clock set back during the update
  can defeat the comparison.
- A process the kernel refuses to describe (`proc_pidinfo` EPERM) is not
  taken to be another user's on that alone, since a security policy can
  refuse a process of the same user. Its owner is read from `sysctl`
  `KERN_PROC_PID`: the same user's is refused by process name, and one whose
  owner cannot be read is refused and listed as not inspected, not as a
  process known to run the binary.
- `COMMONMEASURE_HOME` is read from `KERN_PROCARGS2`. The arguments begin
  after the executable path and the kernel's padding to eight bytes, which
  is XNU's layout, not an interface. A layout that added NUL bytes before
  the arguments would pass the padding check and could read an argument as
  an environment entry, so the home shown would be wrong or missing; the
  listing would still show only an exact subcommand name and the home. The
  leading argument count does not settle it, since an empty argument zero
  looks the same. An environment read empty, as macOS returns for a
  restricted process, is shown as unknown.
- `KERN_PROCARGS2` is read from the process's own memory. After the
  environment come NULs and the strings XNU passes a process beside it
  (`apple[]`: `pfz=`, `stack_guard=`, `malloc_entropy=`, `ptr_munge=`,
  `main_stack=`, `executable_file=`, `dyld_file=`, `executable_cdhash=`,
  `executable_boothash=`, `arm64e_abi=`, `th_port=` and `security_config=`,
  some of which the process clears as it runs). The environment ends at the
  first of those keys that follows an empty string; a key the parser does
  not know leaves the end unfound. An environment entry that follows an
  empty one and starts with one of those keys also ends it, early. No
  `apple[]` key is `COMMONMEASURE_HOME`, so a string starting
  `COMMONMEASURE_HOME=` after that end, with none before it, shows the end
  may be wrong, and the home is shown as unknown. `COMMONMEASURE_HOME`
  found in the environment is shown. The home is shown as not set or empty
  in two cases: `COMMONMEASURE_HOME` is found with an empty or
  whitespace-only value, which the Edge itself reads as unset and resolves
  to its default home; or no string from the start of the environment to
  the end of the read starts with `COMMONMEASURE_HOME=`, wherever the end
  falls, and no empty entry inside the environment is followed by another
  entry. Otherwise it is unknown, never unset.
- A mapped image is matched by device and inode over every executable
  region, not only the first.

Releases are not signed. `install.sh` and `update` check a binary against
the `SHA256SUMS` published on the same origin, which shows that the bytes
match that list, not who published it. A release build of `update` always
uses the public release origin and prints it before changing anything
(`docs/GETTING-STARTED.md` §1, Updating).

### Hosted integration

`commonmeasure hosted service` runs Edge continuously and exposes mediated tools
over HTTPS. The current design has one hosted Edge for one organisation, with
one enrolled key and many authenticated principals and sessions. Word's Copilot
integration uses this path. The cloud machine runs Edge, its private records,
console and background management/reporting; Word and the host's model run
elsewhere. It must be reachable when the host calls its tools.

Each MCP session binds to its authenticated principal. The configured
`session_directory`, when present, is the service's declared scope for all its
sessions; it is not a client's document path or an automatically isolated matter.
User identity remains in the private record; the Content Telemetry projection
identifies the enrolled Edge and separates eligible sessions without exporting
their authenticated subjects. Separately registered working instances follow
`docs/contracts/instance-registration.md`; creating a hosted session is not that
registration.

A hosted Edge runs on one stateful virtual machine per organisation, not one
per user.

### Hub

The hosted tier, Common Measure Hub, is one multi-tenant service run by
Common Measure Ltd at hub.commonmeasure.ai, which is also the identity
origin in every enrolled edge's signature. The split between edge and hub:

- **Decisions stay at the edge.** The runtime refuses before bytes reach the
  host's context through mediated tools. With local integration that runtime
  is beside the host; with remote integration it is on the hosted Edge. The
  hub distributes management state and receives cleared evidence; content
  requests do not pass through it. A hosted edge's supplier credentials come from
  the hub and lapse on their own schedule
  (`docs/contracts/supplier-credentials.md`).
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
  mediated fetch presents to a publisher; an edge
  with no key to sign with makes no request and records why. Because the
  key is the same, no mediated request is made to the hub's own origin. The
  URL of a crossing is an agent's or a publisher's choice, and the hub
  would accept the signature it arrived with. A `context_fetch` or a
  redirect hop whose authority is the enrolled hub's, the origin the
  enrolment's `Signature-Agent` names or the `policy_url`'s is refused
  before anything is sent and recorded as `crossing_refused` with one fixed
  reason; a `robots.txt` or licence probe to such an authority is not sent.
  No policy setting lifts this. An enrolled edge that cannot read those
  origins in full, because `deployment.json` is unreadable or a named URL
  has no authority, stops signing and records why. An edge with no
  enrolment has no hub origin and fetches as before.

Fleet status is a management document separate from Content Telemetry
(`docs/contracts/fleet-status.md`), printed by `commonmeasure status --json`
and sent nowhere. It reports the edge's key id,
the deployment mode, the desired and applied revisions, the digest of the
declared policy and the identity of the canonical effective policy,
software and resolver versions, principal identity and authentication
basis, the last enforcement time and a bounded allowance summary. It does
not put policy detail into the purpose-limited telemetry projection. A
self-reported digest establishes configuration drift evidence, not
hardware-backed proof of enforcement.

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

The projection emits retrieval, grounding and cleared minimal turn boundaries
at Grounding level, with selected coverage (`docs/contracts/telemetry-projection.md`),
as Content Telemetry v1.0
(`schema/SOURCE.md`, `crates/commonmeasure-relay/tests/conformance.rs`). The relay's
wire types (`crates/commonmeasure-relay/src/wire.rs`) have no variant for citation,
display, reproduction or engagement, because the evidence log witnesses
nothing for them. Evidence-backed reproduction and citation events, drawn
from the grounding evaluator's verdicts, are planned. The boundary rules
hold for them: the evaluator's verdict vocabulary stays local, and only
positive claims the sealed evidence supports are projected.

The relay also sends the operator's own hub, where it issued the instance,
an event-level `instance` member on content events; the hub removes it
before onward delivery, so no publisher receives it
(`docs/contracts/telemetry-projection.md` §Instance reference).

The standard never receives the full prompt, the response, the supply
alternatives, the commercial objective, evaluator output, the workforce trace
or rejected private sources, unless a separate contract explicitly requires
them. It does receive the name of the supplier that served an admitted
source, as a namespaced custom field, and nothing about what the supplier
charged, and the number of crossings policy refused in the session, as an
integer on the batch and nothing else about them.

## Security and credentials

- adapter credentials are scoped per provider and environment;
- no secret appears in run manifests, logs or repository files;
- the model never receives raw credentials;
- provider calls originate in the mediated runtime;
- operator policy sets egress separately from capture;
- live paid calls require explicit budgets and idempotency.
