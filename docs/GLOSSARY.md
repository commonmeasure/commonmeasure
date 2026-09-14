---
title: Glossary
---

# Glossary

The product's defined terms, each in plain English. Other documents use
these words for precision and link here rather than redefining them. The
authoritative technical definitions live in the contracts under
`docs/contracts/`; this page is the one-line version.

## The product and its parts

- **Common Measure** — the product: a component an organisation builds into
  its own agent system. It rules on which content an agent may take in,
  records where each piece came from and what it cost, and measures whether
  it helped. The command-line binary is `commonmeasure`.
- **Source record** — everything a session or run leaves behind: the
  evidence log, the sealed provider bytes, the manifest and the dossier,
  taken together. It is what a person or a regulator checks afterwards.
- **Operator** — the organisation, or the person acting for it, that runs
  agents and owns the policy, the record and the machine they live on.
- **Edge** — the part of the product that runs on the operator's machine:
  the runtime, the harness integration, policy, the allowance ledger, the
  evidence logs and the console. It is complete without the hub.
- **Hub** — Common Measure Hub, the service Common Measure Ltd hosts at
  hub.commonmeasure.ai, one service serving many organisations, that
  receives cleared evidence from many edges and distributes
  organisation-wide policy as signed revisions the edge accepts. It is never
  in the decision path for a crossing, and its absence never relaxes local
  policy.
- **Harness** — the agent system an operator runs, whether bought or built.
  Common Measure is a component inside it, not a harness itself.
- **Host** — the specific program the product registers with: Claude Code
  (hooks and the MCP server), Codex (the MCP server; one table serves the
  Codex CLI, the ChatGPT desktop app and the Codex IDE extension), Pi (an
  extension that runs the MCP server), Claude Desktop (the MCP server),
  Cursor (the MCP server and hooks), the Copilot CLI (the MCP server and
  hooks) or VS Code (the MCP server). A Chrome extension observes three
  browser answer surfaces, recorded as the hosts `chatgpt-web`,
  `google-ai-overview` and `bing-copilot-search` and registered with
  `commonmeasure install chrome`. A host supplies session identity
  and, where it can, lifecycle hooks. In session evidence `host` is the
  registration's word for the host, `client` is the MCP client's own name
  and version, and `host_name` is the hostname of a source URL, not this
  host.
- **Registration** — the entries a host's own configuration holds naming
  the binary: written by `commonmeasure install <host>`, checked by
  `commonmeasure doctor`, removed by `commonmeasure uninstall <host>`.
- **Harness plugin** — the package in `plugin/` that registers the product
  with Claude Code through the host's plugin mechanism: a manifest, the
  hooks, the MCP entry and a launcher, no binary. "Plugin" on its own always
  means this, never a processor.
- **Processor** — a pluggable stage that checks or transforms content at a
  point in a run or a mediated crossing, behind
  [`docs/contracts/processor.md`](contracts/processor.md), and writes an evidence record per
  invocation. Add-ons are processors; which ones ship is `ROADMAP.md`
  §First-party add-ons.
- **Console** — the local web application served by `commonmeasure serve` on
  loopback: Overview, Record, Policy, Sources and Compare, rendered from the
  evidence logs. Its writes are the Compare query, the policy mode, a
  scope's denied hosts and the attribution rules.
- **Index** — the SQLite file the console derives from the evidence logs
  (`~/.commonmeasure/telemetry.db`). It can be deleted and rebuilt; the logs
  are authoritative.
- **Gateway** — the external OpenAI-compatible service that runs model
  inference. Common Measure records the route the gateway reports and never
  calls a model provider directly.
- **Receiver** — the service the relay delivers cleared records to. Nothing
  is delivered unless one is configured.
- **Relay** — the command (`commonmeasure relay`) that sends cleared records to
  the configured receiver. It is the only way records leave the machine, and
  it refuses to run without a configured receiver.
- **Spool** — the local, durable queue of projected batches the relay writes
  before any delivery attempt, so a crash cannot lose or double-count a batch.
- **Content Telemetry** — the published standard (v1.0) for reporting what
  content an agent retrieved and grounded on, defined by the SPUR Coalition.
  The relay emits a permitted subset of the source record in that format;
  the schemas are pinned copies in `schema/`.
- **Discovery manifest** — the JSON file a content owner, agent or platform
  publishes at `/.well-known/content-telemetry.json` under the Content
  Telemetry standard, stating who they are and where telemetry about their
  content goes. Distinct from a run's manifest below. It carries no
  reporting demand; a demand comes from a licence or the operator's terms.
- **Nudge** — the standing instruction the plugin gives a host at session
  start to prefer the mediated tools over its built-in fetch and search.
- **Skill catalogue** — the operator's list of admitted third-party skills
  (`COMMONMEASURE_SKILL_CATALOGUE`); a skill not in it cannot be invoked.

## The moment content moves

- **Crossing** — any moment content from outside enters an AI agent's
  working context: a web page fetched, a search result read, a document
  retrieved. The unit everything else is built around.
- **Observed crossing** — a crossing recorded *after* it happened, by a
  hook watching the agent's ordinary tools. It cannot be stopped, only
  witnessed.
- **Mediated crossing** — a crossing requested through the product's own
  fetch and search tools (`context_fetch`, `context_search`), so policy can
  check it before the content moves and can refuse it. A third tool,
  `context_status`, reports the policy state and records nothing.
- **Reconstructed crossing** — a crossing read back from a host transcript
  afterwards, with nothing watching at the time. The weakest record of the
  three.
- **Grade** — how a crossing was recorded: observed, mediated or
  reconstructed. Counts of different grades are never totalled together.
- **Witnessed** — observed or mediated together: a crossing something was
  watching when it happened, as opposed to one reconstructed afterwards.
- **Refused count** — the one fact about refused crossings that leaves the
  machine: a session's running total of them, carried as `refused` on every
  batch the relay sends for the session, with no address, reason or hash.
  A receiver keeps the larger value it has seen.
- **Source policy** — the operator's policy file, `policy.json`: the mode,
  the rules admission applies, the scopes and principals that replace them,
  and the clearance for records to leave. Its format, and every check the
  loader makes, is [`docs/contracts/source-policy.md`](contracts/source-policy.md).
- **Admission** — the yes/no decision on whether retrieved content may
  enter the model's context window, made by deterministic policy rules.
- **Access rule** — one ordered host rule in a policy: a host pattern and
  what happens to sources matching it (allow, refuse, require a named
  licence, require mediation, which admission always meets). The first
  matching rule decides. The denied-host list is a set, checked before any
  rule.
- **Statement / preference** — what a source declares, in machine-readable
  form, about a category of AI use (`train-ai`, `ai-input`, `ai-index`,
  `search`): allow or disallow, from a `Content-Usage` header or `robots.txt`
  rule, a `Content-Signal` line or an RSL licence. A preference is not a
  licence and not enforcement; statements combine most-restrictive-wins and
  an absent one is unknown.
- **Named by** — who named the source of a mediated fetch: `user` when a
  prompt of the session carried the URL, `agent` when prompts were recorded
  and none did, `unknown` when no prompt was recorded. A recorded fact, not
  a carve-out from any preference.
- **Terms** — an agreement the operator holds with a source, declared in
  `policy.json` by host and reference. Terms govern over a source's
  published preference and are never checked by the runtime.
- **Policy mode** — how strictly policy acts: `observe` records only,
  `prefer` records and steers, `strict` refuses what the rules do not
  allow.

## Who the work is for

- **Principal** — the authenticated agent or operator exercising delegated
  authority. Its authentication basis must be something the process cannot
  freely rewrite; a supplied environment label alone is not authority.
- **Engagement** — a named piece of work (a client, a project) that
  records, policy and reporting are grouped under.
- **Governing engagement** — the engagement a policy scope names for a
  working directory. It is recorded at capture time and is the only name the
  runtime and the relay can read.
- **Reported engagement** — the engagement the console counts a session
  under, resolved from attribution rules when records are read. It can be
  changed later without any evidence file changing.
- **Attribution** — the ordered rules in `~/.commonmeasure/attribution.json`
  that map sessions to reported engagements at read time. Work no rule names
  is shown as unattributed, never hidden.
- **Scope** — one policy rule in `policy.json`, keyed to a working
  directory: which engagement governs work there and what that work is
  allowed to do.
- **Clearance / telemetry egress** — explicit written permission for one
  engagement's records to be reported off the machine. Without it,
  nothing leaves.
- **Allowance** — a cumulative cost limit delegated to a principal for a
  declared period, enforced from a local transactional ledger. It is distinct
  from the maximum cost of one job.
- **Ledger** — the allowance ledger only: the local transactional store that
  reserves and reconciles a principal's spend. The evidence trail is the
  evidence log, never "the ledger".
- **Canonical JSON** — the one way a JSON document is written out before it
  is hashed or signed: RFC 8785, with members sorted, no whitespace and one
  spelling for every string and number. It exists so a reviewer recomputes a
  seal from the document with any conforming serialiser, and so a seal does
  not move when a dependency changes. The rule and where it applies are
  [`docs/contracts/canonical-json.md`](contracts/canonical-json.md).
- **Policy identity** — the digest of the canonical effective policy a
  principal resolved, including the resolver version. A reported digest is
  drift evidence; without a protected signing key it is not proof that an
  uncompromised edge enforced the policy. The pre-image and the recipe are
  [`docs/contracts/fleet-status.md`](contracts/fleet-status.md).
- **Fleet status** — the document an edge builds about the policy it
  applies (`commonmeasure status`): edge identity, deployment mode, desired
  and applied revisions, digests, versions, the last enforcement time and a
  bounded allowance summary. A management contract separate from Content
  Telemetry; it carries no policy text and no engagement name.
- **Deployment mode** — the edge's own choice, in `deployment.json`, of
  whether to accept remote policy: `local` accepts none and makes no
  management request; `managed` pins one signing key and accepts a signed
  envelope from it. Nothing remote can change it.
- **Policy envelope** — the signed document the hub distributes a desired
  policy in: organisation and edge binding, ordered revision, issue and
  expiry times, the policy and its digest, under an Ed25519 signature. The
  edge validates it through the ordinary policy loader before activating
  it and keeps the last accepted one when anything fails.
- **Stale (policy)** — the state of a managed edge whose applied envelope
  has passed its expiry without the hub renewing or replacing it. The
  policy stays in force unchanged; `status`, `doctor` and the session
  record say since when, until a refresh clears it.

- **Enrolment** — connecting an edge to the hub (`commonmeasure connect`):
  the edge mints its signing key, the hub registers the public key under
  the organisation and issues the ingest key. Only an enrolled edge presents
  the `CommonMeasureBot` identity to publishers.
- **Key id** — the pseudonymous identifier of an enrolled edge: the
  thumbprint of its public key, published in the `CommonMeasureBot` key
  directory at hub.commonmeasure.ai and carried as the agent identifier on
  the wire. It names an
  edge, never an operator or a person.
- **Access context** — the institution identifiers a session carries when
  operator-held terms for a source attribute usage to the operator's
  agreement. Absent unless terms require it; never a person.

## Running a job

- **Job** — the declared input to a batch run: the question, the supply
  routes to try, the rules and the measures, fixed before anything executes.
  The JSON files under `demo/jobs/` are jobs; "suite" in a job file means
  the same thing.
- **Run** — one batch execution of a job: acquire content, apply policy,
  run inference, evaluate, record. Published as a directory of artefacts.
- **Supply plan** — the declared graph of content acquisition and skill
  invocation a job tries, with its fallbacks and stop conditions
  (`SupplyPlan`); a job may declare several to compare.
- **Model plan** — the declared answer model, eligible inference providers
  and model budget (`ModelPlan`), kept separate from the supply plan.
- **Plan** — one supply route inside a run (one provider, one skill, the
  operator's own documents), executed and measured so it can be compared
  with the others.
- **Route** — a candidate way of supplying a job before it runs. The
  **model route** is the gateway and provider path the gateway reports
  having executed; it is recorded beside the content route.
- **Supply condition** — in a comparison, the set of source classes a plan
  is allowed to draw on (one class alone, or all classes mixed).
- **Manifest** — the sealed statement of what a run was asked to do,
  identified by a hash. Change anything about the experiment and the hash
  changes; two runs with one hash are the same experiment.
- **Evidence log** — the append-only file recording everything that
  happened in a run or session, in order. Missing knowledge appears as an
  explicit **gap**, never as a silent zero.
- **Dossier** — the human-readable explanation of a run printed by
  `commonmeasure inspect`: what ran, what it saw, what it cost, why a route
  was chosen, with every claim ending in a citation to its record.
- **Corpus** — a directory of documents the operator already owns, usable
  as a content source. Its `corpus.json` declares each document's date
  and licence, because owning a directory is not a rights statement.

## Checks and measures

- **Injection screen** — a deterministic pattern matcher for known
  prompt-injection phrasings. It names the exact rule it matched and
  states its blind spots; it is screening, not a full defence, and text
  it does not match is "clean under these rules", not "safe".
- **Grounded (content)** — page text that was put into the model's context
  for a request, as opposed to content only searched or listed.
- **Evaluators (grounding, coverage, freshness)** — deterministic
  measurements recorded per plan: whether the answer's citations really
  appear in the content the model saw (grounding); how much of a declared
  checklist the acquired content covered (coverage); how recent the
  content is against a declared reference date (freshness).
- **Claim** — one sentence of an answer, outside its citation lines, as the
  fidelity verifier segments it; each carries its byte offsets into the
  answer.
- **Fidelity verifier and fidelity judge** — the processors that judge an
  answer's claims after inference. The verifier is deterministic: a claim is
  supported where a verbatim span of the window backs it, contradicted
  where it quotes a citation the grounding evaluator found absent from its
  source, unsupported otherwise; the span is cited. The judge is a model
  asked the same question over the same window, run only when the job
  declares it; its verdicts are recorded as declared and measured for
  agreement with the verifier's, never as ground truth.
- **Selection** — the router's choice among a run's plans, computed only
  from measured terms. Where a needed measurement does not exist, it
  abstains and says what is missing rather than guessing.
- **Unavailable** — the word for a thing that could not be produced, in
  four places: a plan whose dependency (credential, gateway, recording) was
  missing; a grounding verdict whose citation line did not parse; a
  principal whose authentication basis could not be established; and a
  context category a host does not report. Each record names which.

## How strongly a claim is evidenced

- **Verification states** — every integration claim carries one of:
  `planned` (interface identified, nothing proven), `fixture-tested`,
  `replay-tested` (recorded real responses served through the real code
  path, hash-verified), `spec-verified` (checked against the supplier's
  documentation only), `live-verified` (a dated real call succeeded and
  its evidence is kept). The states are never collapsed into one another.
- **Replay** — a run mode that serves previously recorded provider
  responses through the same code path live traffic uses, so an
  experiment can be repeated without the network. Distinct from a live
  run, and labelled as such.
