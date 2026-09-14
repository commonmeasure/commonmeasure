# Product

## Promise

Common Measure controls what content an organisation's AI agents may read,
records where every piece came from and what it cost, and shows whether it
helped. The record it leaves is one a person or a regulator can check.

## Definition

Common Measure is a component that organisations build into agent systems
they build themselves. Vertical agent products are expensive and rarely fit
a firm's own work, so firms increasingly build their own harnesses. The part
they should not build or customise is the part that has to be the same
everywhere: the rules on which sources an agent may use, the record of what
it took in, and the measurement of what that content was worth. Common
Measure supplies that part once, so each firm's harness can differ in
everything else.

It sits between an agent and everything the agent reads: web pages, search
results, licensed content feeds, internal documents, code repositories and
third-party skills. For every piece of content, before the agent reads it,
it:

1. applies rules: which sources are allowed or forbidden, under what
   licence, for which principal, within what spend allowance per job, day
   and month;
2. records where the content came from, what it cost, a hash of exactly what
   entered the model's context, and what the agent did with it;
3. measures whether the content helped: whether the answer was grounded in
   it, how much of the question it covered, how fresh it was.

The category line is "input control and audit for AI agents".

## The question the first version answers

Can a component that manages and measures the content an agent takes in
choose more effective content and skills than a fixed route, within cost and
efficiency constraints, and leave enough local evidence for a person to
explain and trust the choice? The committed experiments hold the answer
model fixed and vary only the supply, so the effect of content is isolated.
The acceptance gate, with the state of each line, is `ROADMAP.md`
§Acceptance gate.

## Buyer

The buyer is an organisation that runs agents for many principals and has to
answer for their inputs: the platform lead, architecture owner, risk owner
or operating owner. Three instances of that buyer are in discussion:

- a consultancy running several concurrent agent engagements, each under a
  client's own source and confidentiality rules;
- a regulated advice firm, where the professional regulator expects a
  defensible record of what an AI system was given and where it came from;
- a university issuing standard agent tooling to students, routed through
  the library's licensed sources with safety checks switched on.

Each of them needs to answer:

- Why did this agent use these sources?
- Could it have obtained a better result for the same cost?
- Did it remain inside our source, licence and spend policy?
- Which principal exercised that authority, and how much of its allowance
  remains?
- What evidence did it need but fail to obtain?
- Which route should we use for the next comparable job?

The developer integrates the product; the compliance owner reads its output.
The source record is written for the second reader: every claim in it cites
the record it was read from, and the grade of each crossing (mediated,
observed, reconstructed) is never hidden in a total.

## Core and add-ons

The core manages and measures the stream of content an agent takes in. It
is the rules, the record and the measurement, and it is complete on its own.

Add-ons run on that same stream through one contract
(`docs/contracts/processor.md`): a namespaced identity, a configuration
digest and an evidence record per invocation. Every add-on compiled into
the binary runs wherever it is wired; three run only in a batch run whose
job asks for them; a per-operator switch in policy is not built
(`ROADMAP.md` §Add-on management and settings). The first set is built and
maintained here:

- the injection screen and the PII detector, at admission (shipped);
- the support-status governor, at admission (shipped);
- the context optimiser, before inference (shipped);
- output provenance labels and fidelity verification (roadmap packages).

Third parties can build add-ons to the same contract. Their findings stay
namespaced to the add-on that produced them and are never collapsed into a
universal score.

## Product functions

1. **Choose routes** — turn an input requirement, desired output and policy
   into eligible routes.
2. **Execute** — acquire content and select or invoke skills through explicit
   capability contracts.
3. **Enforce** — refuse or escalate before content enters the model.
4. **Evaluate** — compare controlled runs using operator-owned measures.
5. **Optimise** — select routes using measured evidence, starting with
   explicit rules and only later using learned policies.
6. **Report** — show the operator the source record and emit its permitted
   projection in Content Telemetry format.

Governance establishes which inputs an agent is authorised to acquire and
use, under which identity and cumulative limits. Measurement establishes
whether the admitted inputs improved the result enough to justify their
cost. Selection needs both: an effective route is not eligible when the
agent lacks authority to use it, and a permitted route is not valuable merely
because policy allowed it.

The operator also needs an inventory of everything in the agent's context
window: resident instructions and tool definitions, memory, skills,
conversation, acquired content and definitions loaded on demand, each with
its observed or explicitly estimated footprint and the remaining budget. A
discoverable tool is distinct from one whose definition entered context or
whose capability was invoked. Host-owned prompts and model internals stay
outside the product's control and are reported as unavailable.

## What it is not

- **Not a harness.** It does not run the agent loop, hold the conversation
  or call the model. It attaches to the operator's harness through hooks and
  MCP, or through a screening-proxy contract.
- **Not an agent framework.** It has no opinion on how the agent is written.
- **Not a content exchange or payment rail.** It records a supplier's price
  and terms and can pay a quoted price under an allowance; auctions,
  settlement and custody stay external. A supplier cannot pay for rank or
  context-window position.
- **Not a model gateway.** An external gateway handles provider
  compatibility, credentials, retries and load balancing. Common Measure
  records the model route beside the content route so an operator can tell
  whether an improvement came from better content, a better model or both.
- **Not an owner-side telemetry platform.** It does not register content
  owners, verify their domains or show them dashboards. It reports to the
  conforming consumer an owner designates, and its relay is tested against
  at least one consumer Common Measure does not run.

## Edge and hub

The edge is the local product: the runtime, harness integration,
measurement, deterministic enforcement, principal resolution, the allowance
ledger, the evidence logs and the console. It works offline and is complete
without the hub.

The hub is the hosted service for coordination across many edges:
enrolment, organisation-wide policy, signed distribution, rollout, drift,
exceptions and fleet-wide evidence. In the standard's terms the hub is the
operator's telemetry consumer: it receives the fleet's cleared projection
and delivers each owner's events onward to the destination that owner
designates. It holds no owner accounts. It has two tiers: a free individual
tier for an organisation of one person, and a paid institutional tier for
an organisation of more, which adds organisation-wide signed policy, fleet
evidence and add-on mandates. Two invariants fix the boundary: the hub
is never in the decision path for a crossing, and its absence never relaxes
local policy. A feature that needs either broken does not go to the hub.

The edge is source-available under the Functional Source License
(`LICENSE.md`), converting to Apache-2.0 two years after each release; the
contracts under `docs/contracts/` are CC-BY-4.0. It is not open source.

## What compounds

Adapter breadth is useful but copyable. The asset that compounds is a history
of comparable decisions about content, joined to outcomes:

- what each route returned;
- what it cost in acquisition, latency and context;
- what was admitted or refused and why;
- what the agent cited and what the evaluator accepted;
- what was missing;
- which plan performed best for a defined class of job.

## Naming

The product, the company and the agent are all Common Measure. The category
is ContextOps: the discipline of controlling and auditing what agents take
in. The evidence a session or run leaves is the source record; the operator's
policy file is the source policy. The hosted tier is Common Measure Hub.

Identifiers: the binary is `commonmeasure`; the environment prefix is
`COMMONMEASURE_`; the home directory is `~/.commonmeasure`; the crates are
`commonmeasure-*`; the plugin, its marketplace entry and its MCP server are
`commonmeasure`; the agent's product token in `User-Agent` and `robots.txt`
is `CommonMeasureBot`; the relay's emitter id is `commonmeasure`. The
product repository is `commonmeasure`; the hub is maintained separately.
The product domain is commonmeasure.ai.

Two namespaces stay as they are. The MCP tools are `context_fetch`,
`context_search` and `context_status`: they name the action, not the
product. Format identifiers inside sealed evidence (`contextops-run/vN`,
`contextops-processor-invocation/v1`, `contextops-session`, the
`contextops-relay:v1` event-id seeds) keep the `contextops-` namespace,
because they are wire strings inside hash-sealed artefacts and a change would
invalidate every committed record. For the same reason the committed run
output under `demo/output/` and the recorded provider captures under
`demo/recon/` carry the identifiers that were current when they were sealed
(`ctx-runtime`, `ctx-supply/0.1`, `CONTEXTOPS_INFERENCE_ENDPOINT`,
`ContextOpsRecon`): they are not edited, and a regenerated run carries the
current identifiers. The gateway route name the demo jobs and the reference
gateway configuration declare (`contextops_gpt_4o_mini_2024_07_18`) also
stays, because every committed run that names it is sealed and regenerating
those runs needs a live gateway.
