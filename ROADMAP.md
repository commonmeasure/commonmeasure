# Common Measure roadmap

This file states what is built, what is not, what is out of scope, and the
open work packages with their acceptance criteria. Git holds the history of
how each part was built. Open defects are in `docs/qa/OPEN.md`. Terms such as
crossing, admission, manifest, engagement and principal are defined in
`docs/GLOSSARY.md`.

A checkbox is ticked only when its acceptance evidence exists in the
repository, or in a named gitignored directory where the evidence is
unredacted licensed provider bytes.

## Next

The order below takes a stranger from the public release to a self-serve
product on both sides: the edge they install without help, and the hub they
enrol with without talking to Common Measure. Each entry names the package
it serves; a hub-side entry is on the hub's own board and has no box here.

1. The hub hosted at one public address, multi-tenant under Common Measure
   Ltd's own cloud account, serving the `CommonMeasureBot` identity
   documents from `https://hub.commonmeasure.ai`, the identity origin chosen
   once for every enrolled edge. Hub board. Nothing after enrolment can be
   exercised until it exists.
2. Self-serve onboarding at the hub: sign in, create an organisation, land
   on the enrolment page. Hub board.
3. WP-45: the walkthrough run to a first delivered batch against the hosted
   hub, with the hub's client pages served by the hub.
4. WP-44, boxes one to three: content owners, the report and unattributed
   hosts as screens.
5. WP-19, last box: managed policy run end to end against the hosted hub.
6. WP-46: the hosted edge, serving the mediated tools over HTTPS to the
   hosts that reach MCP only remotely. It follows the stranger's path
   because a hosted edge is an enrolled, managed edge whose users sign in
   through the hub, so it needs the hosted hub, onboarding and managed
   policy of entries 1, 2 and 5 working first.

Packages not on this path are under §Deferred packages, each with the reason
it waits.

## What is built

The core manages and measures every piece of content an agent takes in. Each
bullet names its test or artefact.

### Access rules: which content an agent may take in

- Deterministic admission policy: source hosts, licence requirements, cost
  caps, three policy modes, a negative test per constraint
  (`crates/commonmeasure-runtime/tests/end_to_end.rs`).
- Ordered access rules: a host pattern to allow, refuse, require a named
  licence or require the mediated path; first match wins; declared in a
  policy scope's constraints like any other; validated at load
  (`crates/commonmeasure-harness/src/policy.rs`).
- The source policy as a published contract: its fields, what scopes and
  principals replace, the admission order and every check the loader makes;
  a JSON Schema derived from the loader's types; validation and ruling
  vectors checked against the binary and the runtime; and
  `commonmeasure policy check`, which settles a candidate with the loader
  itself (`docs/contracts/source-policy.md`,
  `crates/commonmeasure-cli/tests/source_policy_contract.rs`).
- Policy scopes per working directory naming a governing engagement;
  principal resolution from a basis the process cannot rewrite, unauthenticated
  identity refused (`crates/commonmeasure-harness/src/policy.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- Periodic monetary allowances per principal from a local transactional
  ledger: reserved before dispatch, reconciled against the receipt, refused
  before money moves in `strict` (`crates/commonmeasure-runtime/src/allowance.rs`);
  at a mediated crossing, a search reserves the adapter's published price
  and a fetch reserves the price the source's licence quotes, before the
  request, and releases it on a receipt no rail charged
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- Mediated crossings rule before bytes move; observed crossings are recorded
  by hooks afterwards (`crates/commonmeasure-harness/src/mcp.rs`,
  `crates/commonmeasure-harness/src/hook.rs`).
- Turn boundaries carrying the host's turn identifier and a declared
  `minimal` privacy level, with observed crossings grouped under them
  (`docs/contracts/session-evidence.md` §Turn boundaries,
  `crates/commonmeasure-cli/tests/hook_e2e.rs`).
- Support-status rules and entitlement grants sealed per suite and evaluated
  at admission (`crates/commonmeasure-runtime/src/governance.rs`).
- Credentials loaded from the operator home and recorded as provenance
  without values (`crates/commonmeasure-supply/src/credentials.rs`).
- Policy identity: the canonical effective policy and its digest, recorded
  on every mediated crossing and session boundary, shown in the console,
  the CLI and `context_status`; the fleet-status document with the
  receiver's four answers, and drift demonstrated between two edge homes
  (`docs/contracts/fleet-status.md`, `crates/commonmeasure-harness/src/fleet.rs`,
  `crates/commonmeasure-cli/tests/fleet_status.rs`).
- Signed policy distribution on the edge: a locally chosen deployment
  mode, a pinned signer, the envelope's digest checked over the policy as
  carried and the policy validated through the policy loader, rollback and
  expiry refused, the last-known-good kept and enforced offline, the
  envelope refreshed at session start and before relay with each refresh
  recorded, and an expired envelope enforced and reported stale until the
  hub renews it (`docs/contracts/policy-envelope.md`,
  `crates/commonmeasure-harness/src/managed.rs`,
  `crates/commonmeasure-cli/tests/managed_policy.rs`).
- The PII detector's finding recorded on every mediated crossing; refused
  in `strict` on an internal or private source, admitted with the finding
  recorded on a public one, and refused everywhere under `refuse_on_pii`
  (`docs/FAIL-POLICY.md` §6, `crates/commonmeasure-runtime/src/processor/pii.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- A verifiable network identity: an enrolled edge signs every request it
  makes to a source — the mediated fetch and the `robots.txt`, licence and
  manifest probes beside it — and its managed-policy fetch, with the key
  minted at enrolment, as an RFC 9421 HTTP message signature under Web Bot
  Auth. A publisher admits `CommonMeasureBot` on the signature and the hub
  authenticates the policy fetch by the same one. An unenrolled or revoked
  key signs nothing and every record says so; a site that refuses the
  identity is recorded as a challenge
  (`crates/commonmeasure-harness/src/identity.rs`,
  `crates/commonmeasure-harness/src/mcp.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).

### The record: where content came from and what it cost

- A run publishes a sealed manifest, the exact provider bytes, an append-only
  evidence log with explicit gaps and a summary, atomically
  (`docs/contracts/run-output.md`).
- One canonical JSON form, RFC 8785, under every sealed hash, digest and
  derived id, in the edge and in the hub
  (`docs/contracts/canonical-json.md`,
  `crates/commonmeasure-types/src/canonical.rs`).
- Session evidence in three grades, observed, mediated and reconstructed,
  never merged (`docs/contracts/session-evidence.md`).
- The dossier: `commonmeasure inspect` explains a run with every claim citing
  its record (`crates/commonmeasure-cli/tests/inspect_dossier.rs`).
- The console: five sections over the evidence logs; the Compare query, the
  policy mode, a scope's denied hosts and the attribution rules are its
  writes, each revision-checked and saved through the artefact's own loader,
  with a dry-run forecast of a draft rule over the record by grade before any
  save (`crates/commonmeasure-console/src/console/edit.rs`,
  `crates/commonmeasure-console/src/console/forecast.rs`,
  `crates/commonmeasure-cli/tests/serve_e2e.rs`).
- Recorded replay through the real adapters, hash-verified
  (`crates/commonmeasure-runtime/src/replay.rs`, `demo/output/replay/`).
- Fifteen supply adapters: twelve open-web providers, the internal corpus,
  Redpine licensed content with quote then confirm, and Ozone Live licensed
  publisher retrieval
  (`docs/knowledge-base/provider-verification.md`); a local skill invoked as
  a plan (`demo/output/skills/`).
- The relay projects cleared, witnessed facts as Content Telemetry v1.0 to
  one configured receiver, nothing sent by default, with a count of the
  session's refused crossings on each batch and nothing else about them
  (`crates/commonmeasure-relay/tests/conformance.rs`,
  `conformance/session-refused.json`).
- Attribution rules project one evidence store per reported engagement at
  read time (`crates/commonmeasure-console/src/attribution.rs`).

### Measurement: whether the content helped

- Three deterministic evaluators per plan behind versioned rule texts:
  grounding, coverage, freshness (`crates/commonmeasure-runtime/src/evaluate.rs`,
  `coverage.rs`, `freshness.rs`).
- Selection over measured terms only; a missing measurement makes the router
  abstain and name it (`demo/output/latest/`).
- Committed comparisons: source mix (`docs/READ-A-COMPARISON.md`); a fitted
  routing rule beaten on the holdout by per-run measured selection
  (`docs/READ-A-HOLDOUT.md`); a revocation pair (`demo/output/specialist/`);
  the injection screen (`demo/output/injection/`).
- Context assembly inventory per boundary from the host's own transcript
  records: nine categories with explicit characters/4 footprints, the three
  capability states (listed on demand, definition loaded, invoked) apart,
  categories no basis can see reported unavailable, and the change between
  boundaries attributed in `commonmeasure session`
  (`crates/commonmeasure-harness/src/snapshot.rs`,
  `crates/commonmeasure-cli/tests/hook_e2e.rs`).
- Fidelity verification of each answer: per-claim verdicts with the window
  span cited, and an optional model judge measured for agreement against
  them (`crates/commonmeasure-runtime/src/processor/fidelity.rs`,
  `crates/commonmeasure-runtime/src/processor/judge.rs`, `demo/output/cited/`).

### Distribution: the binary without a checkout

- One version identity: the workspace manifest, reported by the binary and
  carried by the plugin manifest and the archive
  (`crates/commonmeasure-cli/tests/version_identity.rs`, `plugin/package.sh`).
- A tag builds Linux x64 and arm64, Windows x64 and macOS arm64 and x64,
  publishes them with SHA-256 checksums to a public release under the notes
  committed for that tag, and packages the plugin archive from the same
  bytes (`.github/workflows/release.yml`).
- An installer that verifies the checksum before the binary reaches `PATH`
  and names what it cannot do (`install.sh`,
  `crates/commonmeasure-cli/tests/installer.rs`); installer and archive
  exercised in a clean container (`docs/RELEASE.md`).
- Host registrations the binary writes, verifies and removes by absolute
  path: `commonmeasure install|uninstall|doctor` for Claude Code (hooks and
  the MCP server), Codex (the MCP server table), Pi (an extension that
  runs the MCP server), Claude Desktop (the MCP server), Cursor and the
  Copilot CLI (the MCP server and hooks), VS Code (the MCP server) and
  Chrome (the native messaging host the browser extension in `browser/`
  reaches), touching only this product's entries
  (`crates/commonmeasure-harness/src/registration.rs`,
  `crates/commonmeasure-cli/tests/install_e2e.rs`); the plugin holds a
  manifest, hooks and a launcher and no binary (`plugin/README.md`).
- Real sessions recorded through the Claude Code, Codex and Pi
  registrations, committed as fixtures: a Claude Code session with
  observed, mediated and turn records, four boundaries and a compaction, a
  Codex CLI run and a Pi session, each with a mediated crossing
  (`demo/host-sessions/`,
  `crates/commonmeasure-cli/tests/recorded_sessions.rs`).

### First-party add-ons

Add-ons are processors on the same stream the core manages, behind
`docs/contracts/processor.md`, each invocation an evidence record. Shipped,
in-process: the `pii-detector`, the `injection-screen` and the support-status
governor at admission, the `html-text-extractor` over each mediated fetch,
the `context-optimiser` before inference, the
`fidelity-verifier` after it, where a suite declares it, the
`fidelity-judge`, and, where a suite declares `output_provenance`, the
`output-provenance` labeller over each answer
(`crates/commonmeasure-runtime/src/processor.rs`). Not shipped: source
quality for coding agents (WP-35), support the source (WP-36), learned
rerankers, compressors and screening; sidecar and remote processors exist in
the contract only. Every shipped processor runs wherever it is wired:
enabling, disabling and settings per scope, and organisation-level mandates,
are WP-34.

## Acceptance gate

The first version passes when every line below passes. The state of each:

- at least three open-web providers and one licensed provider run through the
  same contract — **passed for delivery, not for price**: twelve open-web
  adapters and Redpine (`demo/output/redpine-replay/`); Redpine delivered
  licensed content under a trial meter and no currency charge has been
  observed;
- a strict policy demonstrably blocks an ineligible crossing — **passed**:
  `demo/output/injection/` and the `arc-mediated-strict` session
  `just demo-arc` regenerates (`docs/RUN-THE-DEMONSTRATION.md`);
- the same job can be replayed without silently changing its inputs —
  **passed**: `demo/output/replay/` (`docs/READ-A-RUN.md`);
- the run dossiers expose a real quality/cost/latency trade-off —
  **passed for measured fidelity, not for a quality score**: coverage,
  freshness, cost and latency are measured and ranked (`demo/output/latest/`);
  each answer carries per-claim fidelity verdicts and, where the suite
  declares it, a model judge's agreement with them (`demo/output/cited/`);
  no universal quality score exists, by decision (`DECISIONS.md` §Execution
  and evidence);
- the router improves an operator-owned objective on a holdout set it never
  saw — **passed for per-run measured selection, failed for a fitted rule**:
  per-run selection over measured terms beat the fixed default on the
  holdout, and the routing rule fitted on the training split lost to that
  default, so no fitted rule ships (`docs/READ-A-HOLDOUT.md`, `DECISIONS.md`
  §Product);
- the Content Telemetry projection validates against the standard —
  **passed**: `crates/commonmeasure-relay/tests/conformance.rs` against the pinned
  v1.0 schemas (`schema/SOURCE.md`);
- the private run record and the supplier-facing projection are demonstrably
  different data products — **passed**: `conformance/` holds the projection;
  `docs/contracts/session-evidence.md` states what never leaves.

The gate fails if the provider comparison cannot be made fair, the
evaluation does not change a routing decision, or a fixed default performs
as well as the router after accounting for complexity. Out of the first
version: auctions and supplier bidding; settlement, custody or publisher
payouts; learned online routing before controlled evidence exists; fleet
scheduling; a consumer browser product.

## What is not built

- Usage reporting to the content sources that require it (WP-23).
- WP-19's synchronisation against the hosted hub, recorded in this
  repository. The edge signs the request with its enrolled key and a
  loopback endpoint verifies it as the hub does; no transcript of a run
  against the hub is committed.
- Console editing of policy fields beyond the mode and the denied hosts:
  access rules, scopes, principals, allowances, clearance and recordable
  prefixes are edited by hand.
- An observed record of a host tool call the host reports as failed: a
  failed `WebFetch` fires `PostToolUseFailure`, which no registration
  carries, so the attempt leaves no observed record.
- One session log for a conversation's hooks and its mediated crossings on
  Claude Code: the host starts the MCP server without a session identifier,
  so mediated crossings land under the server's own `local-*` id.
- Registration with Cloudflare's verified-bots programme (WP-29). The
  requests are signed and the key directory is published; the registration
  needs the account.
- The recorded demonstration for the IETF AI-preferences working group
  (WP-31).
- Reproduction, citation and presentation events projected from evaluator
  evidence (WP-32).
- Licensed access through an external settlement rail (WP-33).
- Add-on management: enabling, disabling and settings per scope, the
  active set in evidence, organisation mandates (WP-34).
- Source quality for coding agents: repository and package checks at
  admission (WP-35).
- Support the source: a voluntary payment to a source that helped (WP-36).
- The sidecar processor boundary for third-party add-ons (WP-37).
- The add-on catalogue and marketplace at the hub (WP-38).
- A recorded session through Claude Desktop, Cursor, the Copilot CLI, VS
  Code, the ChatGPT desktop app, the Codex IDE extension or any browser
  answer surface; `install` for Gemini CLI and Goose; and Claude in the
  browser (WP-39, WP-46).
- The console's API and its screens agreeing, with this month's crossing
  evidence on a screen (WP-41).
- A read-only management surface an agent can ask about its own record
  (WP-42).
- Fleet status delivered to a hub and shown there (WP-43).
- The hub's screens for content owners, delivery, policy revisions and
  unattributed hosts (WP-44).
- A charge in currency is observed for one provider (Exa); the rest quote a
  published price, and Redpine's funded price is unknown.
- Relay delivery cadence: retry, backoff and dead-letter for spooled Content
  Telemetry batches. The spool is durable and delivery counts only on the
  receiver's acceptance; recall is manual.
- Crossings the runtime carries but cannot observe, such as an opaque tunnel
  through a forward proxy; no proxy surface exists.
- Multi-process contention for runs: one run per output directory is assumed.
- Clock discipline: gap windows use the local wall clock.

## Out of scope

- Brand and AI-visibility reporting is a separate company's use of this
  product and is not built here.
- Monetisation of outputs, a supply-facing display of demand, a marketplace
  and a publisher-as-operator persona are not built and not planned.
- Exchanges, auctions and settlement rails stay external. A model gateway
  handles provider compatibility, credentials and retries.
- An owner-side telemetry platform: owner registration, domain verification,
  owner dashboards and agent reconciliation. Owners are served by the
  conforming consumers they choose; the edge reports to them and the hub
  delivers onward to them.

## Open packages

### WP-18: effective-policy identity and fleet status

Make applied policy comparable across edges without exporting the policy
document.

- [x] A canonical effective-policy representation (mode, constraints,
  principal and basis, governing scope, allowance identity, resolver and
  schema versions), hashed so an unrelated scope edit creates no false drift.
- [x] The identity recorded at mediated crossings and session boundaries;
  older evidence lacks the field explicitly.
- [x] Policy identity and local allowance state shown in console and CLI.
- [x] A versioned fleet-status contract separate from Content Telemetry: edge
  identity, desired revision, applied identity, versions, last enforcement
  heartbeat, a bounded allowance summary; no prompts, answers, policy
  document, per-crossing spend or engagement names.
- [x] Drift demonstrated between two edge homes; no drift when only an
  unrelated scope changes.

**Acceptance evidence:** a reviewer recomputes an edge's identity locally; a
receiver distinguishes current, stale, divergent and unknown without the
policy. Documentation calls this drift evidence, not attestation.

### WP-19: optional signed policy distribution

Depends on WP-18. The edge side is built. The edge authenticates to the
hub's policy endpoint by signing the request with its enrolled key
(WP-29); an edge that is not enrolled, or whose key was revoked, makes no
request and records why.

- [x] A locally chosen deployment mode: `local` accepts no remote policy;
  `managed` pins one signing identity. Nothing remote can switch it.
- [x] A signed desired-policy envelope (organisation and edge binding, ordered
  revision, issued and expiry times, schema version, digest), validated
  through `PolicyDocument` before activation; no second policy engine.
- [x] Wrong signer, wrong edge, invalid schema, expiry and rollback rejected;
  last-known-good kept and reported across failure; invalid remote state never
  relaxes local enforcement.
- [x] Policy fetch and status kept off the telemetry receiver path.
- [ ] Exercised against the self-hosted and managed hub shapes; an
  unavailable or compromised hub gains no access to the operator record and
  is never needed for a crossing. The edge half is exercised against a
  loopback endpoint that verifies the request signature as the hub does
  (`crates/commonmeasure-cli/tests/managed_policy.rs`); no transcript of a
  run against the hub is committed here.

**Acceptance evidence:** signed rollout from desired to applied, visible
drift before convergence, rollback refused, a complete offline
last-known-good run; `local` mode makes no management request, and an
edge with no key to sign with makes none either.

### WP-21: the binary carries the harness integrations

The binary on `PATH` is canonical; each harness holds a thin registration the
binary writes, verifies and removes, the pattern of tools that live inside
other programs (`gh`, `direnv`, `starship`): one artefact to update, each
host integration a subcommand. Depends on the released binary
(`docs/RELEASE.md`). Three consequences hold
throughout. Path resolution is written down, not assumed: harness hook
environments do not share the login shell's `PATH`, so `install` resolves
the binary once and writes the absolute path into the registration, with
the bundled launcher as fallback. The bundled-binary archive survives for
installs where fetching a release artefact is not possible, as a packaging
of the same bytes, not a second product. The plugin's removal properties
(no global settings edits, uninstall removes every surface, the evidence in
`~/.commonmeasure/` outlives the plugin) are acceptance criteria for every
host added. The package adds no capture, policy or evidence semantics; a
host whose integration would need new semantics gets its own package.

- [x] `commonmeasure install claude` registers hooks and the MCP server by the
  resolved absolute path, bundled launcher as fallback; `commonmeasure install
  codex` performs the mediated-only MCP registration; each writes only that
  host's own surface and `uninstall <host>` removes exactly that
  (`crates/commonmeasure-cli/tests/install_e2e.rs`).
- [x] `commonmeasure install pi` follows the same shape, MCP registration
  first: one extension under Pi's agent directory that spawns the mediated
  server with Pi's session id and registers its tools
  (`crates/commonmeasure-harness/src/pi_extension.ts`,
  `crates/commonmeasure-cli/tests/install_e2e.rs`, `demo/host-sessions/pi/`).
- [x] `commonmeasure doctor` reports per host: registered, resolved binary path
  and version, whether recording works, policy file status
  (`crates/commonmeasure-cli/tests/install_e2e.rs`).
- [x] The plugin thins to manifest, hooks and launcher; bundled binaries stay
  only in the standalone archive (`plugin/bin/commonmeasure-launch`,
  `plugin/package.sh`).
- [x] Uninstall removes every added surface; `~/.commonmeasure/` outlives both
  (`crates/commonmeasure-cli/tests/install_e2e.rs`).

**Acceptance evidence, per box.** Box 1, Claude Code: `install claude` from
a binary outside the checkout, then a real four-turn session whose record
is committed at `demo/host-sessions/claude-code/` (observed crossings, one
mediated crossing in the server's own log, four boundaries, one
compaction), read by `crates/commonmeasure-cli/tests/recorded_sessions.rs`;
`uninstall` restored both host files, pinned by `install_e2e.rs`. Box 1,
Codex: the table is written, read back and removed by `install_e2e.rs`, and
a real Codex CLI run through it recorded the mediated crossing committed at
`demo/host-sessions/codex/`, read by `recorded_sessions.rs`
(`docs/contracts/host-integration.md` §6). Box 2, Pi: the
extension is written, read back and removed by `install_e2e.rs`, and a real
Pi session through it recorded the mediated crossing committed at
`demo/host-sessions/pi/`. Boxes 3 to 5: `install_e2e.rs`.

### WP-22: one-command hub enrolment

The edge half. The hub-side enrolment surface (token issue and exchange,
key registration, the key directory) is on the hub's own board.
There is no default hub: `connect` with nothing named refuses, and the
fresh-checkout posture, nothing leaves the machine, is unchanged. Enrolment
is also where an edge becomes `CommonMeasureBot` to publishers
(`DECISIONS.md` §Product): the key it mints here is the key WP-29 signs
with, and the key id is the pseudonymous identifier a publisher sees and a
settlement rail binds a sub-credential to (WP-33).

- [x] `commonmeasure connect <hub-url> --token <token>` obtains an
  org-scoped ingest key, writes the relay configuration, makes a first
  relay run through the real delivery path, and reports what it did
  including the clearance each delivered event left under
  (`crates/commonmeasure-relay/src/enrolment.rs`,
  `crates/commonmeasure-cli/tests/connect_e2e.rs`).
- [x] A short-lived enrolment token exchanged for the ingest key, so the
  long-lived credential never passes through a chat or shell history;
  pasting a key remains the fallback.
- [x] The edge generates an Ed25519 key pair during `connect`; the private
  key stays in `~/.commonmeasure/edge-key.json`, owner-readable only, and
  never leaves the machine. The enrolment call registers the public key
  under the organisation with a proof of possession; the hub assigns the
  key id (the RFC 7638 JWK thumbprint) as the edge's pseudonymous
  identifier and returns it; every session recorded from then on carries
  the key id in its evidence (`edge_identity`,
  `docs/contracts/session-evidence.md`).
- [x] The hub publishes each enrolled public key in the `CommonMeasureBot`
  key directory and agent card on the product domain (the WP-29 surfaces).
  Revocation at the hub removes the key from the directory; the edge learns
  of it on its next relay run, records it, and stops signing. An edge that
  cannot reach the hub keeps signing until the directory's cache age
  expires at the publisher, which is the bound the bot page states.
- [ ] Where the organisation has a settlement rail configured at the hub,
  `connect` provisions the edge's sub-credential and stores it only on the
  edge, bound to the key id (WP-33); with no rail, `connect` completes
  without one and says so. Waits on the first rail able to issue a
  pseudonymous sub-credential (`DECISIONS.md` §Open decisions); the hub
  exchange carries no rail field yet.
- [x] No argument and no configured hub refuses and names what is missing;
  `commonmeasure disconnect` deletes `~/.commonmeasure/relay.json`, the
  key and the enrolment record, and revokes the key at the hub when the
  hub is reachable; when it is not, the command says the key must be
  revoked from the hub.
- [x] `connect` does not enable managed policy mode (WP-19): it writes
  `relay.json`, `edge-key.json` and `enrolment.json` and touches no policy
  file.
- [x] Exercised against a real hub: the ignored live test
  `a_real_hub_enrols_revokes_and_disconnects_this_edge` in
  `crates/commonmeasure-cli/tests/connect_e2e.rs` runs the binary against
  a running hub server, minting the token through the
  hub's API as an owner; its doc comment says how to run it, and
  `demo/enrolment/real-hub-run.txt` is the transcript of one run
  (`demo/enrolment/README.md`).

**Acceptance evidence:** a fresh edge reaches its first delivered batch
through one command; the console's egress panel reports receiver, count and
key id; the edge's key appears in the directory after `connect` and is gone
after revocation; a session recorded after revocation names the revoked key
id and records that signing stopped; the no-argument case refuses; nothing
is projected before `connect` completes. All but the rail sub-credential
are covered by `crates/commonmeasure-cli/tests/connect_e2e.rs` against a
loopback hub and by the real-hub transcript above; that signing stops at
revocation is covered by `crates/commonmeasure-cli/tests/mediated_e2e.rs`.

### WP-23: usage reporting to the sources that require it

A reporting requirement is communicated by the source itself; policy respects
a discovered declaration and never originates the duty. A crossing never waits
on reporting; a delivery whose clearance, destination or manifest cannot be
established is withheld and recorded as withheld. The hub is the first
consumer: the operator's own telemetry consumer in the standard's terms. It
delivers each owner's events onward to the consumer or endpoint the owner
designates and holds no owner accounts; the owner-side platform (owner
registration, domain verification, owner dashboards) is run by others
(§Out of scope).

- [x] The wire speaks the published standard: schema pin `schema/SOURCE.md`,
  `provenance` and `cached` omitted on grounding events because the hook
  cannot observe the host's cache (`crates/commonmeasure-relay/tests/conformance.rs`).
- [x] The standard's `manifest.json` schema vendored into `schema/`
  (`schema/SOURCE.md`) and its manifest fixtures added to the conformance
  test (`schema/manifest.v1.json`, `schema/manifest-tests/`,
  `crates/commonmeasure-relay/tests/conformance.rs`,
  `crates/commonmeasure-harness/tests/manifest_fixtures.rs`).
- [x] Manifest discovery, under the rules in `DECISIONS.md` §Source
  declarations: after a mediated fetch succeeds, resolve
  `/.well-known/content-telemetry.json` on the final URL's host, then on the
  apex when a subdomain answers 404, through the same address-checked path
  as the fetch, with its own shorter timeout; validate per the standard's
  consumer rules (404 or a network error leaves the host unverified and
  rejects nothing; invalid JSON, a schema failure, duplicate key ids or a
  `domains` entry outside the manifest's own host rejects the manifest; any
  `1.x` is accepted); cache on disk by host with the response's cache age
  and a default age for 404; write a `manifest_resolved` record carrying the
  host, the URL fetched, the HTTP status, the cache decision and either the
  manifest's key facts or the rejection reason. The crossing references the
  manifest record. Loopback tests cover found, 404, invalid JSON, schema
  failure, foreign `domains`, apex fallback and cache reuse; the relay
  projects nothing from the new record
  (`crates/commonmeasure-harness/src/manifest.rs`,
  `crates/commonmeasure-harness/src/discovery.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`,
  `crates/commonmeasure-relay/tests/relay.rs`).
- [x] A `Content-Telemetry-ID` header on each mediated fetch to a host where
  a manifest or licence was discovered, and the same id on the retrieval
  event, so an owner can match reports to its own logs
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`,
  `crates/commonmeasure-relay/src/project.rs`, `conformance/`).
- [x] The HTTP status recorded on every mediated crossing, and a non-2xx
  answer never projected as a retrieval; the conformance corpus regenerated
  (`crates/commonmeasure-relay/src/project.rs`, `conformance/`). The hub's
  pinned copy is taken from this corpus.
- [x] The reporting demand ruled on at admission: meet it, or refuse in
  `strict` and record the breach in `observe` and `prefer`. Needs
  `require-mediation` in the constraint vocabulary (WP-24). A demand is met
  where its profile is the Content Telemetry binding, its level is one the
  relay emits, the session's scope clears telemetry egress and a receiver
  is configured in `relay.json`; the ruling and the receiver are on the
  crossing (`crates/commonmeasure-harness/src/mcp.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [ ] Turn boundaries with a declared `privacy_level` (WP-27), the one unmet
  MUST of the Grounding level.
- [ ] `access_context` on the session where, and only where, the policy
  scope's terms for the host name institution identifiers (`DECISIONS.md`
  §Session policy and egress). The wire type gains the `data.access_context`
  container the pinned schema already carries; turn data for such a session
  is projected at `intent` level or below; loopback tests cover present,
  absent, and a host whose terms name no identifier. The edge half is
  built: a terms entry names the identifiers, the crossing records them,
  and the relay withholds such a session and says why
  (`crates/commonmeasure-relay/tests/relay.rs`). The pinned batch schema
  carries no session `data`; only the session document does, and the hub
  accepts event batches alone, so the container waits on session-document
  delivery at the hub.
- [x] The supplier that served an acquisition on the wire, as the namespaced
  custom field `commonmeasure-supplier` on the retrieval and grounding events
  of a mediated search result and of a run's admitted sources; the mediated
  search records the provider on each result crossing; the operator's own
  corpus and skills are never named; the corpus carries a supplied session
  and a supplied run (`DECISIONS.md` §Session policy and egress,
  `crates/commonmeasure-relay/src/project.rs`,
  `crates/commonmeasure-relay/tests/relay.rs`, `conformance/`). Carried
  end to end against a real hub: one live Ozone Live search through the
  mediated tool, relayed, resolved by the hub to the three publishers
  registered as a network, delivered onward to a consumer, and reported
  filtered to that supplier (`demo/enrolment/ozone-pilot-run.txt`,
  `demo/enrolment/README.md`).
- [ ] Onward delivery from the hub: each registered owner's events delivered
  to the endpoint named in the owner's licence reporting binding or manifest,
  or to the consumer the owner names, at event granularity, behind the same
  clearances; the edge keeps one receiver and no default egress. The owner
  report stays the operator's hand-over artefact; no owners page and no
  owner accounts at the hub. The hub side is built for one destination per
  owner, registered by exact host or by the parent domain, singly or as a
  network from an ownership map with a shared destination, with a durable
  outbox, backoff, dead-letter and requeue, `content_telemetry_id` carried
  onward, and a per-network usage report; the destination is the one the
  operator registers or the hub discovers in the owner's manifest
  (`demo/enrolment/ozone-pilot-run.txt` is one run). Not built: delivery
  routed by the licence, where a source's licence carries the reporting
  binding and the hub delivers that owner's events to the consumer the
  binding names, the operator's registration standing as the override and
  the delivery status stating which route delivered each batch. `content_id`
  prefix resolution at the hub waits on the edge emitting `content_id`.
- [ ] The relay held to the standard, not to the hub: the ignored live test
  against an external conforming receiver
  (`crates/commonmeasure-cli/tests/relay_e2e.rs`) run against that
  receiver's current code, its setup steps pointing at paths that exist, and
  the run recorded in the acceptance evidence.
- [ ] Coverage declared `selected` with the exclusion rule stated; `complete`
  never claimed.
- [ ] Grounding level only; the ingestion measure as code points at capture
  or `tokens_ingested` with `token_basis` as a custom field.

**Acceptance evidence:** one real source with a published manifest receives
events about its own content from a real session at the consumer the source
designates, inspectable from crossing to delivery report; a source whose
demand cannot be met is refused before bytes move in `strict`, with the
refusal naming the demand.

### WP-24: console policy editing

Conditions for any console write: `DECISIONS.md` §Execution and evidence.

- [x] C1: the Policy screen shows each scope where the reported and governing
  engagement differ, both identities and the disagreement (`/api/policy`
  already carries it) (`crates/commonmeasure-console/src/console/app.rs`,
  `crates/commonmeasure-cli/tests/serve_e2e.rs`).
- [x] C2: policy mode, denied source host and attribution rule editing as
  htmx writes under the Compare guard, revision-checked, enforcing nothing
  (`crates/commonmeasure-console/src/console/edit.rs`,
  `crates/commonmeasure-cli/tests/serve_e2e.rs`).
- [x] Ordered access rules (host pattern to allow, refuse, require-licence,
  require-mediation; first match wins; scoped to a policy scope), gated on
  ordered constraint semantics, a host on `require-licence`, and
  `require-mediation` in the vocabulary
  (`crates/commonmeasure-harness/src/policy.rs`,
  `crates/commonmeasure-runtime/tests/end_to_end.rs`).
- [x] A dry-run forecast over recorded history for a draft rule, witnessed
  and reconstructed separate, before any save
  (`crates/commonmeasure-console/src/console/forecast.rs`,
  `crates/commonmeasure-cli/tests/serve_e2e.rs`).

**Acceptance evidence:** each write validates through the runtime's loader,
refuses a conflicting edit with both versions stated, and has a negative-path
test in `crates/commonmeasure-cli/tests/serve_e2e.rs`.

### WP-25: output provenance

A first-party add-on: an output-stage processor that builds a C2PA manifest
for an output from the sealed source record, using the C2PA Rust SDK
(`docs/knowledge-base/source-declarations.md` §C2PA).

- [x] Every grounded source is an ingredient referenced by content hash with
  its grade (`docs/GLOSSARY.md` §Grade), so a mediated ruling and an
  observed witness are never one claim
  (`crates/commonmeasure-runtime/tests/output_provenance.rs`).
- [x] A `c2pa.created` action with the trained-algorithmic-media source
  type; a custom assertion carrying the session and event ids and the
  operator's own manifest reference; a training-and-data-mining assertion
  set from operator policy; an identity assertion for the operator. The
  manifest carries hashes and ids, never prompts or responses
  (`crates/commonmeasure-runtime/tests/output_provenance.rs`).
- [x] The manifest embedded in text output under C2PA Annex A.8 (invisible
  variation selectors that survive copy and paste), so a second session can
  read the output's own provenance back
  (`crates/commonmeasure-cli/tests/provenance_readback.rs`).
- [x] Signing identity left to the operator. Without a trust-list
  certificate the signature verifies as valid but untrusted, and the record
  says so. Each invocation recorded
  (`crates/commonmeasure-runtime/tests/output_provenance.rs`,
  `demo/provenance/README.md`).
- [ ] A committed labelled run: `just provenance-example` produces one
  from the replay recordings under the demonstration identity
  (`demo/jobs/eu-ai-act-replay-provenance.json`). It needs an inference
  gateway, because a plan without an answer has nothing to label; the run
  it produces is not committed.

**Acceptance evidence:** a committed run whose manifest re-derives from its
source record, with a test that fails when a hash or grade is altered, and a
second session that reads the embedded manifest back from the pasted output.
The two tests exist over runs made in the test; the committed run is the
open box above.

### WP-26: fidelity verification

A first-party add-on attaching claim and citation support, contradiction and
unsupported-claim evidence to an output, with no universal truth score. The
grounding evaluator is the deterministic first step.

- [x] Grounding extended from opening-word quotes to mid-text spans through
  the runtime path (`demo/output/cited/`, pinned by
  `crates/commonmeasure-runtime/tests/cited_artefact.rs`).
- [x] Per-claim verdicts on the output record with the span cited
  (`crates/commonmeasure-runtime/src/processor/fidelity.rs`,
  `crates/commonmeasure-runtime/tests/fidelity.rs`; rendered and rechecked
  by `commonmeasure inspect`).
- [x] An optional model judge measured for agreement against the
  deterministic checks; its verdict is never ground truth
  (`crates/commonmeasure-runtime/src/processor/judge.rs`, `demo/output/cited/`).
- [ ] Only `supported` verdicts may project as telemetry (WP-32).

**Acceptance evidence:** a committed run with a `supported` verdict on a
mid-text span, and a judge agreement figure beside the deterministic
verdicts: `demo/output/cited/`. In that run the model's prose paraphrases
the sources it cites, so the verifier finds no verbatim-supported claim and
the judge calls every claim supported; the agreement figure records that
disagreement rather than resolving it.

### WP-27: host observation gaps

Session evidence records what the host lets the hook see.

- [x] Turn boundaries: an observed crossing without its turn's question is
  useful once it carries the host's turn identifier, which Claude Code
  supplies as `prompt_id` on every hook and Codex as `turn_id`; the boundary
  carries the same identifier and the transcript path, and declares
  `privacy_level: minimal`, the only level a record holding no question,
  answer, intent or summary can honour
  (`docs/contracts/session-evidence.md` §Turn boundaries,
  `crates/commonmeasure-cli/tests/hook_e2e.rs`).
- [x] Context categories: no host reports them at a hook boundary, and
  Claude Code's transcript carries the host's own record of what it injected
  (one `attachment` record per instruction file, skill listing, subagent
  listing, tool listing, loaded definition and file read, and a prompt
  snapshot in newer host versions); the snapshot files those records and the
  message blocks into ten categories with explicit characters/4 footprints
  and names the rest unavailable. Pinned against a real transcript with its
  text replaced by same-length filler
  (`crates/commonmeasure-harness/tests/recorded/claude-code-transcript.jsonl`,
  `crates/commonmeasure-harness/src/snapshot.rs`,
  `crates/commonmeasure-cli/tests/hook_e2e.rs`).
- [x] Available-on-demand capabilities kept separate from loaded definitions
  and from invocation evidence: the transcript's `deferred_tools_delta`
  names, its `deferred_tools_record` entries and `tool_reference` results,
  and its `tool_use` blocks, recorded as three separate fields
  (`crates/commonmeasure-harness/src/snapshot.rs`).
- [x] Snapshots compared across one real session: `commonmeasure session`
  prints the API-reported change between consecutive boundaries beside the
  estimated growth of host scaffolding, conversation and acquired content,
  the witnessed crossings in the interval and the unattributed remainder.
  The real session is the committed Claude Code record
  (`demo/host-sessions/claude-code/`), four boundaries with a compaction
  between the last two, and `crates/commonmeasure-cli/tests/recorded_sessions.rs`
  pins the report over it (`crates/commonmeasure-cli/tests/hook_e2e.rs` for
  the synthesised shape).
- [x] Codex: mediated crossings only. Codex's hosted web search fires no
  hook and its shell reaches the web as command text, so an observed matcher
  could witness only third-party MCP results (`DECISIONS.md` §Integration
  and ownership, `docs/contracts/host-integration.md` §2).
- [x] Claude Code built-in `WebFetch` and `WebSearch`: the hook fires for
  both and the matcher covers both. The committed real session
  (`demo/host-sessions/claude-code/`) holds the observed rows for its
  `WebFetch` and `WebSearch` calls beside the transcript fixture that shows
  the calls. A session records nothing when its plugin's marketplace
  directory has moved, because Claude Code then reports the plugin as
  failed to load and no hook fires; `doctor` reads the recorded install
  path and marketplace directory and reports either missing
  (`crates/commonmeasure-cli/tests/install_e2e.rs`). A call the host reports
  as failed fires `PostToolUseFailure`, which is not registered (§What is
  not built).

**Acceptance evidence:** each box names the host observation it used and the
test that pins the record's shape.

### WP-29: verified fetcher identity

The mediated fetch identifies itself honestly and is verifiable for it. An
enrolled edge signs every request it makes to a source with the key minted at
enrolment, so a site can admit `CommonMeasureBot` on the signature rather than
on a name anything can put in a header. A site behind Cloudflare's bot challenge, for
example the SRA's guidance page on the misuse of AI
(`https://www.sra.org.uk/solicitors/guidance/misuse-ai/`), answers an
unverified fetcher with 403 and `cf-mitigated: challenge`; a fetcher that
cannot run JavaScript cannot pass a challenge, so a self-identifying tool is
admitted only where the site operator can verify who it is. Spoofing the user
agent is not an option: the record's claim about who fetched would be false,
and the consent argument made against stealth suppliers (the company's
market-signals record) would apply to this runtime.

- [x] Mediated fetch signs requests with HTTP message signatures under Web
  Bot Auth (`Signature`, `Signature-Input`, `Signature-Agent`;
  `docs/knowledge-base/source-declarations.md` §Web Bot Auth), the scheme
  Cloudflare's Pay Per Crawl AI-owner flow already requires. Each enrolled
  edge signs with the key minted at enrolment (WP-22); the agent card on
  the product domain lists every enrolled edge, and the key directory every
  enrolled edge holding a current directory proof signed by its own key. An
  unenrolled edge sends the user agent unsigned and the record says so; it
  never publishes a key under the `CommonMeasureBot` name. The
  `Content-Telemetry-ID` of WP-23 is a signed component, so the id a
  publisher logs is one the signature covers. Each hop of a redirect is
  signed separately, because the signature covers the authority it reaches
  (`crates/commonmeasure-harness/src/identity.rs`,
  `crates/commonmeasure-harness/src/mcp.rs` `follow`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [x] One signing implementation serves both directions: the same
  construction signs the policy fetch towards the hub, which authenticates
  the edge by it and needs no second credential (`DECISIONS.md` §Delegated
  authority and fleet management, `crates/commonmeasure-harness/src/managed.rs`).
  The signature base is pinned as a test vector on both sides, because the
  two implementations never run in one process.
- [x] `agent_id` on the wire is the enrolled edge's key id; the host tool
  name moves to a namespaced field in `data`; an unenrolled edge sends the
  emitter id `commonmeasure`. The conformance corpus is regenerated and
  re-pinned in the hub in the same change, and the hub's owner report
  carries the key id (`DECISIONS.md` §Product,
  `crates/commonmeasure-relay/src/project.rs`, `conformance/`).
- [x] A bot page and agent card published at the product domain: what
  `CommonMeasureBot` is, how it behaves, how to address it in `robots.txt`,
  how to verify its signature, how to receive reports, what a key id is
  (an edge, never an operator or a person), the directory's cache age as
  the revocation bound, and how to raise a complaint about one key id. The
  user agent carries that documentation URL and a contact. Both are on the
  hub's own board; the edge takes the absolute URLs from the
  exchange at enrolment and never builds one.
- [ ] The identity registered with Cloudflare's verified-bots programme
  once, by Common Measure, as an intermediary signed agent whose key
  directory is the product domain's, so every enrolled edge is verified
  under it (`DECISIONS.md` §Product). Needs the Cloudflare account; nothing
  in either repository is waiting on it.
- [x] A challenge or other non-2xx answer is recorded with its status and the
  identity presented, as a distinct reason from a policy refusal or a
  transport failure (`challenge` and `identity` on the crossing,
  `docs/contracts/session-evidence.md`).
- [x] The nudge text tells the agent that a site refused the runtime's
  identity, so the fallback to a built-in tool is a recorded decision
  (`crates/commonmeasure-harness/src/nudge.rs`, `mediation-nudge/3`).

**Acceptance evidence:** a loopback publisher that admits a request only
when it resolves the key id in the directory and verifies the RFC 9421
signature serves the page to an enrolled edge and answers an unenrolled one
403 with `cf-mitigated: challenge`, which is recorded as a challenge with
the identity presented; a key revoked at the hub leaves the directory, the
edge stops signing on the next relay run, and the same page challenges it
again; the correlation id is a covered component; no code path sends a user
agent other than the runtime's own
(`crates/commonmeasure-cli/tests/mediated_e2e.rs`). The same publisher uses a
listed key only when the directory response carries that key's directory
proof, a signature over `("@authority";req)` tagged
`http-message-signatures-directory` and valid now, so a key listed with no
proof, a proof for another authority or an expired one is challenged; the
proof matches the vector pinned in both repositories
(`crates/commonmeasure-harness/src/identity.rs`); `connect` signs and
uploads it, a loopback hub checks the upload by that rule and serves it, a
session or server start renews it once it is a day old, and the session
record says until when the key is listed or why it is not
(`crates/commonmeasure-cli/tests/connect_e2e.rs`). Not verified: the SRA
page itself, or any live site, which needs the Cloudflare registration
above, and the signature verified by an implementation other than this
repository's — the hub verifies the policy fetch by the same rule, and the
pinned vector is what holds the two to it; and a directory proof uploaded to
a running hub and served in its directory, which the ignored real-hub test
in `crates/commonmeasure-cli/tests/connect_e2e.rs` asserts.

### WP-30: source preferences and the named source

Read what a source declares about AI use, record it beside the crossing,
and record who named the source. The mechanisms, their standing and how the
edge reads each are `docs/knowledge-base/source-declarations.md`; the rules
are `DECISIONS.md` §Source declarations.

- [x] A preference reader that gathers statements for a crossing from: the
  response's `Content-Usage` header; `robots.txt`, fetched once per host
  through the same address-checked path as a page fetch and cached for 24
  hours, using the `CommonMeasureBot` group where present and the `*` group
  otherwise, with `Content-Usage` path rules and the `Content-Signal` line;
  and the `License:` line or `Link rel="license"` header resolving to an RSL
  licence, from which the permits and the reporting binding are read. Each
  statement is recorded with its source, category and value; absent is
  unknown, never disallow; statements combine most-restrictive-wins per the
  vocabulary draft; operator-held terms named in `policy.json` for the host
  are recorded as governing over a preference, and a terms entry may name
  the institution identifiers WP-23 carries as `access_context`
  (`crates/commonmeasure-harness/src/declarations.rs`,
  `crates/commonmeasure-harness/src/discovery.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [x] Policy on a disallowed AI-use statement: `strict` withholds the bytes
  from context and records them as withheld; `observe` and `prefer` carry
  the bytes with the breach named. A reporting demand that cannot be met is
  ruled on under WP-23. If the by-name rule (`DECISIONS.md` §Open decisions)
  is confirmed, a preference in the `CommonMeasureBot` group applies in
  every mode; the rule is not confirmed, and the record names the group
  that governed so it can be applied later
  (`crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [x] Who named the source: the `UserPromptSubmit` hook hashes every URL in
  the prompt and stores no text; each mediated crossing carries `named_by:
  user | agent | unknown` (`crates/commonmeasure-harness/src/prompt.rs`,
  `crates/commonmeasure-cli/tests/hook_e2e.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [x] Pasted content: the prompt hook scans pasted material for an RSL
  `<script>` or `<link>` in HTML, a `Content-Usage` line in a pasted HTTP
  response, and a C2PA manifest embedded in text under Annex A.8 or in HTML
  under A.7, read with the C2PA SDK, taking the training-and-data-mining
  entries; `constraint_info` is recorded as a reference for the mediated
  path, since the hook makes no network request. Plain text with nothing
  embedded is recorded as "no embedded statement"
  (`crates/commonmeasure-cli/tests/hook_e2e.rs`).
- [ ] A C2PA manifest in a pasted image, read with the C2PA SDK. The host's
  prompt hook carries text only, and the record says an image was not
  scanned. Whether the image reader is in the first demonstration is
  `DECISIONS.md` §Open decisions.
- [x] Fixtures with expected outcomes: the attachment draft's example
  `robots.txt`, and one host whose `robots.txt`, licence and `Content-Signal`
  line contradict each other
  (`crates/commonmeasure-harness/tests/fixtures/declarations/`,
  `crates/commonmeasure-harness/tests/declaration_fixtures.rs`).

**Acceptance evidence:** a real session in which each statement source
produces a recorded statement with its origin named, a `strict` withholding
that leaves the bytes out of context and says so, and a `named_by: user`
crossing from a pasted URL; every new path has a real-binary test and a
negative test.

### WP-40: canonical JSON at every sealing point

Every sealed hash and digest is computed over the RFC 8785 canonical form of
the JSON value (`docs/contracts/canonical-json.md`): sorted members, no
insignificant whitespace, the RFC's number and string serialisation. The
bytes hashed are a function of the document and of nothing else, so a
reviewer recomputes a seal with any conforming serialiser and no seal moves
when a dependency changes.

- [x] One canonicalising function in `commonmeasure-types`
  (`crates/commonmeasure-types/src/canonical.rs`), tested against the RFC's
  worked example, its member-ordering example and the number table of
  Appendix B, used at every sealing point: the run manifest hash
  (`crates/commonmeasure-runtime/src/run.rs`), replay capture hashes
  (`replay.rs`), the relay's derived session and event ids
  (`crates/commonmeasure-relay/src/project.rs`) and the conformance corpus
  (`crates/commonmeasure-relay/tests/conformance.rs`), the policy digest and
  the effective-policy identity pre-image
  (`crates/commonmeasure-harness/src/policy.rs`).
- [x] The hub implements the same rule in its own crate for the policy
  digest and the envelope, from the RFC and not from a shared crate, with
  the same test vectors; the edge and hub digests of one policy agree
  whether or not a defaulted field is written
  (`the_edge_and_hub_digest_one_policy_equally`,
  `crates/commonmeasure-harness/src/policy.rs`).
- [x] The workspace depends on the published `c2pa` crate, with no
  `[patch.crates-io]` entry and no vendored copy. That dependency's feature
  flags include serde_json's `preserve_order`, and no seal moves with it:
  every committed hash and the replay manifest
  (`demo/recon/replay-manifest.json`) re-derive under the build as it
  stands, because a digest is taken over the canonical form of the value.
  The contract that names each seal states the canonical rule.
- [x] `docs/contracts/run-output.md` and `docs/contracts/session-evidence.md`
  state the canonical rule where they name a hash;
  `docs/contracts/canonical-json.md` holds the rule and the list of seals it
  governs.

**Acceptance evidence:** a digest is a function of the value and not of the
map that carried it, so no serialiser's member order can move one —
`insertion_order_does_not_reach_the_digest` builds one value two ways and
digests both — and every committed manifest seal, the replay manifest and
the golden corpus re-derive under the build as it stands, with the published
c2pa crate enabling serde_json's insertion-order maps; the conformance test
and the hub's re-pinned corpus pass; one policy digested on the edge and on
the hub is equal.

### WP-41: the console's API and its screens agree

The console is documented as rendered "from the same JSON values the `/api/*`
routes serve" (`ARCHITECTURE.md` §Console and relay, `DECISIONS.md` §Execution
and evidence). It is not, in both directions. Three screens compute values no
route serves; one route serves a value no screen renders; and most of the
fields WP-23, WP-29 and WP-30 put on every mediated crossing appear on no
screen. This package makes the sentence true and puts this month's evidence in
front of the operator. It adds no capability and no write the console does not already have.

- [ ] `GET /api/policy/forecast` answers the value `console::forecast::forecast`
  builds, and `forecast_fragment` renders that value and computes nothing of its
  own; the fragment keeps its "nothing is saved" statement and its principal
  caveat.
- [ ] `GET /api/providers` answers the `{name, connected}` list the Sources
  screen is passed, and the screen renders that answer.
- [ ] `POST /api/compare` answers the per-provider result values the Compare
  screen renders, under the same `Origin` guard and the same injected runner, so
  the screen and the route cannot make different calls.
- [ ] The four write routes answer JSON to a request that asks for it and HTML
  otherwise, carrying the outcome kind, its status, the notice and the revision
  now in force; every negative-path test in
  `crates/commonmeasure-cli/tests/serve_e2e.rs` gains its JSON twin.
- [ ] A budget screen renders `/api/budget`: per-engagement footprint with the
  charge absence stated as the projection states it, the declared caps from the
  policy panel's own projection, and every declared principal allowance with its
  ledger standing. `crates/commonmeasure-console/src/console/budget.rs`'s module
  documentation names the screen that exists.
- [x] The Record pane renders both hashes, the HTTP status and the identity a
  mediated crossing presented, and an observed crossing's content hash alone
  with no status and no identity, asserted against the log the real binary
  wrote (`crates/commonmeasure-cli/tests/serve_e2e.rs`,
  `the_record_pane_shows_the_hashes_status_and_identity_the_record_carries`).
- [ ] The Record detail renders the rest of what a mediated crossing carries:
  the statements with their sources, the group that governed, the effective
  preference per category, operator terms where they governed, `named_by`, the
  manifest outcome and its probes from the referenced `manifest_resolved`
  record, the reporting ruling with the receiver or its absence, the
  `Content-Telemetry-ID`, and the allowance decision and its settlement. A
  field the record does not carry is absent, never rendered as unknown or zero
  (`docs/FAIL-POLICY.md` §7).
- [ ] The Overview's refusal line counts the window it names, or names the window
  it counts.

**Acceptance evidence:** for each of the three new read routes, a test in
`crates/commonmeasure-cli/tests/serve_e2e.rs` that drives the real binary,
asserts the route's JSON and asserts the rendered screen carries the same values;
a session fixture whose mediated crossing carries a disallow statement, an
operator-terms override, a resolved manifest, a reporting ruling and an allowance
reservation, with the detail pane asserted to name each; and a policy write
driven twice, once as a form and once as JSON, reaching the same file state and
the same revision.

### WP-44: the hub's operator screens

The hub's product capabilities are API-first and three of them have no screen:
content owners and their reports, onward delivery, and the unattributed hosts
no registration claims. The client documentation instructs
`curl` for each. The buyer is a platform or risk owner, not an engineer with a
terminal, and the report is a hand-over artefact they have to produce on request.
The handlers, scopes and tests exist; this package is the SvelteKit half and adds
no route.
Box four, onward delivery as a screen, waits until an operator asks for it;
the first three are on the path in §Next. Policy revisions have a screen at
the hub: the ordered history, the current revision's digest and publisher,
a builder that composes the next revision and checks it against
`docs/contracts/source-policy.md` before it is published, and each edge's
last fetch.

- [ ] Content owners: register an owner, add and remove hosts with the stated
  basis, see which hosts resolve to each, and see the refusal when a host is
  already claimed. The `basis` is shown as recorded and unverified, as the API
  states it.
- [ ] The report: generated for one owner, shown on the page and downloadable as
  the JSON the API serves, with `resolution` and the URL bound stated on the page
  and no session identifier or engagement anywhere in it.
- [ ] Unattributed hosts: the list by event count, with the registration action
  beside each, so claiming a host is one step from seeing it.
- [ ] Onward delivery: per owner, the destination and the credential it was given
  (never revealed back), queued, delivered and dead counts, the dead letters with
  their last error, and requeue.
- [ ] Every screen states the boundary it sits on: the hub holds no owner
  accounts, opens no owner-facing view, and is never in the path of a crossing.
- [ ] The hub's client pages for these three are rewritten around the screens, with
  the API kept as the second half of each page rather than the whole of it.

**Acceptance evidence:** a Vitest flow per screen against the real handler
responses; one registered owner whose report downloads from the page and matches
the API's bytes; a destination set and a delivery dead-lettered and requeued from
the page.

### WP-45: the stranger's path

A stranger who has read the product's website installs the edge, records a
crossing, reproduces the example and enrols with the hosted hub with no
step that needs a person from Common Measure, a private repository or a
credential handed out; every failure names its cause and the next action;
the documents answer the question a stranger has at each step. The hub's
halves (the hosted service, onboarding, the client pages served by the hub)
are on the hub's own board.

- [x] The example reproduced from a committed policy:
  `demo/policy/four-fetches.json` is the smallest source policy under which
  the example's four fetches come out as the page shows (two admitted, one
  refused on the licence's payment term, one refused by an operator rule);
  `docs/GETTING-STARTED.md` §4 tells the reader to copy it and quotes the
  four results as run; the policy is loaded through the runtime loader,
  ruled on the four hosts, and driven through the real binary against a
  loopback publisher whose `robots.txt` names an RSL licence with a
  subscription payment term (`crates/commonmeasure-cli/tests/demo_policy.rs`).
- [x] Every platform the release names has had the installer run on it, or
  the release says which have not. `docs/RELEASE.md`
  and `docs/GETTING-STARTED.md` §1 say the Windows binary is built and
  checksummed by the release run and the installer has not been run on
  Windows, and §4 of the walkthrough states that a policy declaring
  `principals` refuses every mediated crossing on Windows. A Windows
  transcript, when one is made, replaces the first sentence.
- [ ] The walkthrough reaches a first delivered batch against the hosted
  hub: `docs/GETTING-STARTED.md` §7 names the hub's address, shows the
  smallest scope that clears the reader's working directory for egress, and
  quotes `connect`, one mediated crossing in that scope and a relay run
  delivering one batch, each run in order from a fresh operator home.

**Acceptance evidence:** each box's quoted commands were run as written
(`AGENTS.md` §Document ownership); the policy test and
`crates/commonmeasure-cli/tests/docs_paths.rs` pass; the delivered batch is
visible in the hosted hub's fleet view, with the transcript kept under
`demo/enrolment/`.

### WP-46: the hosted edge

Claude on the web and in Chrome, ChatGPT on the web, Microsoft 365 Copilot
and the Copilot cloud agent on GitHub reach MCP servers only over HTTPS from
the vendor's cloud (`docs/knowledge-base/host-surfaces.md` §Order). This
package serves them: the same binary in a service mode, one per
organisation, enrolled and managed like any edge, answering Streamable HTTP
and authenticating each person with a token the hub issues. The policy
engine, the refusal semantics, the record format and the relay are
unchanged; what is new is transport, identity, tenancy and where the record
lives. The design, its sources and its build sequence are
`docs/knowledge-base/hosted-edge.md`. The hub's authorisation server (client
metadata documents, dynamic registration, tokens bound to the edge's
address, sign-in and membership) is on the hub's own board. Depends on
WP-22 and WP-19.

- [ ] A principal authenticated by a token: the `oauth_subject` and
  `edge_token` bases beside `os_user`, policy bindings keyed by `subject`
  or `edge_token`, exactly one key per binding; no existing policy document
  resolves differently and its policy identity does not move.
- [ ] Streamable HTTP at protocol revisions 2025-06-18 and 2025-11-25
  behind the same tool definitions and the same `McpServer`: one endpoint
  per host word, a session id minted at `initialize` and bound to its
  principal, `404` for an unknown session, `Origin` and protocol-version
  checks, JSON responses; the stdio path unchanged; the three tools declare
  `readOnlyHint`.
- [ ] A resource server: protected resource metadata per endpoint, `401`
  with `WWW-Authenticate`, tokens verified against the issuer pinned at
  enrolment for signature, issuer, audience, expiry and organisation, with
  no hub call in a crossing's ruling; edge-issued tokens for a host with no
  OAuth, stored hashed and revocable.
- [ ] A service mode: refuses to start unenrolled, unmanaged or without an
  exclusive lock on its home; runs the relay and the policy refresh on an
  interval; holds the private-address floor whatever the policy says, so
  `allow_private_hosts` is not honoured and the cloud metadata service is
  never reached; `doctor` and `status` report it.
- [ ] The contracts carry it: `docs/contracts/host-integration.md` names
  the hosted path and grades each host,
  `docs/contracts/session-evidence.md` the new bases and the hosted session
  id, `docs/contracts/policy-envelope.md` the interval refresh.
- [ ] One recorded mediated crossing through one of the four hosts, from a
  hosted edge deployed for one organisation, committed in a directory under
  `demo/host-sessions/` named for the host.
- [ ] Each remaining host registered by its administrator's documented
  steps with a recorded session, or listed with the reason it is not.

**Acceptance evidence:** a real-binary test driving the server over HTTP
against a loopback origin with the assertions of
`crates/commonmeasure-cli/tests/mediated_e2e.rs`, two concurrent sessions
leaving two files, and each token check refused by name against a loopback
issuer; a real session through a registered host read by
`crates/commonmeasure-cli/tests/recorded_sessions.rs`, carrying the host's
own client name, `authentication_basis: oauth_subject` and a mediated
crossing whose content hash the agent quoted back; an ignored live test
against the hub's authorisation server, its doc comment saying how to run
it.

### WP-47: the source policy as a published contract

Publish what the policy loader accepts and what admission decides, so
anything that writes or checks a policy outside this repository is held to
the loader's answers rather than to a copy of its types. No policy semantics
change and no field is added.

- [x] A source-policy contract: every field with its default, what a scope
  and a principal replace, the order admission applies, and each check the
  loader makes after parsing with the refusal it produces
  (`docs/contracts/source-policy.md`).
- [x] A JSON Schema of the policy file derived from the loader's own types,
  printed by `commonmeasure policy schema`, with a test that fails when the
  committed schema is not what the binary prints
  (`docs/contracts/source-policy.schema.json`,
  `crates/commonmeasure-cli/tests/source_policy_contract.rs`).
- [x] Validation vectors: accepted documents with the loader's form, and a
  refused document for every check with the loader's sentence, each carrying
  the schema's verdict beside the loader's, including documents the schema
  accepts and the loader refuses (`docs/contracts/source-policy-vectors.json`,
  driven through the real binary by
  `crates/commonmeasure-cli/tests/source_policy_contract.rs`).
- [x] Ruling vectors: policy, working directory, source and declared licence
  to the governing scope, the mode, the ruling and the runtime's sentence,
  checked against the runtime's admission
  (`docs/contracts/source-policy-vectors.json`).
- [x] `commonmeasure policy check <file>` loads a candidate through the
  loader and prints what it accepted, with the digest of its loader's form,
  or the refusal (`crates/commonmeasure-cli/src/main.rs`).
- [x] The contract states which fields mean something different on each
  machine a policy reaches — a principal binding's `os_user` and a scope's
  `match` — and the policy-envelope contract states that a distributed
  policy declares no `principals`
  (`docs/contracts/source-policy.md` §Policy written for many machines,
  `docs/contracts/policy-envelope.md` §What a distributed policy carries).

**Acceptance evidence:** the schema and both vector sets are checked against
the binary and the runtime in this repository's offline gate. The hub's
half, its publish check and its policy builder held to the same vectors, is
on the hub's own board.

## Deferred packages

These packages are not on the path in §Next. Each keeps its boxes and its
acceptance criteria, and moves back to §Open packages when it is picked up.
The sentence under each heading says why it waits.

### WP-31: the demonstration for the IETF AI-preferences working group

Waits behind the hosted hub: it is not on the stranger's path, and its
reporting scenes need the hub delivering onward from a hosted origin.

A recorded session on two real sites, replayed from its sealed source
record, with the console view and a plain-language walkthrough. Depends on
WP-23, WP-25, WP-29 and WP-30. The two sources are openattribution.org,
which declares AI training and AI use allowed and asks for usage reporting,
and forageshopping.com, which declares AI use only and asks for usage
reporting; neither is a third party (`DECISIONS.md` §Open decisions).

- [ ] Each site publishes, and the walkthrough shows side by side: a valid
  v1.0 content-owner manifest served with a cache age; an RSL licence
  carrying the Content Telemetry reporting binding at the `grounding` level;
  a `Content-Usage` header and `Link rel="license"` header on every page; a
  `robots.txt` whose `License:` line, `Content-Usage` rules, `Content-Signal`
  line and by-name `CommonMeasureBot` group all agree with the licence. One
  site also publishes a text sample under Annex A.8 and an image, each
  carrying a C2PA manifest whose training-and-data-mining assertion points at
  the licence through `constraint_info`.
- [ ] Both sites registered as content owners at the consumer each site
  designates, with the hub delivering onward, so the reporting demand is met
  end to end.
- [ ] Twelve scenes, each naming the draft text it exercises:

  | Scene | Draft text | What the record shows |
  |---|---|---|
  | Fetch a page with a `Content-Usage` header | attach §2 | statement read from the response, no extra request |
  | Fetch under a robots rule with path prefixes and a by-name group | attach §3.1, §3.4 | the group and rule that governed, cache age |
  | Conflicting statements on one host | vocab §5.1 | sources listed, most restrictive applied |
  | Operator holds a licence | vocab §5.2 | terms reference from policy, the statement it overrode |
  | Fetch allowed, use disallowed | acquisition and use separated | bytes fetched, withheld from context, recorded as withheld |
  | Preference survives transformation | derived forms | hash and preference carried from crossing to grounding event |
  | Search results versus grounding fetch | vocab §4.2 | ungrounded results as search, the fetch as AI use |
  | User pastes a URL | "directly provided by the user" | `named_by: user`, acquisition by the system, preference applied |
  | User pastes HTML with embedded RSL | attach §1.3.1 | statement read from the paste |
  | User pastes text or an image with a C2PA manifest | attach §1.3.1, CAWG | assertion read, licence and manifest followed from `constraint_info` |
  | User pastes plain text | attach §1.3.1 | no statement, unknown, mode decides |
  | Reporting demand met, output signed, output pasted back | reporting binding, C2PA A.8 | manifest resolved, report delivered at the hub, the output's own provenance read in a second session |

- [x] A mediated fetch of an HTML page delivers readable text, not markup:
  the runtime extracts the text before the admit screens run and before
  delivery, records the extraction as a transform-stage processor invocation
  carrying the hash of the bytes received and the hash of the text delivered,
  and the crossing carries both hashes; the screens rule on the delivered
  text alone, so a page whose footer or script markup carries an identifier
  is not refused for it (`crates/commonmeasure-runtime/src/processor.rs`,
  `crates/commonmeasure-cli/tests/mediated_e2e.rs`).
- [ ] The walkthrough's position on the user-versus-system question: the
  line is a recorded fact and an operator policy choice on the operator's
  side, not a definitional carve-out; a pasted URL is a reference, not the
  asset; the line is invisible to the publisher and only the operator can
  record it; the outcome to the publisher is the same whoever chose the
  asset; agent loops erase the seed; a preference is not a veto, so
  expressing one about user-directed use takes nothing from users.

**Acceptance evidence:** every scene's record re-derives from the sealed
source record; the walkthrough quotes only commands it ran; the two sites'
manifests validate against the standard's `manifest.json`, their
`robots.txt` parses under the attachment draft's rules, and their licences
validate against the reporting-binding schema.

### WP-32: telemetry projection of evaluator evidence

Projects batch-run evaluator evidence, which no stranger reaches in a first
month with the product.

Emitting evidence-backed `content_reproduced`, `content_cited` and
`content_presented` events. Content Telemetry v1.0 assigns reproduction,
citation and presentation events to the agent or its operator, because no
content-owner infrastructure can observe output construction or a
recipient-facing surface. Every event this projection emits is backed by a
sealed local run: a reproduction event resolves to a dossier line, a byte
span into retained window text, and hashes a third party can recompute. The
boundary it respects is `ARCHITECTURE.md` §Content Telemetry boundary.
The committed cited run (`demo/output/cited/`) carries the `supported`
verdicts on mid-text spans this projection draws from.

- [ ] The relay's conformance tests extended to the three event shapes
  against the pinned v1.0 schemas (`schema/SOURCE.md`).
- [ ] The mapping below, with the verdict vocabulary and everything else in
  the evaluation record staying local:

  | Evaluator fact | Projected event | Key fields |
  |---|---|---|
  | `supported` verdict | `content_reproduced` | `reproduction_type` from an exact compare (`verbatim` on a byte-for-byte match, else `near_verbatim`; the fold admits only case); `reproduced_hash` over the quote as produced; `content_hash` of the matched window part, correlating to the `content_grounded` event already emitted; `reproduced_chars` |
  | Parsed `CITATION:` line behind a `supported` verdict | `content_cited` | `citation_type: direct_quote`; `excerpt_hash` equal to `reproduced_hash`; shared `output_element_id`; `citation_id` linking the pair |
  | `contradicted`, `uncovered`, `unavailable` verdicts | nothing | negative verdicts are evaluator output and stay local |
  | Paraphrase | nothing | the evaluator cannot see paraphrase, so it never manufactures a reproduction claim |
  | Console or harness rendering a source or excerpt | `content_presented` | `presentation_kind` and `presentation_type` per surface; only for surfaces this product renders, never inferred for a downstream UI |

- [ ] Nothing emitted for outputs this product did not mediate, and no
  verification or detection role: third-party corroboration of reproduced
  content belongs to verification tooling.

**Acceptance evidence:** a conformance test derives the expected event set
for a committed cited run from `summary.json` alone and matches the relay's
projection exactly, the discipline of
`crates/commonmeasure-relay/tests/conformance.rs`. An event the run cannot
re-derive is a defect.

### WP-33: licensed access through an external settlement rail

Waits on the first rail able to issue a pseudonymous sub-credential per edge
(`DECISIONS.md` §Open decisions).

The product pays a quoted price under an allowance and holds no funds
(`PRODUCT.md` §What it is not). Where a source fronts its content with a
licence server, the edge acquires a licence token and presents it, and the
rail settles with the operator. The first rail is Supertab Connect's Open
Licensing Protocol (`docs/knowledge-base/supply-map.md` §Supertab): a
`License:` line in `robots.txt` or a `Link` header names `license.xml`, the
matching content rule names a token endpoint, the edge posts its credential
and the resource pattern and receives an ES256 JWT, and the page request
carries `Authorization: License <jwt>`. Depends on WP-22 for the
sub-credential, WP-29 for the signature and WP-30 for reading the licence.

- [ ] The rail contract: acquire a token for a URL under a sub-credential,
  report the quoted price, settle nothing locally; one adapter per rail
  behind it, Supertab Connect first.
- [ ] Token acquisition runs under the allowance ledger as a reservation at
  the quoted price, reconciled against the response; `strict` refuses before
  the token is requested when the allowance is exhausted or the price is
  unknown.
- [ ] The crossing records the token's `jti` as `license_ref`, the licence
  URL as `terms_ref` and the price; the request also carries the Web Bot
  Auth signature of WP-29, so identity and entitlement travel together.
- [ ] The sub-credential is issued at enrolment (WP-22), bound to the edge's
  key id, stored only on the edge and revoked with the key. It is
  pseudonymous to the publisher: the rail knows the operator, the publisher
  sees the network identity and the key id.
- [ ] Every claim about the rail's tab, budget and settlement endpoints is
  verified by a credentialled call before the adapter is marked live
  (`docs/knowledge-base/provider-verification.md`); today those claims rest
  on vendor documentation only.

**Acceptance evidence:** a real page behind a licence server fetched
mediated with a token acquired under an allowance, the crossing naming the
`jti`, the price and the key used; the same fetch refused in `strict` with
the allowance exhausted; the rail's statement for the period matching the
edge's reconciled spend.

### WP-34: add-on management and settings

The extension story, not the self-serve path.

Every processor compiled into the binary runs wherever it is wired; three
are gated by per-job flags; nothing in `policy.json` names a processor; the
run manifest seals the installed set and not the active one; a session
records no processor set; the console shows no processor. `PRODUCT.md` and
`README.md` say that a per-operator switch is not built. This package
builds the switch and gives each add-on a
settings surface with no add-on-specific screen: an add-on declares its
settings, and the console and the hub render them. Depends on WP-24 for the
console write path and on WP-18 and WP-19 for the organisation half.

- [ ] Policy vocabulary: an `addons` map on the policy file and on a scope,
  keyed by processor name, each entry carrying `enabled` and `settings`;
  validated through the runtime loader against the settings schema the
  manifest declares; an unknown add-on name or an invalid setting refused at
  load.
- [ ] Each processor manifest declares a settings schema (a bounded subset:
  booleans, integers with ranges, enumerations, string lists) and defaults;
  the configuration digest covers the rules and the schema; the effective
  settings are recorded on every invocation.
- [ ] `active(policy)` beside `installed()`: the run manifest seals the
  active set with each add-on's effective settings; an installed add-on
  that is disabled appears as disabled, never by absence, and the judge's
  key-absence encoding is replaced the same way; a session records its
  active set at start and on every policy change.
- [ ] The effective-policy identity of WP-18 includes the active add-on set
  and settings, so two edges running different screens never hash equal.
- [ ] Console: an Add-ons screen listing installed processors with manifest,
  permissions, active state and settings per scope and invocation counts
  (the index gains `processor_invoked`), and a settings form rendered from
  the schema as a write under WP-24's conditions, with the dry-run forecast
  for any change that widens what may happen; disabling a screen is one.
- [ ] Hub: an organisation marks each add-on `mandatory`, `optional` or
  `disallowed`, with locked settings where mandatory, rendered from the same
  schema and distributed only inside the WP-19 signed envelope; the edge
  treats a mandate as a floor, refuses a local relaxation, records the
  attempt and enforces offline; an unreachable hub changes nothing.
- [ ] The processor contract restated: installed by the binary, active by
  policy; the console statements about writes that do not exist removed
  (`crates/commonmeasure-console/src/serve.rs`,
  `crates/commonmeasure-console/src/console/policy.rs`).
- [ ] Add-on activity in the console: the index gains `processor_invoked`;
  the Record view lists each invocation beside the crossing or output it
  acted on, with processor, stage, decision, what it found or changed, and
  gaps; the Overview counts invocations by add-on; the Add-ons screen shows
  per-add-on counts and the last invocation; an add-on that was active for
  a session and never invoked is shown as active with no invocations.
- [ ] Add-on activity in the dossier: `commonmeasure inspect` and
  `commonmeasure session` print one line per invocation in order, naming
  the processor, the input it acted on and the decision.
- [ ] Add-on activity in the fleet: the fleet-status contract of WP-18
  carries the active set and per-add-on invocation counts per period as a
  bounded summary, and the hub's fleet view shows them per organisation;
  no invocation detail and no content crosses.
- [ ] Out of this package: the third-party boundary (WP-37) and the
  catalogue (WP-38).

**Acceptance evidence:** a real session with `pii-detector` disabled in one
scope and mandatory from the hub in another: the first records the add-on
as disabled at session start, the second refuses the local relaxation and
records it; the console form saves a settings change the runtime loader
validates; a batch run's manifest names the active set.

### WP-35: source quality for coding agents

Depends on WP-34 for its settings and on WP-37 for the boundary it is built
behind.

A first-party add-on for agents that write code: an `admit`-stage processor
that rules on repositories, packages and their documentation before they
enter context, as the PII detector rules on text. The sources are external
repositories the agent reads (GitHub and other forges, raw file hosts) and
package registries (npm, PyPI, crates.io and the rest). Rules are
deterministic and declared in the add-on's settings (WP-34); the verdict is
evidence, refused in `strict`, recorded and carried in `observe` and
`prefer`. The processor needs the network to look a source up, so its
manifest declares that permission; lookups go through the same
address-checked path as a fetch and are cached on disk with the source's
own cache age; a lookup that fails leaves the fact unknown, never a pass.

- [ ] Source identification: a crossing to a repository or package host
  resolves to a `content_id` (`github.com/org/repo`, `npm:name`,
  `pypi:name`, `crates:name`) recorded on the crossing and carried on the
  wire, so a consumer's prefix resolution (standard §8.6) can attribute it.
- [ ] Repository checks, each a setting with a threshold or a switch:
  minimum stars, maximum age of the last commit, archived or disabled
  state, a licence present and in an allowed set, a security policy
  present, the owner or repository on the operator's block list or allow
  list. Package checks: known vulnerabilities from OSV for the resolved
  version, deprecation and yanked flags from the registry, install scripts,
  a name within edit distance of a popular package, maintainer count and
  package age. Every check is recorded with its source URL and cache age.
- [ ] Block and allow lists per scope over host, owner and package
  patterns, first match wins, edited through the WP-24 rule surface.
- [ ] Scope stated in the record: the add-on rules on crossings the runtime
  mediates or observes; a clone or an install the host performs outside the
  runtime's sight is not ruled on, and the session says so (WP-27).
- [ ] Reporting to maintainers: retrieval and grounding events for a
  repository carry its `content_id`, and the hub delivers them to the
  endpoint the repository declares once the standard's home settles how a
  repository under a shared host declares one; the question is taken there.
- [ ] Fixtures: one repository and one package per check with a recorded
  lookup each, and a negative test per rule.

**Acceptance evidence:** a real session in which a fetch of a repository
below the star threshold is refused in `strict` with the check named, a
package with a known vulnerability is recorded with its OSV id in
`observe`, and a listed repository passes; the record names each check, its
source and its cache age.

### WP-36: support the source

Depends on WP-33 for the rail contract and on WP-26 for the measured terms.

A second first-party add-on: a voluntary payment to a source that helped,
routed through an external rail under the allowance, with Common Measure
holding no funds (`PRODUCT.md` §What it is not). Measured helpfulness
decides who may be paid; the operator decides how much. Whether a voluntary
payment falls under "pay a quoted price under an allowance" is
`DECISIONS.md` §Open decisions. Depends on WP-33 for the rail contract and
on WP-26 for the measured terms.

- [ ] An `output`-stage processor that, at the end of a run or session,
  lists the grounded sources with their measured contribution and the payee
  each declares: a funding declaration in the repository or the registry
  record, a Content Telemetry manifest carrying a payment reference, or an
  operator-declared payee in the terms entry for the host. A source with no
  declared payee is listed as unpayable and never paid to a guessed address.
- [ ] Settings: a budget per scope and period, a floor for measured
  contribution, a payee allow list, and whether a payment is automatic or
  proposed for the operator's approval in the console.
- [ ] Payment through a rail adapter behind the WP-33 contract, reserved and
  reconciled in the allowance ledger, recorded on the run with the payee,
  the rail's reference and the measured term that justified it; a payee on
  a rail with no adapter is recorded as unpayable.
- [ ] Nothing about a payment crosses the Content Telemetry wire: the
  standard has no such event and the projection adds no field.

**Acceptance evidence:** a committed run whose dossier lists the grounded
sources, their measured contribution and the payee found or the unpayable
state, with one payment reconciled against the rail's own record; the same
run with the budget exhausted proposes and pays nothing.

### WP-37: the sidecar processor boundary

Depends on WP-34, and no processor outside the first-party set has a consumer
yet.

The extension point for every processor that is not compiled into the
binary: an organisation's own, a third party's from the catalogue, and
first-party add-ons where the boundary suits them. The contract already
describes a supervised local sidecar; nothing implements it, and today
nobody can ship a processor without editing the runtime crate. The measure
of this package is how little an organisation's engineer needs to write
one: a program in the language they already use that reads one invocation
and writes one record. Depends on WP-34 for the active set and settings.

- [ ] A sidecar protocol: one scoped invocation per call over stdio, JSON
  in and JSON out, the invocation record's schema published in `schema/`
  and validated in the conformance test; a sidecar receives the inputs its
  manifest declares and nothing else.
- [ ] Owned manifests: the manifest type takes owned strings, carries an
  artefact digest and a signature, and is read from the add-on's artefact
  rather than compiled in.
- [ ] A supervisor that starts the sidecar, enforces the timeout the
  manifest declares, grants only the permissions it declares (no network,
  filesystem or credential access unless declared), and records a failure
  by the manifest's fail behaviour per mode.
- [ ] `commonmeasure addon install <artefact>` and `remove`, verifying the
  signature against the operator's trusted signers; the organisation's own
  signing key is a trusted signer by default, so an in-house add-on needs
  no one else's approval; the installed set is what `active(policy)` may
  enable.
- [ ] Writing one: `commonmeasure addon new <name> --lang python|typescript|rust`
  scaffolds a processor from the invocation schema, with the manifest, a
  settings schema, one test and a README; the three scaffolds are kept in
  the repository and built in CI, so the schema cannot drift from them.
- [ ] Trying one: `commonmeasure addon test <path> --session <id>` replays a
  recorded session or run through the processor locally and prints the
  invocation records it would have written, with no policy change and no
  evidence written; `commonmeasure addon sign` signs the artefact with the
  organisation's key.
- [ ] A guide, "write your own processor", that walks from the scaffold to
  an add-on running in a real session, and is run as written before it is
  committed (`AGENTS.md` §Document ownership).
- [ ] One first-party add-on built as a sidecar, source quality for coding
  agents (WP-35), so the boundary is proven by a real processor with a
  network permission.
- [ ] Evidence identical to in-process: the same invocation record, the same
  place in the run and session evidence, and the same visibility in the
  console and dossier (WP-34).

**Acceptance evidence:** a real run in which a sidecar add-on rules on a
crossing and its invocation record validates against the published schema;
the same add-on with a declared permission removed is refused the access
and the refusal is recorded; an unsigned artefact is refused at install.

### WP-38: the add-on catalogue and marketplace

Depends on WP-34, WP-37 and the hub's billing.

The hub's catalogue of add-ons, first-party and third-party, free and
paid, from which an organisation approves what its edges may run. Depends
on WP-34 for mandates and settings, WP-37 for third-party artefacts, and
the hub's billing.

- [ ] A catalogue entry per add-on: manifest, settings schema, permissions,
  artefact digest and signature, developer, price (free or a plan), and the
  organisations that approved it; first-party add-ons are listed the same
  way.
- [ ] A private catalogue per organisation: an in-house add-on is uploaded
  by an owner, signed with the organisation's key, listed to that
  organisation only, and distributed to its edges the same way as an
  approved public one; publishing it to the public catalogue is a separate,
  explicit act.
- [ ] Organisation approval: an owner approves an add-on from the catalogue
  and marks it mandatory or optional with locked settings (WP-34); the
  approved set and the artefacts are distributed to edges inside the signed
  policy envelope, and an edge installs only what its organisation approved.
- [ ] Developer accounts: a developer submits an add-on with its signed
  artefact, the hub verifies the manifest, schema and signature, and a
  review gate precedes listing; the developer sees per-organisation
  invocation counts for its add-on and nothing else.
- [ ] Billing: a paid add-on is billed to the organisation through the hub's
  billing, with the developer's share paid out through the hub's payment
  provider. The product's boundary on content settlement (`PRODUCT.md`
  §What it is not) is unchanged: this is product billing, not content
  payment.
- [ ] The edge shows the catalogue entry an installed add-on came from and
  reports an artefact whose digest no longer matches the catalogue.

**Acceptance evidence:** a third-party add-on submitted, listed, approved
by one organisation as mandatory, distributed to an enrolled edge, run in
a real session with its invocation visible in the console, and billed on
the organisation's statement; an edge in a second organisation that did
not approve it cannot enable it.

### WP-39: the other hosts

Adds reach to further hosts; Claude Code, Codex and Pi are enough for the
first stranger.

Claude Code, Codex, Pi, Claude Desktop, Cursor, the Copilot CLI and VS Code
are the hosts integrated today. Each host below
is investigated first and integrated second, under the host-integration
contract: a host supplies session identity and, where it can, lifecycle
hooks; with hooks it gets observed crossings, without them it gets the
mediated tools only, and the record's grade says which. Every finding is
marked `planned`, `spec-verified` or `live-verified` and never collapsed.
Depends on WP-21 for the `install <host>` shape.

- [x] Investigation, one section per host in a host-surfaces document
  under `docs/knowledge-base/`: how the host is configured with
  an MCP server (file, scope, transport), whether it has lifecycle hooks
  and which events carry a tool result, what session identity it exposes,
  whether it has an extension API that can observe tool calls where hooks
  do not exist, and how it identifies itself so `host` in the record is
  exact. Each section names the document or the live probe it rests on
  (`docs/knowledge-base/host-surfaces.md`).
- [x] Investigation of the hosts the first page does not name, one
  section per host in `docs/knowledge-base/host-surfaces-2.md`: Gemini
  CLI, Zed, Cline, OpenCode, Devin, JetBrains AI
  Assistant and Junie, Kiro, Goose, Amp and Google Antigravity, with the
  other agent hosts Herdr recognises in a table, and an order for
  integration.
- [x] The MCP server answers `initialize` with the client's requested
  protocol version when it can serve it (`2025-03-26`, `2025-06-18`,
  `2025-11-25`), so a client that refuses another version (Junie) connects;
  pinned by a test in `crates/commonmeasure-cli/tests/mediated_e2e.rs`
  (`initialize_is_answered_with_the_requested_protocol_version_when_the_server_serves_it`,
  and `a_batch_is_answered_under_2025_03_26_and_refused_under_later_revisions`
  for the batching `2025-03-26` requires).
- [x] The Claude Code hook reader refuses a payload under
  `GEMINI_SESSION_ID`, `DEVIN_PROJECT_DIR` or `GROK_SESSION_ID`, recording
  nothing and printing nothing, as under `CURSOR_PROJECT_DIR`
  (`crates/commonmeasure-cli/tests/hook_e2e.rs`
  `the_claude_code_reader_refuses_under_gemini_devin_and_grok_environments`).
- [ ] Gemini CLI: `install gemini-cli` writes the server into
  `~/.gemini/settings.json` and hooks read Gemini's `AfterTool` shape.
- [ ] Hosts that read Claude Code's registration (the Devin CLI, Grok
  Build, Oh My Pi): the mediated record names the host rather than
  `claude-code`, through a `--host` value an entry in the host's own file
  carries.
- [x] Goose: the MCP server takes `AGENT_SESSION_ID` as the session id
  when no `--session` is given (`crates/commonmeasure-cli/tests/mediated_e2e.rs`
  `the_session_id_comes_from_the_argument_then_agent_session_id_then_the_server`).
- [ ] Goose: `install goose` writes the extension into
  `~/.config/goose/config.yaml`.
- [x] GitHub Copilot: the agent mode inside VS Code and the Copilot CLI,
  as two surfaces; the coding-agent on GitHub as a third, which runs where
  no edge is and is recorded as out of reach unless a hosted edge serves it.
  The Copilot CLI: `install copilot` writes the server into
  `~/.copilot/mcp-config.json` and four camelCase hooks into a hook file of
  its own, and the hook command reads the CLI's payload under `sessionId`;
  registration `fixture-tested` in `install_e2e.rs`, observed
  `spec-verified` in `hook_e2e.rs` on the documented shapes, mediated
  `planned`: the CLI listed the entry and refused to start it for want of a
  Copilot plan. The Copilot app and agent mode in VS Code: `spec-verified`,
  the same files by their documentation. The cloud agent runs in a
  sandbox with no operator home, out of reach of the local binary
  (`docs/contracts/host-integration.md` §6).
- [x] Cursor: its MCP configuration and its hooks, and whether a hook
  event carries the fetched content or only the call. `postToolUse`
  carries every tool's output; `install cursor` writes the server and four
  hooks, the hook command reads Cursor's shapes under its
  `conversation_id`. Registration `fixture-tested` in `install_e2e.rs`;
  observed `spec-verified` in `hook_e2e.rs` on the documented shapes, no
  payload recorded from Cursor; mediated `planned`, no Cursor session is
  recorded (`docs/contracts/host-integration.md` §6).
- [x] VS Code without Copilot: the workspace and user MCP configuration
  that any MCP-capable extension reads, so one registration serves several
  extensions; which extensions read it is recorded, not assumed. VS Code's
  own harnesses and, forwarded, the Copilot harness read it; Cline,
  Continue and Roo Code keep their own files
  (`docs/knowledge-base/host-surfaces.md`). `install vscode` writes one
  `servers` entry in the user `mcp.json` and no hooks; registration
  `fixture-tested` in `install_e2e.rs`, mediated `planned`, no chat
  session is recorded (`docs/contracts/host-integration.md` §6).
- [ ] VS Code hooks: whether to register any, decided once a session with
  Copilot Chat shows what its `PostToolUse` input carries. Its hooks run
  only through that extension, which also loads Claude Code's hook files
  and ignores matchers, so none is registered until a payload shows a tool
  result to record.
- [x] Codex beyond the CLI: the desktop app and the IDE extension, and
  whether the CLI registration reaches them. One table serves all three;
  `install codex` writes the approval mode Codex needs and the record
  carries the client's name and version. The CLI: `fixture-tested`, the
  session one `codex exec` run recorded through that table is
  `demo/host-sessions/codex/`. The ChatGPT desktop app and the IDE
  extension: `spec-verified`, the documentation states both read the same
  table and no session through either is recorded here
  (`docs/contracts/host-integration.md` §6).
- [x] Claude Desktop: its MCP configuration; no hook surface is expected,
  and the record says mediated-only if so. None exists; `install
  claude-desktop` writes the one entry, registration `fixture-tested` in
  `install_e2e.rs`; a launch starts two servers, one per client, and each
  that makes a call leaves its own session naming its client; mediated
  `planned`, no session through the application is recorded
  (`docs/contracts/host-integration.md` §6).
- [ ] Claude in the browser: the web app's remote connectors and the
  browser extension. Both reach tools only over HTTP, so they need the
  mediated tools served as a remote MCP endpoint by an edge running as a
  service under the organisation's policy, never by the hub; that edge is
  WP-46.
- [ ] Browser answer surfaces: ChatGPT on the web, Google AI Overviews and
  Bing Copilot Search, whose own web search crosses no tool this product
  can offer. The extension in `browser/` observes each answer's sources and
  sends them to the binary over Chrome native messaging; `install chrome`
  writes the host manifest, `uninstall` removes it and `doctor` reads it
  back. Registration `fixture-tested` in `install_e2e.rs`; the message path
  `fixture-tested` in `browser_e2e.rs`, and the ChatGPT stream parser in
  `browser/test/`; the Google page reader finds no source on the live page,
  whose overview links name no destination; no session through the
  extension is committed (`docs/contracts/host-integration.md` §6). Ticked
  when a real session per surface is recorded through the real binary.
- [ ] Implementation per host, in the order the investigation ranks by
  reach: `commonmeasure install <host>` and `uninstall <host>` writing only
  that host's surface, `doctor` reporting it, hooks registered where they
  exist, the nudge delivered where the host accepts one, and one recorded
  real session per host as the acceptance evidence. A host with no usable
  surface is listed as such with the reason, not integrated by a workaround.
- [ ] `docs/contracts/host-integration.md` §6 carries every host with its
  verification state, and the glossary's host entry lists them.

**Acceptance evidence:** the host-surfaces document with a dated probe per
host; for each integrated host, a real session recorded through the real
binary with a crossing at the grade the host allows, and `doctor` matching
the registration; for each host not integrated, the reason in one sentence.

### WP-42: the management MCP surface

Second-month value for an agent reading its own record, and depends on WP-34
for the add-on tool.

An agent can be governed by this product and cannot read what it recorded, ask
why it was refused in any form but a sentence, or report what an add-on found.
This package gives an agent a read-only view of its own record, scoped to its own
session, through the registration that already exists. It adds no write tool, and
the reason is in the package: the product rules on this agent's inputs, so a page
the agent reads must never be able to reach the rules that would refuse the next
one. Depends on WP-34 for the add-on tool.

- [ ] A refusal is a value. Every mediated refusal carries, beside the sentence
  it already carries, the stage that decided (`host`, `access_rule`,
  `declaration`, `pii`, `injection`, `allowance`, `address`, `reporting`), the
  rule or statement that decided it, the policy scope, the policy identity and
  the evidence sequence the refusal was written at. The sentence is unchanged, so
  no host's rendering moves.
- [ ] A read-only record reader injected into `McpServer` from
  `crates/commonmeasure-cli/src/main.rs` `serve_mcp`, as the `SearchRunner` is
  injected into `serve`; `commonmeasure-harness` gains no dependency and a
  harness with no console still serves the content tools.
- [ ] The tools, all reads: this session's records; the effective policy and its
  identity; one crossing's refusal in full; this principal's allowances and
  remaining state; the fleet-status document; the forecast for a draft rule.
  Each answers the same value the corresponding `/api/*` route serves, so the
  two clients cannot disagree.
- [ ] Scope: a management read answers for the current session and the current
  policy scope. A wider read is a policy field the operator sets, off by
  default, and the tool names the scope it answered for.
- [ ] Refused bytes are unreachable. No management tool returns page text, run
  directory content or sealed provider bytes; a test asserts a refused crossing's
  content is not recoverable through any tool.
- [ ] The written rule, in `docs/contracts/host-integration.md`: the management
  surface has no write tool, and the list of what an agent must never be able to
  change is the policy, the allowance, telemetry clearance, the attribution
  rules, enrolment, the deployment mode, the relay and add-on enablement.
- [ ] One registration: the management tools arrive through the same MCP server
  entry `install <host>` writes, enabled by a policy field, so no host gains a
  second server and no crossing is recorded twice.
- [ ] An add-on tool naming, per session, which processors were active, which ran,
  on what, what each found or changed and what it could not see (WP-34).

**Acceptance evidence:** a real session in which the agent is refused a fetch by
an access rule, calls the refusal tool, and receives the rule, the scope and the
policy identity that the session log records for the same crossing; the same
session's record tool answering only its own session with a wider read refused by
name; and a test asserting the tool list contains no write.

### WP-43: fleet status delivered and shown

Waits behind the hosted hub and the managed-policy run of WP-19, which it
extends.

`commonmeasure status --json` builds a complete fleet-status document and
nothing carries it anywhere. The contract says delivery is a management request
on a path of its own (`docs/contracts/fleet-status.md` §Where it travels); that
path is unbuilt on both sides. Without it the hub cannot answer the question it
exists to answer, and cannot infer it either: the desired-policy fetch records
nothing about the edge that asked. Depends on WP-18 for the document and WP-29
for the signature. The edge sends; the hub classifies and shows; no policy
document and no engagement name crosses, and no crossing waits on any of it.

- [ ] The edge posts its fleet-status document to the management path on the hub
  it is enrolled with, signed with its enrolled key under RFC 9421, on its own
  schedule and never in a crossing's path; an unenrolled or revoked edge sends
  nothing and records why; a `local` edge sends the document with
  `deployment_mode: local` or does not send at all, stated once and tested.
- [ ] The path is separate from the telemetry receiver path, so an ingest key
  can never carry policy state and policy state can never carry a crossing.
- [ ] The hub stores the last document per key id under the organisation, with
  its receipt time, and holds no history beyond a stated retention.
- [ ] The hub computes the four answers — `current`, `stale`, `divergent`,
  `unknown` — from the document and its own desired revision alone, by the rule
  `docs/contracts/fleet-status.md` §What a receiver concludes states, with the
  edge's implementation as the reference.
- [ ] A fleet screen: one row per enrolled edge with key id, name, deployment
  mode, desired and applied revision, applied digest, the answer, the last
  enforcement time and the bounded allowance summary; an edge that has never
  reported is shown as never reported, not as unknown.
- [ ] The document's absence is a state of its own: an edge silent past a stated
  interval is `stale contact`, distinct from `stale policy`.
- [ ] Documentation calls every digest drift evidence and never attestation, in
  both repositories.

**Acceptance evidence:** two edge homes on one organisation, one applying the
current revision and one an earlier one, reported to a running hub, with the
fleet screen showing `current` and `stale` and the hub reaching both answers from
the documents alone; an edge whose policy was edited after activation shown as
`divergent`; a revoked key's report refused and recorded; the hub's answer
recomputed by hand from the stored document.

---

## Definition of done

- No mock, fake backend or test-only implementation stands in for product
  code (`AGENTS.md`); recorded bytes may be served across a real transport
  boundary; a real-binary or real-service test covers every main path and a
  negative test protects each policy.
- Claims use the vocabulary of `docs/contracts/provider.md`; no metric
  represents unknown as zero (`docs/FAIL-POLICY.md` §7).
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --offline -- -D warnings` and `cargo test --workspace --offline` pass.
- A human can follow one run from named inputs to evidence without reading
  source; README, architecture, contracts, CLI help and console agree; no
  stale document, dead code or generated artefact remains; no credentials or
  licensed bodies enter git.
- A package that changes the run or manifest contract re-validates the live
  path before it closes; a `live-verified` claim goes stale when the contract
  moves.
- The QA gate (`docs/qa/gate.md`) runs after each package; open findings go
  to `docs/qa/OPEN.md`.

## Handoff template

```text
Agent/persona:
Scope completed:
Files changed:
Commands/tests run:
Integration evidence:
Known gaps or claims not verified:
QA findings introduced/resolved:
Recommended next package:
```
