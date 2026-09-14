---
title: "Getting started: from the installer to your own evidence"
---

# Getting started: from the installer to your own evidence

This is the Common Measure walkthrough from the installer to an operator
console showing the first crossing your own agent made. Nothing before the
last section needs a checkout or a toolchain. Every command here was run
while the document was written; where output is quoted, it is what the
command printed. Terms like crossing, run, admission and engagement
are defined in [`docs/GLOSSARY.md`](GLOSSARY.md). Two things to know before you start:

- **Nothing needs configuring first.** With no credentials and no policy
  file, the hooks record everything a session retrieves, and from
  a checkout the batch runner publishes a run that names what it could not
  do and `--replay` drives the whole acquisition path from committed
  recorded responses (§8). Credentials and policy add capability; their
  absence is always stated.
- **The model is never Common Measure's.** The harness path uses the host's
  model; the batch path needs `COMMONMEASURE_INFERENCE_ENDPOINT` before any plan
  can complete. Without a gateway, no plan produces an answer.

## 1. Install the binary and register it with Claude Code

The installer ends with `commonmeasure` on your `PATH`; the route from a
checkout (§8) ends in the same place, and both then register with Claude
Code the same way. The releases and the installer are described in
[`docs/RELEASE.md`](RELEASE.md).

### From a release, with no toolchain

The release holds binaries for Linux (x64 and arm64), macOS (Apple silicon
and Intel) and Windows x64. The installer downloads the binary for your
platform, verifies its checksum against the release's `SHA256SUMS`, places
it in `~/.local/bin` and prints the line to add when that directory is not
on `PATH`. No account and no token is needed:

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
```

On a platform the release has no binary for it stops and lists the
binaries the release holds. Run on macOS (Apple silicon) from an empty
home directory, the command printed:

```text
installed ~/.local/bin/commonmeasure: commonmeasure 0.3.1, checksum verified
~/.local/bin is not on PATH. This installer does not edit shell profiles; add this line to yours:
  export PATH="~/.local/bin:$PATH"
Next: commonmeasure install claude (or codex, pi) to register with your host, then work a session, then run 'commonmeasure session' to see what it recorded and 'commonmeasure serve' for the console on loopback. Nothing leaves this machine.
```

The home directory is written as `~` above; the installer prints it in full.

The release workflow's `verify` job runs the same installer against each
published release in a clean Linux container on GitHub, with no toolchain
and no credential ([`docs/RELEASE.md`](RELEASE.md) §The clean-container
check). The installer's refusals (a checksum that does not
match, a binary reporting another version, a platform or a release that
does not exist, a missing `curl`) are each driven against a loopback origin
standing where the release stands
(`crates/commonmeasure-cli/tests/installer.rs`). The Windows binary is
built and checksummed by the release run; the installer has not been run on
Windows.

### Register with the host

On either path, the binary writes its own registration:

```sh
commonmeasure install claude
```

This writes the four hooks (`SessionStart`, `PostToolUse`,
`UserPromptSubmit`, `Stop`) into `~/.claude/settings.json` and the MCP
server at user scope into `~/.claude.json`, each naming the binary by its
resolved absolute path, because a hook runs outside your login shell's
`PATH`. Only this product's entries are written; everything else in those
files is kept. Replacing the binary at that path changes what the next
session runs, with no reinstall. `commonmeasure uninstall claude` removes
exactly those entries and leaves `~/.commonmeasure/` alone.

The Claude Code plugin in `plugin/` is the other registration route, for
marketplace and archive installs; it declares the same hooks and carries no
binary (`plugin/README.md`). Use one route or the other: `install claude`
refuses while the plugin is enabled, because two registrations record every
crossing twice. `commonmeasure install codex` and `commonmeasure install
pi` register the mediated tools with those hosts the same way
(`plugin/README.md` §Codex and Pi); the Codex table also carries the
approval mode Codex needs to call the tools without asking, and it serves
the Codex CLI, the ChatGPT desktop app and the Codex IDE extension alike.
`commonmeasure install claude-desktop` and `commonmeasure install cursor`
register with those two applications (`plugin/README.md` §Claude Desktop
and Cursor), and `commonmeasure install copilot` and `commonmeasure install
vscode` with the Copilot CLI and VS Code (`plugin/README.md` §GitHub
Copilot and VS Code). `commonmeasure install chrome` registers the binary
for the browser extension in `browser/`, which records the sources ChatGPT
on the web, Google AI Overviews and Bing Copilot Search show
(`plugin/README.md` §Chrome).

Version 0.3.2 registers all the hosts above. Earlier binaries may refuse
a host added in this release; rerun the installer to update.

### Check what arrived

```sh
commonmeasure doctor claude
```

`doctor` prints, for the host: which of the four hooks are registered and in
which file, whether the MCP server is registered and its command, the binary
each names and the version that binary reports when run, whether a plugin is
installed beside the registration, whether the sessions directory can be
written, and whether the policy file loads. A registration whose binary has
gone is reported as not found, which is the one state in which every hook
exits without recording and nothing in the session says so.

## 2. Record a session of your own work

Open a new Claude Code session and work. No other step is needed: the
observed half needs no credentials and no configuration. A `PostToolUse` hook
fires after every `WebFetch`, `WebSearch` and third-party MCP result, and
records the crossing; for `WebFetch`, with the SHA-256 of exactly the text
that entered the model's context, which makes the row checkable against the
transcript later. Search results are recorded as retrieved but not grounded:
the model saw titles and snippets, not pages, and the record does not claim
more.

Read it back at any time:

```sh
commonmeasure session            # the most recent session
commonmeasure session <id>       # a named one
commonmeasure status             # the policy in force, by digest, and this edge's fleet status
```

A session that fetched one page reads:

```text
records    3
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

Each completed turn also records a context snapshot: the host transcript's
own usage counters for the turn's last model call, printed with that basis
named, and an inventory of the window by category estimated from the host's
own transcript records, printed with its basis named. What no basis can see
(a context limit, the free capacity, the resident tool schemas) is said to
be unavailable rather than estimated; the witnessed acquisition is shown
beside the counters as a lower bound; and between two boundaries the report
shows the API-reported change beside the estimated growth of scaffolding,
conversation and acquired content and the remainder it cannot attribute
([`docs/contracts/session-evidence.md`](contracts/session-evidence.md) §Context snapshots).

Observed capture can only watch: the hook fires after the crossing, so
nothing can be refused. The refusable grade of evidence exists only when the
agent asks Common Measure to carry the crossing, and agents use their built-in
tools unless told otherwise. The plugin's `SessionStart` hook therefore adds
one paragraph to every session's context asking the agent to prefer
`context_fetch` and `context_search` over `WebFetch` and `WebSearch`, and to
respect a refusal rather than retrying it with a built-in tool
(`plugin/README.md` §The standing nudge). Each delivery is recorded in the
session log as `nudge_issued` under the wording's versioned identity. It is
a request, not enforcement: nothing blocks the built-in tools, so expect
mediated crossings alongside observed ones.

A mediated fetch checks the operator's source policy first, records the
crossing either way, and hands the agent the bytes plus the hash it just
recorded. Of the eight installed in-process processors, the PII detector and
the injection screen run at this crossing, judging the text before the model
sees it, and each invocation is recorded as a `processor_invoked` event
beside the crossing it judged ([`docs/contracts/processor.md`](contracts/processor.md)); the context
optimiser runs at the batch runner's transform stage, the support governor
only in batch runs whose job declares `governance`, the fidelity verifier on
every batch answer, the fidelity judge only in batch runs whose job declares
`fidelity_judge`, and the output provenance labeller only in batch runs
whose job declares `output_provenance`. One limitation:
the host does not tell the MCP server its session id, so mediated crossings
are recorded under a session of the server's own (`local-<timestamp>-<pid>`),
one per server process, alongside the host session's observed record. Both
appear in the console. The plugin's own tools are excluded from observed
capture, so a mediated fetch is never double-recorded.

## 3. Credentials, when you want provider search

`context_search` and the batch runner's `--live` need provider credentials,
which live in `~/.commonmeasure/credentials.env` (`$COMMONMEASURE_HOME/credentials.env`)
beside `policy.json`:

```sh
cat > ~/.commonmeasure/credentials.env <<'EOF'
EXA_API_KEY=…
TAVILY_API_KEY=…
EOF
chmod 600 ~/.commonmeasure/credentials.env
commonmeasure credentials     # which providers are configured, and from where
```

The MCP server loads the file at start, so the mediated tools work in a
session launched from anywhere. The rules (the launching environment wins;
nothing is read from the working directory; the file is parsed, never
sourced; a loaded file is recorded as `credentials_loaded` with path, digest
and variable names) are in `DECISIONS.md` §Integration and ownership. The
repository-root `.env` is a development convenience for the justfile's live
recipes and for exporting into your shell by hand. A fresh checkout has
none: `.env.example` lists every variable the adapters read, blank, and is
the file to copy and fill in:

```sh
cp .env.example .env         # then fill in the values you hold
set -a; . ./.env; set +a
```

An unconfigured provider reports

```text
unavailable: exa needs EXA_API_KEY, which is not configured. Set it in
/home/op/.commonmeasure/credentials.env (KEY=VALUE lines, chmod 600), or export
it in the environment that launches the harness — the environment wins where
both name it. No search was attempted.
```

rather than a search that returned nothing without saying why. A configured
search records each result against policy and reports the provider's
observed charge in its own unit (an Exa search reports, for example,
USD 0.007).

One provider needs no credential: `context_search` with
`"provider": "internal"` queries the operator's own corpus, the directory
`COMMONMEASURE_INTERNAL_CORPUS` names, set in the launching environment or in
the same credentials file. Results carry the licence and effective dates
declared in the corpus's `corpus.json`, and the crossings are recorded only
when the corpus's `file://` prefix is named in `record_internal_prefixes`
(§4); without it the agent still gets the result and the record is withheld.

The replay mode needs none of this (§8). A skill needs no credential either,
and is not reachable from a session tool: it is invoked only by a run, only
with `--live`, and only when `COMMONMEASURE_SKILL_CATALOGUE` names a catalogue
declaring it (`demo/skills/README.md` is a worked example;
[`docs/contracts/provider.md`](contracts/provider.md) §Skill supply is the contract).

## 4. Policy: refusing a crossing before it happens

Absent policy means observe: record everything, refuse nothing. To let the
operator refuse, put a source policy at `~/.commonmeasure/policy.json` (or
`$COMMONMEASURE_HOME/policy.json`). The repository commits the smallest one
that reproduces the four fetches of the example,
[Four fetches, two refused](guide/four-fetches.md): copy it into place.
The example's records, like the results below, were left by an edge not
enrolled with a hub, so every request went out unsigned;
[`docs/HUB.md`](HUB.md) says what enrolment adds.

```sh
cp demo/policy/four-fetches.json ~/.commonmeasure/policy.json
cat ~/.commonmeasure/policy.json
```

```json
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*.people.com", "action": "allow"},
    {"kind": "access_rule", "host": "*.theguardian.com", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ]
}
```

Four ordered access rules in strict mode: three named hosts are allowed and
the last rule refuses every other host. Under it the four fetches of the
example, made through `context_fetch` from a new session, came out as
follows; each result is quoted as it ran, with the page text elided and
`/home/op` standing for your home directory.

`https://www.gov.uk/government/organisations`, admitted:

```json
{
 "url": "https://www.gov.uk/government/organisations",
 "content_hash": "sha256:1a18717c55ede85cf8fb906bbece35b391789e72adb01c2a161284beec407b1a",
 "retrieved_hash": "sha256:12f13d42ea021a4b70364b684b382e178957ab4e811da2f6595632fc224e8894",
 "estimated_tokens": 10140,
 "token_basis": "characters/4",
 "http_status": 200,
 "licence": {"state": "unknown"},
 "declarations": {"effective": {"train-ai": "unknown", "ai-input": "unknown", "ai-index": "unknown", "search": "unknown"},
                  "statements": [], "governing": "statements", "terms": null, "robots_group": "*", "licences": []},
 "policy": "Admitted; no constraint excluded it.",
 "breach": null,
 "named_by": "unknown",
 "recorded_in": "/home/op/.commonmeasure/sessions/door-walkthrough.ndjson",
 "content": "Cookies on GOV.UK\nWe use some essential cookies to make this website work.\n…"
}
```

`https://www.people.com/`, admitted:

```json
{
 "url": "https://people.com/",
 "content_hash": "sha256:6bd98241473b3aa18354d512f538024e282775ee12423a2e34f566f4b68202f9",
 "retrieved_hash": "sha256:999d64faba57b0114709d11fa16efd4c70f630e903929ed312378ecd776d32bc",
 "estimated_tokens": 3410,
 "token_basis": "characters/4",
 "http_status": 200,
 "licence": {"state": "unknown"},
 "declarations": {"effective": {"train-ai": "unknown", "ai-input": "unknown", "ai-index": "unknown", "search": "unknown"},
                  "statements": [], "governing": "statements", "terms": null, "robots_group": "*", "licences": []},
 "policy": "Admitted; no constraint excluded it.",
 "breach": null,
 "named_by": "unknown",
 "recorded_in": "/home/op/.commonmeasure/sessions/door-walkthrough.ndjson",
 "content": "Skip to content\nPEOPLE\nSearch\n…"
}
```

The Guardian article the example names, refused before any request for
it, as a tool error:

```text
refused before the crossing: The licence https://theguardian.com/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet. (operator policy in /home/op/.commonmeasure/policy.json; the source's declarations are recorded in /home/op/.commonmeasure/sessions/door-walkthrough.ndjson)
```

`https://www.economist.com/`, refused by the operator's own rule, as a tool
error:

```text
refused before the crossing: access rule 4 (*) refuses host www.economist.com. (operator policy in /home/op/.commonmeasure/policy.json)
```

Two of these differ from the example, whose records come from an earlier
session: gov.uk served different bytes (a different `retrieved_hash`) that
extracted to the same text (the same `content_hash`), and people.com's page
had changed, so both its hashes and its token estimate differ; the Guardian
refusal is word for word the same, and the Economist refusal names rule 4
because this policy has four rules where the example's scope has 32.

Read the session back:

```sh
commonmeasure session door-walkthrough
```

```text
session    /home/op/.commonmeasure/sessions/door-walkthrough.ndjson
records    12
crossings  0 observed, 2 mediated, 2 refused, 0 reconstructed
grounded   2 put page text into the model's context

policy identity (mediated crossings and boundaries)
  sha256:b39db117d02f79b1974456194076ea75c1a3b954d30bf37dbd47974fdaff97ab  4 record(s)

crossings
  mediated   grounded  https://www.gov.uk/government/organisations
      identity: unsigned — this edge is not enrolled with a hub, so it holds no key a publisher can verify: no enrolment record at /home/op/.commonmeasure/enrolment.json
  mediated   grounded  https://people.com/
      identity: unsigned — this edge is not enrolled with a hub, so it holds no key a publisher can verify: no enrolment record at /home/op/.commonmeasure/enrolment.json
  mediated   refused   https://www.theguardian.com/politics/2026/sep/09/mr-congeniality-burnham-advises-badenoch-to-ditch-the-point-scoring-over-defence-spending
      refused: The licence https://theguardian.com/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet.
  mediated   refused   https://www.economist.com/
      refused: access rule 4 (*) refuses host www.economist.com.
```

A refusal appears in three places: the tool error the agent sees, quoted
above; the session log, as a `crossing_refused` record with the reason and
no content hash, because nothing was fetched; and the console, where the
Record's rail row for the session counts it under "refused" and the
session's detail pane shows a refused card carrying that reason. An admitted
fetch carries `"policy": "Admitted; no constraint excluded it."` on the
result. Only mediated crossings can be refused; observed capture still
records everything the agent's built-in tools did. Delete the file to
return to observe mode. A policy file that cannot be parsed is an error,
not a silent fallback to permissive. The two publishers' pages and
declarations are theirs to change, so a later run may differ from the
quotes above; `crates/commonmeasure-cli/tests/demo_policy.rs` holds the
same policy to a loopback publisher whose licence does not change.

### Scopes, engagements, principals and allowances

The same file carries the rest of the declared policy:

- **Scopes** bind a working directory to an engagement and to the rules
  that govern work there; the mode, constraints and clearance of the
  matching scope apply, and the top level applies where nothing matches
  ([`docs/contracts/source-policy.md`](contracts/source-policy.md) §Scopes and
  principals).
- **Engagements** are the names records, policy and reporting are grouped
  under. `allow_telemetry_egress: true` on a scope is the clearance for that
  engagement's witnessed crossings to leave the machine (§7); without it
  nothing leaves.
- **Principals** are authenticated identities: a `principals` list names
  who may exercise the policy, resolved from an authentication basis the
  process cannot rewrite, and a principal that cannot be authenticated is
  refused rather than degraded ([`docs/contracts/source-policy.md`](contracts/source-policy.md)
  §Scopes and principals). That basis is
  the process's operating-system user and is unix-only, so a policy that
  declares `principals` refuses every mediated crossing on Windows; a
  policy declaring none is unaffected there.
- **Allowances** bound a principal's cumulative spend over a declared
  period, enforced from a local transactional ledger before any priced
  acquisition is dispatched ([`docs/contracts/run-output.md`](contracts/run-output.md)
  §`acquisition.quote.allowance`).

For a firm the natural shape is one principal per solicitor and one scope
per matter. Each solicitor works as their own operating-system user on
their own machine, so the `principals` list binds a policy name to that
user's numeric id (`id -u` prints it); each matter is an engagement kept in
its own directory, and a scope matches that directory, names the matter as
the engagement and carries the clearance for its records to leave. The
policy for one solicitor and one matter:

```json
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.legislation.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ],
  "principals": [
    {"principal": "solicitor-a", "os_user": 501, "require_scope": true}
  ],
  "scopes": [
    {
      "match": "/home/solicitor-a/matters/2026-014",
      "principal": "solicitor-a",
      "engagement": "matter-2026-014",
      "allow_telemetry_egress": true
    }
  ]
}
```

The scope declares no constraints of its own, so the two access rules at
the top level govern work in the matter directory. `require_scope: true`
means this solicitor's agent can work only inside a matter directory the
policy names: outside one, every mediated crossing is refused rather than
falling back to the top-level rules. A second solicitor is a second
`principals` entry with their own user id, and a second matter a second
scope; a directory holding client-confidential material gets a scope whose
`constraints` refuse every host. What each field does, and what declaring
any principal changes for every other user of the machine, is
[`docs/contracts/source-policy.md`](contracts/source-policy.md).

[`docs/GLOSSARY.md`](GLOSSARY.md) defines each term in one line.

Where the file comes from a hub rather than your editor, because
`deployment.json` pins the hub's signing key, the edge refreshes it at every
session start and before every relay run, and `commonmeasure doctor` adds a
line naming the revision in force and its expiry. An envelope that has
expired keeps enforcing its policy and that line says `stale since` when;
[`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md) §Cadence
and staleness has the rest.

## 5. The console

```sh
commonmeasure serve
```

serves the operator console on `http://127.0.0.1:4173`, loopback by default
(`--listen` moves it). It derives an index from the session evidence logs
and refreshes it before every answer, so a crossing recorded while the
console is open appears on reload. Its five sections (Overview, Record,
Policy, Sources, Compare) and what each shows are described in
`console/README.md`. A published run directory is not rendered by the
console; `commonmeasure inspect <dir>` prints its dossier.

## 6. Import history from before Common Measure

Work done before the plugin was installed left host transcripts behind.
Always look before importing:

```sh
commonmeasure import --dry-run
```

which reports, per host, what a real import would do (`<host>  N new
crossings from M transcripts`) and names the hosts it will not import from,
with reasons: Codex transcripts hold shell command text, and a URL inside a
command is not evidence that anything was retrieved.

```sh
commonmeasure import
```

imports what the transcripts evidence. Every imported row is recorded as
`reconstructed`, read back after the fact with nothing watching at the time,
and never counted with witnessed evidence. Import is idempotent: a second run
imports `0 crossings`.

## 7. Egress, only when you ask for it

Everything so far stays on your machine. The one command that can change that
is the relay, and on a fresh install it refuses:

```sh
commonmeasure relay
```

```text
commonmeasure: no telemetry receiver is configured: pass --receiver or set one
in ~/.commonmeasure/relay.json; nothing was projected and nothing was sent
```

There is no default destination, and no default hub either:

```sh
commonmeasure connect
```

```text
commonmeasure: no hub URL and no --token: run `commonmeasure connect <hub-url>
--token <token>` with the token an owner minted in the hub; there is no
default hub and nothing was sent
```

The hub is the service an organisation's machines enrol with, so that one
owner can send every machine the same policy and see the cleared evidence
each one delivers; [`docs/HUB.md`](HUB.md) says what it is and what leaves
the machine, and the hub's own documentation opens at Start here
(`/docs/start-here` on the hub).

An owner of your organisation mints the token in the hub's API keys page,
which prints the `connect` command to run. It exchanges the token for an
ingest key, written straight into `relay.json`, mints the edge's signing key
(`edge-key.json`, which never leaves the machine), records the key id the
hub assigned in `enrolment.json`, signs and uploads the proof that lists the
key in the hub's key directory, and makes a first relay run. With
`--managed` it also reads the hub's policy signer under the new ingest key,
writes `deployment.json` pinned to it, and makes a first policy
synchronisation, so a machine that takes the organisation's policy from the
hub needs no file written by hand (`docs/contracts/policy-envelope.md`
§Deployment mode). `commonmeasure disconnect` undoes the enrolment. With a receiver named in `relay.json`
(by `connect`, by hand, or with `--receiver`), the relay projects witnessed
crossings, never reconstructed ones and of refusals only a count per
session, into Content Telemetry v1.0 batches, spools them durably, and
delivers. What its report states is in `ARCHITECTURE.md`
§Components (the relay); what may leave and under whose clearance is in
`DECISIONS.md` §Session policy and egress; the wire format is
`ARCHITECTURE.md` §Content Telemetry boundary.
Delivery against a real receiver is exercised by the ignored live test in
`crates/commonmeasure-cli/tests/relay_e2e.rs`, whose doc comment says how to run it.

## 8. If you build from source

Nothing above needs a checkout. From one, the test suite, a build of the
binary, the batch runner and the replay mode are available:

```sh
cargo fetch                      # once, online: the dependency crates
cargo test --workspace --offline
```

The toolchain is rustc 1.97 or later (`Cargo.toml` `rust-version`). After the
one fetch the workspace builds and every test passes offline: tests that need
a network boundary start a real loopback origin and drive the production
adapters over it, so no external service is involved. The first build takes
a few minutes. The same checkout builds the binary the installer would
have placed, which then registers with a host exactly as in §1:

```sh
cargo install --path crates/commonmeasure-cli
```

Then publish a run with nothing configured:

```sh
cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output /tmp/commonmeasure-empty
cargo run -p commonmeasure-cli -- inspect /tmp/commonmeasure-empty
```

`inspect` prints the run dossier: the mode (`no-external-acquisition`), the
sealed manifest hash, and one block per supply plan, every claim ending with
a citation into the artefact it was read from ([`docs/READ-A-RUN.md`](READ-A-RUN.md) is the
five-minute tour of one). Every plan here is `unavailable`, each with a gap
naming what is missing:

- the provider plans: acquisition was not authorised, because external calls
  cost money and happen only under `--live` with a credential;
- every plan: no inference gateway is configured; the gap names
  `COMMONMEASURE_INFERENCE_ENDPOINT`.

The same unconfigured checkout can still run the whole acquisition path,
using the recorded provider responses in the repository:

```sh
cargo run -p commonmeasure-cli -- run demo/jobs/eu-ai-act-replay.json \
  --replay demo/recon --output /tmp/commonmeasure-replay
cargo run -p commonmeasure-cli -- inspect /tmp/commonmeasure-replay
```

This is the replay mode ([`docs/contracts/run-output.md`](contracts/run-output.md)): the committed
recorded bytes served from loopback origins through the real adapters,
parsers and policy. No external call is made and no credential is read; a
provider without a hash-verified recording fails the run. The dossier's
header states the mode and its meaning:

```text
mode         replay — provider responses were served from verified recordings over loopback, and
             no external provider was contacted. Inference is never replayed: an answer can only
             come from a real gateway call at run time  [s:/run/mode]
```

and each provider plan's `bytes` line names the recording behind its sealed
response, for example:

```text
  bytes        recorded replay, not live capture — the response is the 2026-08-01 recording of
               https://api.exa.ai/search, served from exa/exa-search.json over loopback; the sealed
               response hash equals the recorded hash, recomputed here from the sealed bytes, so
               these are those bytes  [s:/plans/1/acquisition/replay; r:/bindings/0]
```

Each provider plan acquired, admitted sources, sealed its response bytes and
is marked `replay-tested` rather than `live-verified`; `replay.json` links
each sealed response to its recorded source by hash. The plans stay
`unavailable` for one reason: no inference gateway; the model line and each
plan's gap say so. TollBit's recorded search returns candidates without
text, and its refusals and gaps say that too.

With `COMMONMEASURE_INFERENCE_ENDPOINT` set (`demo/gateway/tensorzero/` records
the reference sidecar), the same command completes real inference over the
replayed context: fixed supply, varying policy or model configuration. The
committed example `demo/output/replay/` is such a run.

The committed example in `demo/output/latest/` is a third variant of the
same replay supply, and it needs no gateway at all:
`demo/jobs/eu-ai-act-replay-rubric.json` declares a coverage rubric and an
as-of date, so every plan's admitted window is measured (which declared
rubric items it covered, at which byte spans, and each part's declared date
against the reference) and the router selects the plan whose weighted
coverage and freshness score highest. Its dossier's `selection` line
explains what was bought, what it covered against which rubric, and why it
won; `just rubric-example` regenerates it. The committed demonstrations are
listed in `README.md`.

---

This walkthrough covered an installed binary registered with a host, a
session that recorded its own crossings, a policy that refused one, a
console that shows it back, imported history, an egress boundary that
refuses until you name a destination, and, from a checkout, an empty run
and a replayed comparison over recorded supply. Absent at this
point: any evaluation beyond the deterministic checks (grounding:
`demo/jobs/eu-ai-act-replay-cited.json` requests citations; coverage and
freshness: `demo/jobs/eu-ai-act-replay-rubric.json` declares the rubric and
as-of date they measure against) and an inference gateway on your machine
until you configure one (`demo/gateway/tensorzero/`). A licensed fetch is
committed as a replay (`demo/output/redpine-replay/`); a live one needs a
Redpine credential. Each absence appears in the evidence as a named gap;
`ROADMAP.md` is the status board.
