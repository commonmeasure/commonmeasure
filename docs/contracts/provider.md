---
title: Provider capability contract
domain: extensions
audience: integrator
section: reference
---

# Provider capability contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

Supply adapters declare capabilities; the runtime does not assume them.
Terms like plan, crossing and admission are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).

An adapter declares only what it implements, never what its vendor documents. A
plan is validated against those declarations before execution, so a run cannot
discover at the crossing that it planned an operation nobody offers. The map
is in `crates/commonmeasure-supply` (`IMPLEMENTED_PROVIDERS` and each adapter's
`capabilities()`): every open-web adapter declares `search`, and six of them
— Exa, Firecrawl, Linkup, Parallel, Search1API and Tavily — declare `fetch`
as well (Keenable, Nimble, SERPdive, TinyFish, TollBit and You.com declare
`search` alone); the internal corpus adapter declares `query` and nothing
else; Redpine, a licensed supplier bought by quote then confirm, declares
`search` and `quote`; Ozone Live, retrieval over a licensed publisher corpus
with no quote gate, declares `search` and `fetch` (its per-result `licensed`
boolean names no licence, so it declares no `licensed`); Dataville declares
`search` alone, returning one Wikipedia or arXiv record per request; and the
local skill adapter declares `invoke` and nothing else. `licensed`, `report`
and `corroborate` are vocabulary no adapter declares. A corpus query is not
a web search, and running somebody's program is neither; no adapter declares
another adapter's capability.

Content and skills use **one dispatch path with disjoint declared
capabilities**. The rule is that content and skill suppliers are never
treated as having equivalent search, fetch, invocation, provenance, terms or
payment semantics, and the single dispatch path is what enforces it in code:
there is no parallel type system, and nothing can ask a local execution for
search semantics, a supplier's price or a licence it cannot state.

Declared capabilities also compose: Redpine's `search` is implemented *behind
its quote gate* (`quote` → purchase decision → `confirm`), so its direct
search method refuses, and a surface without a purchase decision (the
mediated session tools) must not dispatch to a quote-declaring provider.
Advertising one there would let a session tool spend the operator's balance
without a decision. The mediated surface refuses `invoke` for the same reason
and by the same mechanism: it dispatches only the capabilities it mediates,
and a session tool takes no decision authorising a spend and no decision
authorising an execution (`crates/commonmeasure-harness/src/mcp.rs`).

## Capabilities

- `search` — discover candidates from a query or objective. A provider that
  publishes a maximum page size smaller than the job's result limit answers at
  its own size: the adapter declares that ceiling in `maximum_search_results`
  and the runtime records a `coverage_gap` naming both numbers, so a
  comparison cannot run at two sizes with nothing saying so. That is a fact
  about the provider, not about the web — a search that simply returned little
  claims no gap;
- `fetch` — retrieve content for a known identifier or URL. Dispatched only
  when a suite names a `fetch_target`: each adapter declaring `fetch`
  retrieves that exact URL, and the job prompt and result limit do not
  govern the acquisition (`crates/commonmeasure-runtime/src/run.rs`);
- `query` — retrieve directly from a bounded corpus. Implemented by the
  `internal` adapter over a directory of documents the operator owns. A remote
  knowledge-base or retrieval endpoint needs an adapter to its existing API;
  that integration is not implemented by the directory adapter. The supplying
  system retains indexing, retrieval and access control. Because the corpus is
  bounded, a shortfall against the
  requested result count is recorded as a first-class `coverage_gap`; the
  internal corpus is the one supply whose "not found" is a fact about the
  supply. Its `corpus.json` may declare a licence, corpus-wide and per
  document where one corpus mixes rights; the internal corpus is the first
  supply in the system where a licence state other than unknown can be
  declared. Without a written declaration the state is unknown, because
  owning the directory is not a rights statement;
- `invoke` — execute a declared entrypoint with declared inputs and return
  its result, exit status and provenance. It is the one capability whose
  result is *produced* rather than retrieved, which is why it has its own
  word. Implemented by the local skill adapter over the operator's catalogue,
  and only `invoke`: a skill adapter declares no `search` (a catalogue of
  procedures is not a content index), no `query`, no `quote` (no supplier
  prices a local execution) and no `licensed`. `SupplyStep.limit` does not
  govern an invocation, because one invocation produces one result, and the
  record states this instead of carrying a number that decided nothing;
- `quote` — return price/terms before delivery. Implemented first by the
  Redpine adapter, whose provider offers it natively (`preview` returns a
  billing note, trial state and a `preview_id`; `confirm` settles): the
  quote's price and billing note enter plan evidence before the purchase
  decision, and a declined quote is a recorded decision
  ([`docs/contracts/run-output.md`](run-output.md) §Plans);
- `licensed` — attach a machine-readable licence or agreement reference;
- `report` — accept a use report or Content Telemetry projection;
- `corroborate` — return a provider-side receipt that can be joined to a
  crossing identifier.

## Skill supply

A skill is a named unit of procedure or executable capability, supplied by a
third party or the operator, that the runtime may choose between and invoke to
produce a result for a job. It is a **candidate**: one of several routes an
objective ranks, whose evidence is about what was supplied. It is not a
processor ([`docs/contracts/processor.md`](processor.md)) and not a harness plugin
(`plugin/`); the three-way distinction is defined once, in that document.

Two absences in the Agent Skill format determine the rest of this contract,
and the contract records each as an absence.

**It declares no version.** The allowed frontmatter properties are `name`,
`description`, `license`, `allowed-tools` and `metadata`, verified against
the validator's own allow-list, not inferred from documentation. The run
record asks for identifier and version; the version is therefore unknown and
is never fabricated. Identity is instead the declared `name` plus a bundle digest:
SHA-256 of `SKILL.md` and of the entrypoint bytes executed. This is the
processor contract's configuration-digest reasoning, except that here the
digest can be over what runs, because what runs is a file this runtime read.

**It declares no machine-readable invocation contract.** Scripts are described
in prose for a model to run, so an adapter cannot learn how to invoke a skill
*from* the skill. The operator declares it in a catalogue
(`COMMONMEASURE_SKILL_CATALOGUE`), following the `corpus.json` precedent, so
every invocation input carries operator provenance and not the skill's own
claim. The record keeps the two apart for each argument.

The consequences for the rest of this contract:

- **Licence.** A skill *can* declare a `license`, machine-readably. It
  licenses the procedure. What may be done with the output is stated
  nowhere, so the envelope's licence is `unknown` and the declaration is
  recorded as the skill's, namespaced.
- **Date.** Nothing published a produced result, so nothing dated it.
  Freshness does not apply to skill supply, and a freshness-weighted
  objective abstains over a skill plan rather than scoring it.
- **Charge.** No supplier prices a local execution, so the charge is unknown,
  not zero, and it is not routed through the quote-then-buy path, because
  there is no quote to decide on.

Nothing executes outside what policy admits. Policy has two independent
layers, each testable on its own:

1. **Machine and operator.** The catalogue declares, per skill, the root, the
   entrypoint, an absolute interpreter and an argument template. The
   entrypoint must resolve within the declared root (the corpus's
   containment rule), and a symlink that resolves outside it is refused
   without executing. No shell parses the command line; the argument vector
   is explicit. The environment is emptied, so the child cannot read the
   operator's provider credentials. The wall clock is bounded and enforced by
   signalling the child's process group. Output past the declared cap means
   the result is refused whole and not truncated.
2. **Job policy.** A denied or unlisted skill provider is refused before
   invocation and no process is spawned. A skill is an ordinary provider to
   eligibility, allow- and deny-provider policy and the router, because every
   skill is its own provider name, `skill:<name>`.

This is a supervised spawn, not a sandbox. The child runs as the operator with
the operator's access, and that leaves four things open. The recorded digests
are taken immediately before the spawn, so a file replaced between the read
and the exec runs under the earlier digest. A descendant that moves into its
own process group is beyond the signal that ends the wall clock. The output
cap bounds what is retained, not what a hostile program can make this process
allocate while reading. The emptied environment keeps provider credentials
out of the child's environment, but the child can read any credential file
the operator can.

## Normalised envelope

Every successful operation returns, when observed:

- provider and adapter version;
- operation and provider request identifier;
- canonical content identifier or URL;
- title, publisher/owner and publication/update time;
- content or excerpts with transformation metadata;
- media type and content hash;
- retrieval rank and provider score, namespaced rather than normalised;
- licence/agreement reference and permitted-use declarations;
- quote, observed charge, currency and billing unit;
- timestamps and latency;
- provider receipt or reporting endpoint;
- fields the adapter could not observe.

Unknown values remain unknown. An adapter never infers publisher consent from
crawler accessibility or converts missing licence metadata into permission.

A declared date follows the same rule as a licence: it is the supplier's
claim, mapped only from a field the provider has been observed to return (or,
without live evidence, is documented to return), and it is carried on the
envelope with provenance naming who declared it (a supplier's
`publishedDate`; the operator's `corpus.json` for the internal adapter,
whose `dates` map is the one place a document date can be declared).
Nothing verifies a declared date against when the content was written. An
envelope with no
declared date is undated: its date is unknown, not stale, and it is never
dated from a filesystem timestamp, a cache header or a clock.

Dataville selects `wiki` for `en.wikipedia.org` and `arxiv` for `arxiv.org`,
using the first mapped host in the job's order. With no hosts it selects
`wiki`; with only unmapped hosts it refuses before making a request. Its
`maximum_search_results` is one. It maps the canonical URL from
`data.metadata.url` for Wikipedia and `data.metadata.abs_url` for arXiv;
without that URL there is no envelope. `data.last_updated` carries update-time
provenance (Wikipedia revision time, not publication time). Metadata and
stale-copy notices stay in native metadata. When `data.attribution.license.url`
is present, the envelope records it as a supplier-declared licence reference;
Dataville's declaration about upstream content does not establish the operator's
permitted uses, and the adapter declares no `licensed` capability. Its charge
is observed USD from `usage.request_cost`, unknown when absent, regardless of
the published rate. An anonymous-tier response fails even on HTTP 200 because
the configured key was not accepted. Wikipedia-only URL lookup does not fulfil
the general `fetch` capability and is not wired.

## Writing an adapter

An adapter is one Rust module in `crates/commonmeasure-supply/src/` implementing the
`SupplyAdapter` trait (`crates/commonmeasure-supply/src/lib.rs`): `provider()` returns
its name, `capabilities()` returns the capabilities it implements, and it
overrides only the methods for those capabilities (`search`, `query`,
`fetch`, `quote`, `invoke`); the trait's defaults return
`CapabilityUnavailable` for the rest. The credential travels in a request
header and never in a command line, a URL or an artefact. Registration is
three edits in the same file: the name in `IMPLEMENTED_PROVIDERS`, the
credential variable in `required_variable`, and the constructor arm in
`remote_adapter`. The declared capability list also appears in
`declared_provider_ref`, which plan validation and the mediated status tool
read. An adapter's verification state (below) is `spec-verified` at most
until the response bytes of a dated live call are captured and kept as
evidence.

## Verification states

- `planned` — interface identified; no implementation claim;
- `fixture-tested` — behaviour passes recorded or synthetic fixtures;
- `replay-tested` — a run served the committed, redacted bytes of a dated
  live response through the real transport, adapters and policy, and sealed
  the result linked to the recorded input by hash
  ([`docs/contracts/run-output.md`](run-output.md)). Distinct from `live-verified`: no call
  reached the provider;
- `spec-verified` — current primary documentation supports the implementation;
- `live-verified` — a dated call against the real supply passed and its
  evidence is recorded, authenticated where the provider is remote (a local
  corpus read has no credential to present; the sealed response is the
  evidence). A dated execution of a real bundle receives the same state on
  the same terms (the supply was reached and the exact bytes that came back
  are sealed), and the dossier reports it as an execution rather than a
  call, because nothing was called. A run claims this for a plan only when
  the exact response bytes are sealed beside it
  ([`docs/contracts/run-output.md`](run-output.md));
- `production-observed` — sustained operator traffic confirms behaviour and
  failure modes.

## State of each adapter

Each state below rests on a test in this repository or on the host
integration evidence, and is given per capability because a state earned by
one operation is not inherited by another. `live-verified` here means a test
serves the recorded response of a dated authenticated call to the real
supplier, or a hosted edge made the call
([`docs/contracts/supplier-credentials.md`](supplier-credentials.md) §Status). The
recorded responses are not published, except Dataville's, which its test
carries inline with body and abstract text elided and their original lengths
recorded; the tests that read the others run where
`COMMONMEASURE_PRIVATE_EVIDENCE` is set (`CONTRIBUTING.md`). Where no test
reads a live call, the state is `fixture-tested`: the adapter's parser and
request construction are exercised over the supplier's documented shapes
through the real transport and a loopback origin. No adapter is
`production-observed`.

| Adapter | Capability | State | Evidence |
|---|---|---|---|
| Dataville | `search` | `live-verified`; `fixture-tested` against the recordings | `crates/commonmeasure-supply/tests/dataville_spec.rs` carries elided authenticated Wikipedia and arXiv responses recorded on 25 September 2026 inline and serves them through the production transport. The charge is observed USD; no full run replay is claimed. |
| Exa | `search` | `live-verified`, `replay-tested` | `crates/commonmeasure-supply/tests/recorded_replay.rs` serves the recorded call; `crates/commonmeasure-cli/tests/replay_contract.rs` replays it through a run of `demo/jobs/eu-ai-act-replay.json`; a hosted edge searched with a key the hub released. The charge is observed. |
| Exa | `fetch` | `live-verified`; `fixture-tested` against the recording | `recorded_replay.rs` serves the recorded `/contents` call; no run replays it |
| Firecrawl | `search` | `live-verified`, `replay-tested` | `recorded_replay.rs`; `replay_contract.rs` over `demo/jobs/eu-ai-act-replay.json` |
| Firecrawl | `fetch` | `live-verified`; `fixture-tested` against the recording | `recorded_replay.rs` serves the recorded scrape; no run replays it |
| Tavily | `search` | `live-verified`, `replay-tested` | `recorded_replay.rs`; `replay_contract.rs` over `demo/jobs/eu-ai-act-replay.json`. The charge is quoted from the published price and never observed. |
| Tavily | `fetch` | `live-verified`, `replay-tested` | `replay_contract.rs` replays the recorded call through a run of `demo/jobs/recon-w3c-prov-fetch.json`; `recorded_replay.rs` covers the documented `/extract` shape |
| TollBit | `search` | `live-verified`, `replay-tested` | `recorded_replay.rs`; `replay_contract.rs` over `demo/jobs/eu-ai-act-replay.json` |
| Parallel | `search` | `live-verified`, `replay-tested` | `crates/commonmeasure-cli/tests/replay_contract.rs` replays the recorded call through a run of `demo/jobs/recon-w3c-prov-search.json`; `recorded_replay.rs` covers the documented shapes |
| Parallel | `fetch` | `live-verified`, `replay-tested` | `replay_contract.rs` replays the recorded call through a run of `demo/jobs/recon-w3c-prov-fetch.json` |
| Linkup | `search` | `live-verified`, `replay-tested` | `replay_contract.rs` over `demo/jobs/recon-w3c-prov-search.json`; `crates/commonmeasure-supply/tests/linkup_spec.rs` covers the documented shapes |
| Linkup | `fetch` | `live-verified`, `replay-tested` | `replay_contract.rs` replays the recorded call through a run of `demo/jobs/recon-w3c-prov-fetch.json` |
| Search1API | `search`, `fetch` | `fixture-tested` | `crates/commonmeasure-supply/tests/search1api_spec.rs` |
| Keenable | `search` | `live-verified`, `replay-tested` | `replay_contract.rs` over `demo/jobs/recon-w3c-prov-search.json`; `crates/commonmeasure-supply/tests/keenable_spec.rs` covers the documented shapes |
| Nimble | `search` | `live-verified`, `replay-tested` | `replay_contract.rs` over `demo/jobs/recon-w3c-prov-search.json`; `crates/commonmeasure-supply/tests/nimble_spec.rs` covers the documented shapes |
| SERPdive | `search` | `fixture-tested` | `crates/commonmeasure-supply/tests/serpdive_spec.rs` |
| TinyFish | `search` | `fixture-tested` | `crates/commonmeasure-supply/tests/tinyfish_spec.rs` |
| You.com | `search` | `live-verified`, `replay-tested` | `replay_contract.rs` over `demo/jobs/recon-w3c-prov-search.json`; `crates/commonmeasure-supply/tests/you_spec.rs` covers the documented shapes |
| Ozone Live | `search` | `live-verified` | a hosted edge searched with a key the hub released; `crates/commonmeasure-supply/tests/ozone_spec.rs` covers the documented shapes. The charge is unknown. |
| Ozone Live | `fetch` | `fixture-tested` | `ozone_spec.rs` |
| Redpine | `quote`, and `search` behind it | `live-verified` | `crates/commonmeasure-supply/tests/redpine_quote_replay.rs` serves the five recorded legs of a purchase covered by a trial: `initialize`, balance, inspect, `preview`, `confirm`. No charge in currency has been observed. |
| `internal` | `query` | `fixture-tested` | `crates/commonmeasure-supply/tests/internal_corpus.rs` over `demo/corpus/` |
| `skill:*` | `invoke` | `live-verified` | two third-party bundles were executed and the run sealed; `crates/commonmeasure-cli/tests/inspect_dossier.rs` reads it. `crates/commonmeasure-supply/tests/skill_invocation.rs` covers containment, limits and the record with probe bundles. |
