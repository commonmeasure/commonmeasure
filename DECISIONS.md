# Current decisions

This file records the decisions that constrain the current product. Git holds
the history; this document does not repeat it.

## Product

- Common Measure rules on, records and measures the content an agent takes
  in; the definition, the buyer and the core and add-on split are
  `PRODUCT.md`.
- Add-ons run on the same stream as the core through the processor contract
  (`docs/contracts/processor.md`), each invocation an evidence record, and
  each stays replaceable behind the contract. Which add-ons ship is
  `ROADMAP.md` §First-party add-ons.
- Not built and not planned: monetisation of outputs (affiliate links,
  ad-server decisions), a supply-facing display of demand, a content
  marketplace between buyers and suppliers of content, and a
  publisher-as-operator persona. Brand and AI-visibility reporting is a
  separate company's use of this product and is not built here.
- The product name and every identifier are `PRODUCT.md` §Naming. Format
  identifiers inside sealed evidence keep the `contextops-` namespace; `ctx`
  is never claimed as a short name
  (`docs/knowledge-base/naming-collision.md`).
- The edge is source-available under the Functional Source License,
  FSL-1.1-Apache-2.0 (`LICENSE.md`); each release converts to Apache-2.0 two
  years after it is made available. The product is described as
  source-available, never as open source. `Cargo.toml` carries
  `license-file = "LICENSE.md"`, which every crate inherits, in place of an
  SPDX expression; the licence's SPDX identifier is FSL-1.1-ALv2. The protocol contracts and
  evidence format specifications under `docs/contracts/` are CC-BY-4.0
  (`docs/contracts/LICENSE`), so anyone may implement the standard whatever
  the code licence. A contribution is accepted under the contributor licence
  agreement in `CONTRIBUTING.md`, which assigns its copyright to Common
  Measure Ltd so the licence can be relaxed later without a further request
  to each contributor.
- The commercial premise, and the public description of it, is
  "source-available edge, hosted hub". The hosted tier has two tiers. The
  individual tier is free: one organisation of one person gets enrolment, a
  key id, the directory listing and owner reports at no charge. The
  institutional tier is paid: an organisation of more than one person, whose
  first three paid features are organisation-wide signed policy, fleet
  evidence and add-on mandates. Pricing is open (§Open decisions).
- The unit of batch work is a `ContextJob`. A job runs one route per supply
  plan, one step each, and the plans are compared, not combined. Cached
  context, sequential fallback and complementary sources are extensions of
  the same job shape and none is built.
- Common Measure acts for the operator. Supplier price and terms may affect a
  decision; a supplier cannot pay for rank or context position.
- Deterministic policy precedes learned routing. Learned routing requires
  versioned outcome evidence and measured holdout improvement.
- No fitted routing rule ships. The one rule fitted on a training split lost
  to the fixed default on the holdout (`docs/READ-A-HOLDOUT.md`); selection
  is per-run over measured terms, which beat both. A fitted rule ships only
  when it beats the fixed default on a holdout it never saw.
- The agent's product token, `CommonMeasureBot`, is chosen once and is never
  renamed, because publishers address it by name in `robots.txt` and a
  renamed token would orphan every rule that names it.
- `CommonMeasureBot` is one identity fronting many operators: an intermediary
  agent in Cloudflare's classification, where a publisher trusts the party
  that runs the software and extends that trust to each end user of it.
  Common Measure registers the identity once, on the product domain, and
  answers to publishers for the whole fleet; the identity origin written
  into every enrolled edge's signature, from which a publisher fetches the
  key directory, is https://hub.commonmeasure.ai. Each enrolled edge signs with
  its own key under that identity, so one edge can be revoked without
  touching the name or any other edge. An edge that is not enrolled sends
  the user agent unsigned and is not the network identity. Publishers are
  told to trust the signature, never the name. The name therefore travels
  only where a signature travels with it: a request to a supplier's API
  under the operator's own credential names the adapter and its version
  instead, and telemetry delivery and enrolment reach the hub under the
  operator's ingest key.
- A publisher sees an enrolled edge as a pseudonymous key id, never as an
  operator name. The hub holds the mapping from key id to organisation and
  is the party a publisher complains to; the operator is not disclosed
  without its agreement. The same key id is the agent identifier on the
  Content Telemetry wire, so a publisher can match each signed request to
  the licence token it carried and the report it received. That match is
  the evidence for the claim that the agent pays and reports, and it is why
  the owner report discloses the key id while still disclosing no session
  identifier and no engagement.
- Settlement stays outside the product. Where a rail funds licensed access,
  the account is an aggregator account held by Common Measure with a
  sub-credential per enrolled edge, each settled to the operator's own
  payment method with the rail as merchant of record; Common Measure never
  holds funds. The sub-credential is issued at enrolment, bound to the
  edge's key id and stored only on the edge; the edge's allowance ledger
  caps what it may spend. A rail that cannot issue a pseudonymous
  sub-credential is used only as an acquisition adapter under the
  operator's own account and name, never under the network identity.
- The processor contract is the product's extension point, and an
  organisation writes its own processor without Common Measure's
  involvement: a program in any language behind the sidecar boundary,
  installed on its own edges or distributed to its fleet through its own
  catalogue, with the same evidence, visibility and settings as a
  first-party add-on. First-party add-ons are held to the same path, so
  it is never worse than the in-process one (`ROADMAP.md` §The sidecar
  processor boundary).
- An add-on marketplace is planned: third-party processors, free or paid,
  listed in the hub's catalogue, approved per organisation, and run behind
  the sidecar boundary so that the permissions a manifest declares are
  enforced rather than trusted (`ROADMAP.md` §The sidecar processor
  boundary, §The add-on catalogue and marketplace).
- Every add-on invocation is visible to the operator. The console and the
  dossier show, per session and per run, which add-ons ran, on what, what
  each found, changed or refused, and what it could not see, from the
  invocation records. An add-on that ran leaves no less trace than a
  crossing, and an add-on that was active and did not run is recorded as
  such.
- An add-on declares its settings as a schema in its manifest, and the
  console and the hub render the form from that schema. No add-on ships a
  screen of its own, so a third-party add-on needs no console code and an
  organisation's locked settings render on the same form (`ROADMAP.md`
  §Add-on management and settings).
- Eligibility, delegated authority and cumulative limits filter first;
  measured effectiveness ranks what remains. Permission is not evidence of
  value, and measured value does not override policy.
- Context and model routes are separate in the evidence record. Common Measure
  makes the job-level decision; an external gateway handles model-provider
  compatibility, credentials, retries and low-level balancing. TensorZero is
  the first gateway, run as a local sidecar through its OpenAI-compatible
  endpoint; a controlled comparison pins one model snapshot to one provider
  route with no hidden fallback, and a field the gateway does not report stays
  unknown.
- The owner-side telemetry platform is not Common Measure's to run. Owners
  register with a conforming consumer operated by others; Common Measure
  keeps the edge interoperable with at least one such receiver by an
  executable test and does not build owner registration, domain
  verification or owner dashboards
  (`ROADMAP.md` §Out of scope).
- The packaging boundary: value to a single operator goes to the edge;
  coordination across operators goes to the hub; the hub is never in the
  decision path for a crossing and its absence never relaxes local policy. A
  feature that needs either invariant broken does not go to the hub.
- A hosted edge serves the hosts that reach
  MCP only over HTTPS. It is the edge binary in a service mode, one per
  organisation, enrolled and managed like any edge, and never part of the
  hub. The policy engine, the refusal semantics, the record format and the
  relay are unchanged, and no hub call sits in a crossing's ruling. It
  speaks Streamable HTTP, and each person authenticates with an access
  token the hub issues as the authorisation server, under the hub's own
  sign-in. The token's subject is the principal on every record, under its
  own authentication basis. A firm's own identity provider reaches the
  hosted edge only by federation at the hub. Common Measure Ltd operates it
  in the first instance, on one machine with a durable disk that holds the
  operator record in the file layout every contract describes, in a cloud
  project of the organisation's own. Common Measure Ltd therefore holds
  that organisation's private record, which a local edge never lets leave
  the firm, and says so in the organisation's agreement. The hub still
  sees only the cleared projection. The design is
  `docs/knowledge-base/hosted-edge.md`, and the package is `ROADMAP.md`
  §The hosted edge.

## Execution and evidence

- A missing credential, gateway or other dependency produces an
  `unavailable` result naming the dependency, never a weaker successful one
  (`docs/FAIL-POLICY.md` §5).
- Recorded bytes may stand in for an uncontrolled network only across a real
  transport boundary into the production path. `commonmeasure run --replay`
  serves hash-verified captures from loopback origins through the production
  adapters, policy, inference path and evidence storage; a provider without a
  recording, or a capture whose bytes no longer match the manifest, fails the
  run before it starts. `replay-tested` sits between `fixture-tested` and
  `spec-verified` in the verification vocabulary and is distinct from
  `live-verified`. The run contract is `contextops-run/v7`; a replay run
  publishes `replay.json` linking each sealed response to its recorded input
  hash.
- Observed, mediated and reconstructed are different grades of evidence and
  are never merged; every record states which produced it
  (`docs/contracts/session-evidence.md` §Crossing).
- Capture hooks are best-effort and never interrupt the host agent.
  Unreadable payloads produce no records.
- A PII finding on a public source is recorded on the crossing, not
  refused: under `strict` the crossing is admitted with the finding in
  `breach`, because a public page's published contact details are not the
  personal data the detector exists to keep out of a model. `strict` still
  refuses a finding on an internal or private source: a named internal
  prefix, a loopback or private address reached under `allow_private_hosts`,
  or the operator's own corpus. "Public" is what the existing source
  classification already says, and no second classification exists for
  this rule. The policy switch `refuse_on_pii`, off by default, restores
  the refusal on every source for an operator that wants it
  (`docs/contracts/source-policy.md` §Recording, `docs/FAIL-POLICY.md` §6).
- The private operator evidence log is authoritative. Console views and
  Content Telemetry messages are derived projections with separate access
  and egress policies.
- Unknown cost, token, latency, licence and evaluation values remain unknown,
  never zero (`docs/FAIL-POLICY.md` §7).
- An output provenance label is signed with an identity the operator
  supplies; the edge issues none. A label whose certificate is on no trust
  list is recorded as valid but untrusted, never as trusted and never as
  absent. The manifest carries hashes and identifiers only; no prompt,
  context or answer text enters it. The demonstration identity under
  `demo/provenance` is committed, private key included; it authorises
  nothing and is on no trust list.
- A model judge's verdict is a declared statement, never ground truth. The
  fidelity judge runs only when a suite declares it, under the suite's own
  model plan and over the window the answer model received; it is measured
  for agreement against the deterministic fidelity verifier's per-claim
  verdicts, and neither its verdicts nor the verifier's are mapped onto a
  quality score, fed to selection or projected as telemetry.
- The console is server-rendered by `commonmeasure serve` from the same values
  the JSON API serves, with vendored htmx as the only client script. It
  reads; its writes are the Compare query, which runs live provider
  searches on operator submission, and three declaration edits on the
  Policy screen: the policy mode, a scope's denied hosts and the
  attribution rules. It cannot start a run, make a fetch outside Compare,
  write any other policy field, or touch an evidence file.
- Every console policy write meets these conditions
  (`crates/commonmeasure-console/src/console/edit.rs`): it validates the
  candidate bytes through the runtime's own
  loader so the console cannot write a policy the runtime would refuse; it
  saves atomically behind a revision check and refuses a conflicting edit
  with both versions stated; it shows the effect before the save for fields
  that widen what may happen (`allow_telemetry_egress`,
  `record_internal_prefixes`); it never creates a declaration where none
  exists; it is keyed by the artefact's own unit, the scope, not by an
  engagement name; and it states that a policy edit governs the next crossing
  and nothing already recorded, and reaches a running mediated session only
  when that session next starts. A scope that inherits the top-level
  constraints keeps them on its first write, and the page says the scope has
  stopped inheriting. A draft can be forecast before any save: the candidate
  is judged by the runtime's own admission check over every recorded
  crossing, witnessed, reconstructed and previously refused each on its own
  line, and the forecast writes nothing.
- A denylist is not an access rule. `DeniedSourceHost` has set semantics and
  admission applies it before any access rule, so no rule allows past a
  denial. Access rules are the ordered form: `access_rule` constraints are
  read in declaration order, the first whose host pattern matches decides,
  and the actions are `allow`, `refuse`, `require_licence` for a named
  licence on that host, and `require_mediation`. `require_mediation` is
  accepted by the loader and met at admission; no other path reads it, so
  an observed crossing is not checked against it.

## Integration and ownership

- Harnesses and agent SDKs integrate and distribute Common Measure; they do not
  determine the core architecture. The binary integrates through MCP tools
  and hooks in any host that offers them, and through a screening-proxy
  contract; the Rust crates are a possible embedding surface and are
  documented as such.
- The binary on the machine is the product and each host holds a thin
  registration naming it. `commonmeasure install <host>` resolves the binary
  once and writes its absolute path into the host's own configuration,
  because a host's hook environment does not share the login shell's `PATH`;
  it writes only this product's entries and `uninstall` removes exactly
  those; replacing the binary changes what the next session runs with no
  reinstall. The Claude Code plugin is a second registration route for
  marketplace and archive installs and carries no binary in the checkout;
  its launcher resolves the binary on the machine, and only the standalone
  archive bundles one. One registration per host at a time: `install claude`
  refuses beside an enabled plugin, because two registrations record every
  crossing twice.
- Codex is mediated only. Codex's hooks documentation
  (`docs/contracts/host-integration.md` §2 cites it) states that hosted
  tools such as its web search do not use the local tool path and fire no
  `PostToolUse` hook, and its shell reaches the web as command text with no
  response a hook could attribute to a URL, so an observed matcher there
  could witness only third-party MCP results and none is registered. Codex
  hooks exist and `PostToolUse` carries `tool_response` for MCP tools; the
  hosted web search fires none, so the host's own fetch is mediated only.
  One `[mcp_servers.commonmeasure]` table serves
  the Codex CLI, the ChatGPT desktop app and the Codex IDE extension, and
  `install codex` writes `default_tools_approval_mode = "approve"` on it,
  because Codex asks before every MCP call otherwise and its
  non-interactive runs refuse a call that would ask. Codex sessions have no
  observed floor, no turn boundaries and no nudge, and the record claims
  none; the record does carry the client's own name and version, which is
  what tells the three surfaces apart.
- Claude Desktop is mediated only: it has no hook surface, and it starts one
  server for its chat client and one for its local agent mode; each server
  that makes a call leaves its own session naming its client, and the record
  does not merge them. Cursor has both paths: its hooks carry every tool's output, so the
  registration writes the four hook moments under Cursor's names and the
  hook command reads Cursor's payload shape when told `--host cursor`.
  Cursor (opt-in) and VS Code load Claude Code's hook file and run its
  commands with payloads of their own, and Cursor's documentation does not
  say which shape, so every hook command the Claude Code registration
  writes names `--host claude-code` and a reader told that refuses a payload
  of another host's shape, and any payload under Cursor's environment
  variable, recording and printing nothing. A host's own registration
  files are never deleted by `uninstall`: the host may have written them,
  so an emptied object stays.
- The Copilot CLI has both paths: `install copilot` writes the server into
  `~/.copilot/mcp-config.json`, which the GitHub Copilot app and VS Code's
  Agent Host also read, and four hooks under the CLI's camelCase names into
  a hook file of the product's own, `~/.copilot/hooks/commonmeasure.json`,
  which `uninstall` deletes when nothing else is in it. VS Code is mediated
  only: `install vscode` writes one `servers` entry in the user `mcp.json`;
  no hook is registered, because VS Code runs hooks only through the Copilot
  Chat extension, which loads Claude Code's hook files, ignores matchers and
  is not documented to carry a tool result. The Claude Code reader refuses
  the snake_case payload VS Code and the Copilot CLI send by its
  `timestamp`, which Claude Code never sends.
- Pi is mediated only, through an extension the binary writes. Pi has no
  MCP client, so the extension is the client: it spawns the mediated server
  from the binary it names and registers the server's tools with Pi under
  their own names. Pi has no web tool of its own, so nothing is observed.
- A turn boundary declares `privacy_level: minimal`, a constant of the edge
  rather than a policy field: the record holds no question, answer, intent
  or summary, so no higher level's claim could be honoured, and a projection
  may only lower what it carries.
- Provider credentials are operator state in the operator home
  (`$COMMONMEASURE_HOME/credentials.env`, default `~/.commonmeasure/`), parsed as
  `KEY=VALUE` and never sourced through a shell. The launching environment
  always wins, so a file can add a credential but never override the shell's
  choice; on unix a file readable by other users is refused by name. A
  present file that cannot be used fails the mediator at start; an absent
  file is the ordinary environment-only state. A loaded file is recorded as
  `credentials_loaded` with path, digest and variable names, never a value
  (`docs/contracts/session-evidence.md` §Credential provenance). The binary
  never reads a credential file from the working directory: a mediator that
  did would let any cloned repository plant attacker-chosen keys. The
  repository-root `.env` is a development convenience for scripts that source
  it explicitly, and `.env.example` is the one committed list of credential
  variable names, blank. The batch runner takes its credentials from the
  launching shell only, because a run's evidence contract does not record
  credential provenance.
- This repository is Common Measure Ltd's source-available product work.
  Third-party code and standards retain their notices.
- The product's web surfaces share one visual language, defined by the hub's
  token set: semantic names only, warm-gray neutrals, one teal accent that
  always means "you can act here", and three states (`ok`, `warn`, `err`)
  each with a `-soft` tint. The console's stylesheet (`console/styles.css`)
  and the guide's (`docs/guide/guide.css`) carry the same literal values as
  the hub's, and `crates/commonmeasure-console/tests/design_tokens.rs` enforces the WCAG
  2.2 contrast obligations offline. The console keeps one stylesheet, no
  build step and no scripts beyond htmx; the hub keeps its component library.
  The hub self-hosts Geist; the console and guide name it first and fall back
  to system stacks.

## Session policy and egress

- The reported engagement is resolved at read time from ordered attribution
  rules over the recorded working directory. Labels do not enter the evidence
  log. The governing engagement is a separate property declared on a policy
  scope. Neither derives from the other and neither overrides the other:
  attribution cannot feed enforcement, and policy cannot feed reporting,
  because read-time re-attribution is the property that lets an operator
  correct recorded history by editing a rule
  (`crates/commonmeasure-console/tests/store.rs`). The console is the one component that
  reads both names and it reports every scope where they differ; the relay
  names the governing engagement in its report and no reported one. An
  unmatched attribution rule resolves to `unattributed`, never to the
  governing name.
- Enforcement scope is resolved when a mediated crossing occurs and is
  recorded on that crossing.
- Session projections are filtered per reported engagement at the console.
  At the relay, each crossing's recorded cwd resolves through the ordered
  policy scopes. Only a matched scope with a named governing engagement and
  explicit telemetry clearance may project; client, unmatched, unnamed and
  undeclared work defaults to no egress.
- Clearance is read once, at projection. The spool is the record of an
  egress decision already taken, and a later relay run delivers it without
  re-reading policy; a batch spooled by an earlier invocation leaves as
  `SpooledBeforeThisRun`. Withdrawing `allow_telemetry_egress` stops future
  projection and recalls nothing already spooled
  (`crates/commonmeasure-relay/tests/relay.rs`). The manual recall path is deleting
  `outbound.ndjson` and `outbound.ack` under `~/.commonmeasure/relay/spool/`
  before the next run with a configured receiver. No automatic recall is
  built.
- A telemetry receiver's identity is the `(receiver URL, api_key)` pair in
  `relay.json`; the key binds every delivery to one organisation and is
  revocable there. Clearance stays per-engagement at the edge and names no
  receiver. The first receiver is the hub (Common Measure Hub, maintained
  separately) accepting the Content Telemetry v1.0 wire contract
  under an org-scoped key; the hub consumes no crate from this repository.
  A second, external conforming receiver is a named test target, so the
  relay is held to the standard and not to the hub. The hub delivers a
  registered owner's events onward to the owner's designated endpoint or
  consumer under the same clearance; it never opens an owner-facing view of
  an operator's evidence.
- No engagement name crosses the wire. Per-engagement fleet views on a
  receiver would need a contract conversation with the standard's home
  (`schema/SOURCE.md`: consumed, never edited locally) and a privacy decision
  of their own; neither is taken. A hub groups by organisation, API key, wire
  session, agent, host and supplier.
- A count of refused crossings per session crosses the wire, and nothing
  else about them: no URL, no reason, no hash. It travels as `refused` on
  every batch of the session, counting the refusals under scopes cleared
  for egress, so an organisation's owner can see policy enforced across
  the firm without any refused source leaving the machine
  (`docs/contracts/session-evidence.md` §The refused count on the wire).
- The supplier that served an acquisition crosses the wire, as the
  namespaced custom field `data.commonmeasure-supplier` on the retrieval and
  grounding events of a supplied source (`crates/commonmeasure-relay/src/project.rs`).
  A content owner, or the network reporting for it, can then tell a
  retrieval served through a licensed supplier from one fetched from the
  open web, which is the fact a supplier's usage report turns on. The name
  is the provider name a plan and a mediated search use. The operator's own
  corpus and a catalogued skill are not suppliers and are never named. What
  the acquisition cost — spend, quotes, receipts, allowance state — stays off
  the wire (§Delegated authority and fleet management).
- An institution identifier crosses the wire only under terms that require
  it. Where the operator holds terms with a source that attribute usage to
  the operator's subscription or agreement, the policy scope's terms entry
  for that host names the identifiers (a ROR or ISNI where one exists,
  otherwise the agreement id under an opaque scheme) and the session
  carries them in the standard's `access_context` container (§5.1.3), with
  turn data for that session at `intent` level or below. The identifiers
  name the operator as an institution, never a person, and are the
  operator's to declare; the hub may hold them as organisation facts and
  distributes them only inside the signed policy envelope
  (`docs/contracts/policy-envelope.md`). With no
  such terms the container is absent. This is the second identity beside
  the edge's key id: the key id says which edge fetched, the access context
  says under whose entitlement.

## Source declarations

- A reporting duty comes from the source, never from policy: either an RSL
  licence carrying the Content Telemetry reporting binding, discovered from
  the page, or terms the operator holds and references in `policy.json` as a
  fact about an agreement. The discovery manifest identifies an owner and its
  telemetry endpoint and carries no demand. Absent both, no duty exists and
  the record says so. The operator is responsible for what they declare.
- Manifest discovery runs once per new host, after the page bytes are
  fetched, and never delays a crossing. It is cached on disk with the
  response's cache age and a default age for a 404, and it is written as its
  own record kind, never as a crossing, so a probe can never project as a
  retrieval. Every outbound request the runtime makes for a manifest, a
  `robots.txt` or a licence goes through the same host-policy and
  address-checked path as a mediated fetch.
- "Provided by the user" means the user supplied the content bytes. A URL
  the user pastes is a reference; fetching it is acquisition by the system.
  Who named the source (`user`, `agent` or `unknown`) is recorded as a fact
  on every mediated crossing. What the operator's policy does with that fact
  is the operator's choice; it is not a carve-out from any preference.
- A mediated crossing records identifiers and hashes and never the content
  itself. The hash of what entered context is the whole claim a session makes
  about a source's bytes, and it is what a reviewer recomputes; the operator's
  session record is not a store of the content an agent read. A batch run is
  the other case and seals the provider's response under its own directory,
  because a run is a published experiment someone else has to reproduce.
- What a processor may read is not what the record retains. A processor
  receives the content its manifest declares it needs, in memory, for the one
  invocation: a screen reads the text to rule on it, and a verifier reads the
  answer and the sources it cites. What each leaves behind is its invocation
  record, which carries the hashes, the decision and what it found, never the
  bytes it read. So an add-on that has to see a mediated crossing's content
  can, and the crossing still retains none of it.
- A preference is not a licence and not enforcement: it is read, recorded
  with its source, and combined most-restrictive-wins; an absent statement
  is unknown, never disallow. The mechanisms and their standing are
  `docs/knowledge-base/source-declarations.md`.

## Delegated authority and fleet management

- Working-directory policy scope is not agent identity. A principal is
  authenticated from a basis the autonomous process cannot rewrite; an
  environment value may label work but is not authority by itself.
- Principal policy is an overlay before the engagement and directory overlay.
  Once a principal is configured to
  require a binding, an unmatched directory cannot fall through to a more
  permissive top-level policy.
- A periodic allowance is stateful local policy, not another `ContextJob`
  constraint. Allocation (what a principal may spend per period) is desired
  state in the declared policy artefact, authorable at the hub only inside
  the signed policy envelope (`docs/contracts/policy-envelope.md`).
  Enforcement (reservation,
  commit, release, reconciliation, refusal) is the edge's alone and works
  offline; no hub call sits in a crossing decision. Strict policy does not
  treat an unknown price, a native provider unit or an incomparable currency
  as zero. Reporting is a bounded allowance summary (identity, period,
  remaining state) on the fleet-status contract
  (`docs/contracts/fleet-status.md`), never per-crossing
  spend, quotes or receipts, and never on the Content Telemetry wire. The
  supplier's name is the one provider fact that crosses, as a custom field
  on the events it served (§Session policy and egress). The ledger itself
  stays local.
- A principal binding is keyed by an operating-system user id, which each
  machine assigns, so the same binding distributed to many edges names a
  different person on each. Until an identity that names the same person on
  every edge exists, a distributed policy declares no `principals` and an
  allowance is not authored at the hub. The hub refuses one at publishing,
  where a policy is written for many machines; the edge does not, because the
  same file written on the one machine it governs holds a binding correctly
  (`docs/contracts/source-policy.md` §Policy written for many machines).
- The source policy's format is published as a contract with a schema
  derived from the loader's types and vectors checked against the loader and
  the admission check (`docs/contracts/source-policy.md`). Anything that
  writes or checks a policy outside this repository is held to those
  vectors; it does not become a second policy engine, and the schema alone
  is never taken as validation, because it cannot see the loader's checks.
- An organisation's add-on mandate is desired state of the same kind as an
  allocation: authored at the hub only inside the signed policy envelope,
  a floor the edge enforces offline and refuses to relax locally, with the
  refusal recorded. The active add-on set and its settings are part of the
  effective-policy identity, so a fleet view cannot report two edges with
  different screens as converged.
- The edge is capable of complete local enforcement without a hub. The hub
  manages desired state, rollout and drift; decisions remain local. Remote
  policy is accepted only under a locally chosen deployment mode and a pinned
  trusted signer.
- A managed edge refreshes its policy envelope itself, at session start and
  before relay, and records each refresh. An expired envelope keeps
  enforcing the last accepted policy, and the record says it is stale since
  the envelope's expiry until the hub renews or replaces it; nothing waits
  on the refresh and no crossing waits on the hub
  (`docs/contracts/policy-envelope.md` §Cadence and staleness).
- A policy envelope names its revision by the digest of the policy as the
  envelope carries it, checked before the policy is parsed, so the hub never
  has to reproduce the edge loader's serialisation. The fleet-status
  document reports that digest as `applied.digest`, beside the loader's
  digest of the file in force and whether the file was edited since, so a
  receiver compares like with like whatever form it published; a revision
  keeps the digest it was published under
  (`docs/contracts/canonical-json.md` §Shared policy vectors).
- The edge authenticates to the hub's policy endpoint by signing the
  request with its enrolled key, using HTTP message signatures (RFC 9421,
  Ed25519), the same mechanism the mediated fetch uses towards publishers
  (`ROADMAP.md` §Verified fetcher identity). There is no second credential
  and no credential field in the deployment file. An edge that is not enrolled, or whose key
  the hub has revoked, makes no request and records why. Each
  implementation is exercised against the other's rule and both are pinned
  to one signature vector.
- A key is listed in the hub's key directory only with its holder's
  signature over the directory's authority, signed on the edge and uploaded,
  because the private key never leaves the edge and Cloudflare uses only the
  keys a directory response carries a signature by
  (`crates/commonmeasure-harness/src/identity.rs`). The edge signs a new
  proof, for the origin it enrolled under and no other, when it reaches the
  hub at `connect`, a relay run or a session or server start and the held
  proof is more than a day old. A key with no current proof is not listed,
  so every verifier reads the same key set, and the hub signs no directory
  of its own.
- The hosted tier is one multi-tenant service run by Common Measure Ltd at
  hub.commonmeasure.ai; whether an organisation may run a hub of its own is
  an open decision (§Open decisions).
- Retention at the hub: an organisation's raw delivered telemetry is kept
  for twelve months by default; the organisation may shorten that period;
  it is deleted on the organisation's request; and it is removed thirty
  days after the organisation closes its account. Aggregates and generated
  reports belong to the organisation for as long as the account exists.
- Every sealed hash and digest (the run manifest, replay captures, relay
  event ids and the conformance corpus, the policy identity) is computed
  over the canonical JSON form of RFC 8785, keys sorted, implemented once
  in the edge and once in the hub to the same rule
  (`docs/contracts/canonical-json.md`). The two implementations are
  separate on purpose: sides that agree by sharing a library have not been
  shown to agree, and the edge compares its digest against the hub's.
- Effective-policy digests are drift evidence. They are not described as
  attestation of an uncompromised edge unless a later protected device
  identity supports that claim. Fleet management metadata uses a separate
  contract and does not extend Content Telemetry with policy detail.

## Open decisions

- First licensed-supply partner for a funded live comparison.
- Initial evaluator suite and bounded job domain.
- Pricing of the institutional tier, and whether an organisation may run a
  hub of its own.
- Whether a preference addressed to `CommonMeasureBot` by name in
  `robots.txt` binds in every policy mode, with preferences in the `*` group
  following the operator's mode.
- Whether the C2PA image reader is in the first source-preference
  demonstration (`ROADMAP.md` §The demonstration for the IETF
  AI-preferences working group), or text only.
- Whether to obtain one third-party manifest publisher before claiming
  the acceptance of usage reporting to the sources that require it
  (`ROADMAP.md`), given that neither demonstration source is a third party.
- A namespace sweep for `commonmeasure` and `CommonMeasureBot` matching the
  one recorded for `contextops` (`docs/knowledge-base/naming-collision.md`).
- Whether the edge consults a public resolver of owners' designated
  consumers after a fetch, advisory only and cached, with its own discovery
  as the baseline.
- Whether a voluntary payment to a source that helped (`ROADMAP.md`
  §Support the source) falls under "pay a quoted price under an allowance" in
  `PRODUCT.md` §What it is not, or that boundary is restated first.
- The first settlement rail able to issue a pseudonymous sub-credential per
  enrolled edge, and whether its licence token and the Web Bot Auth
  signature are both verified on one request (`ROADMAP.md` §Licensed access
  through an external settlement rail).
