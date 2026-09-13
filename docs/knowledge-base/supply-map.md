---
title: Supply map
draft: true
---

# Supply map

The supplier landscape as it bears on adapter planning. This is not an
endorsement or a verification record: the verification state of each
adapter and the evidence behind it are in
[`docs/knowledge-base/provider-verification.md`](provider-verification.md), and the recorded responses
are in `demo/recon/`. Provider descriptions below are from vendor
documentation unless a verification finding is cited.

## The licence finding, applied to this whole document

Of the providers called live, **only TollBit returns any licence, rights or
provenance metadata at all**, and what it returns is publisher identity plus
an availability boolean contradicted by its rates endpoint. Every open-web
provider returns no licence, rights or provenance metadata on the tested
endpoints: they provide search and extraction without publisher licence
assurance.

Consequences that bind adapter and console work:

- the licence/provenance column is **unknown** for every open-web provider, and
  unknown is the honest value, not a defect to be filled in;
- no adapter may promote `og:site_name`, a domain name or a favicon into a
  publisher rights claim;
- crawler accessibility is not permission. Firecrawl successfully scraping a
  page says nothing whatever about the rights to use it.

## Open-web acquisition

### Exa

Search plus page contents, highlights and summaries. Suitable for measuring
semantic search quality, source selection and context volume. Public pricing
separates search from returned contents. It sells web search and extraction,
not publisher licence assurance.

Primary links:

- https://exa.ai/docs/reference/search
- https://exa.ai/pricing

Verification findings: the only provider in the set that reports cost in
currency; contents can be served from cache, still billed, so its latency
is not comparable to a cold fetch; no licence field.

### Parallel

Separate Search and Extract APIs, objective-focused excerpts, MCP distribution,
and paid machine-callable search/extract endpoints. Particularly useful for
testing whether pre-shaped excerpts reduce context cost without harming answer
quality.

Primary links:

- https://docs.parallel.ai/search/search-quickstart
- https://docs.parallel.ai/extract/extract-quickstart
- https://docs.parallel.ai/getting-started/pricing
- https://docs.parallel.ai/integrations/agentic-payments

### Firecrawl

Search, scrape, crawl, map and browser operations. The v2 search endpoint can
return results and scraped markdown in one request and reports credits used.
Useful as both a search-plus-extract route and a fetch route for known URLs.

Primary links:

- https://docs.firecrawl.dev/api-reference/v2-introduction
- https://docs.firecrawl.dev/api-reference/endpoint/search

Verification findings: reports credits, not currency, and the field moves
between endpoints; the only provider that declares its own cache state; no
licence field.

### Tavily

Search, extract, crawl and map endpoints on a credit model. Like the other
open-web providers it conveys no publisher licensing.

Primary links:

- https://docs.tavily.com/documentation/api-reference/endpoint/search
- https://docs.tavily.com/documentation/api-credits

Verification findings: reports no cost anywhere, so Tavily acquisition cost
is quoted only and must never be displayed as observed; `response_time` is
in seconds; `raw_content` is null unless requested; no licence field.

### TinyFish

Four API surfaces behind one key — Search
(ranked results with snippets), Fetch (URL to extracted content), Agent
(natural-language web automation) and Browser (remote CDP sessions) — plus a
web-native model it calls Mako. Search and Fetch are published as free at any
wallet balance; Agent and Browser meter from a wallet. Only the Search API is
wired (`crates/commonmeasure-supply/src/tinyfish.rs`, `spec-verified`, no credential
held).

Primary links:

- https://docs.tinyfish.ai/search-api/reference.md
- https://www.tinyfish.ai/
- https://www.tinyfish.ai/compare/tinyfish-vs-tavily

**Supply class, stated plainly.** TinyFish is the first provider in this map
whose own homepage advertises defeating the mechanisms by which sites express
refusal. All three pages above were read 25 August 2026 through the mediated
fetch, with content hashes in the operator session record. The homepage sells
"Stealth sessions that navigate login walls, defeat anti-bot protections, and
handle dynamic rendering" (hash `sha256:405432…`); its FAQ answers "What does
'stealth' mean?" with "C++ patches compiled directly into the Chromium binary —
not JS injection… 85% pass rate across Akamai, Cloudflare, DataDome,
PerimeterX, and Imperva"; and the comparison page (hash `sha256:64d462…`)
scores fetch coverage on news publishers (15/15 against Tavily's 12/15) as a
selling point, listing a "Stealth Browser" and "Credential Vault" among the
capabilities its competitor lacks. The licence finding at the top of this
document — crawler accessibility is not permission — understates the position
here: an anti-bot barrier or a login wall is a site's *expressed* refusal, and
content obtained by engineering past one is further from permission than
content that was merely reachable. Consequences:

- every TinyFish result stays licence-unknown, exactly like the other open-web
  providers: a free result is not a licensed one, and a fast fetch is not a
  permitted one;
- the Agent, Browser, credential-vault and logged-in-profile capabilities are
  deliberately not wired, and adding them would need a named product decision,
  because they execute crossings whose refusal is already on record at the
  origin;
- an operator whose source policy distinguishes consent posture has, in this
  entry and the adapter's declared capabilities, what it needs to refuse this
  supplier explicitly rather than silently (supply class belongs in declared
  capabilities and the verification record, not in a directory name;
  `crates/commonmeasure-supply/src/lib.rs`).

TinyFish's funding and the public challenge to its consent posture are in
the company's market-signals record, kept outside this repository. One
unverified observation, recorded so nobody re-notices it as a discovery:
SERPdive's documented search models are `krill`, `mako` and `moby`, and
TinyFish's web-native model is also named Mako; whether any relationship exists
between the two vendors is unknown and nothing here assumes one.

## Licensed and commercial supply

### Redpine

Licensed non-public datasets and real-time signals, exposed through API, MCP and
CLI with per-use economics. It is a supply partner/adapter target, not the
buyer-side optimiser.

Primary links:

- https://www.redpine.ai/products
- https://www.redpine.ai/pricing

### Ozone Live

Retrieval over a corpus the Ozone Project licenses from its member publishers,
in the shape of an open-web grounding API: a query returns scored passages
with publisher identity, and an ownership manifest lists which domains Ozone
licenses so a router can tell, from a URL any other provider returned, whether
a licensed source for the same page exists. It calls no model and generates
no answer. Search and fetch are wired
(`crates/commonmeasure-supply/src/ozone.rs`, `spec-verified`, credential held,
no live call recorded).

Primary links:

- https://api.ozone.live/openapi.yaml
- https://api.ozone.live/v1/capabilities

Verification findings: `licensed: true` and a `publisher_id` on every result,
which is more rights metadata than any other provider in this map returns and
still not a licence reference ([`docs/knowledge-base/provider-verification.md`](provider-verification.md)
§Ozone Live); no cost field and no published price; `published_date` is
documented, and the vendor's own example dates a 2019 article in 2026
([`docs/knowledge-base/unverified-assumptions.md`](unverified-assumptions.md) OZ1). The licence finding at
the top of this document holds here in a narrower form: a supplier's
statement that it holds a licence is not a statement of what the operator
may do.

The ownership manifest (`GET /v1/publishers/domains`) is the part of the
service the rest of this map lacks. It answers "does a licensed supplier hold
this host?" cheaply and in bulk, which is a routing input: a run that has an
open-web result for a page in Ozone's manifest could prefer the licensed
copy. That decision belongs to the runtime beside the declarations read
before a crossing, not to an adapter, and is not built.

### Cashmere

Publisher licensing infrastructure with API/MCP access, authentication,
publisher-specific terms and pay-per-use or subscription models. Public access
requires approval. Existing private research says Cashmere has engaged with
Content Telemetry; this must not be described publicly without permission.

Primary links:

- https://cashmere.io/product/connect
- https://cashmere.io/terms
- https://cashmere.io/pricing

### TollBit

Metered premium-publisher content. Its self-reporting endpoint is
supplier-side evidence, not a substitute for the independent operator
record.

Verification findings (`demo/recon/tollbit/`): search authenticates with the
`TollbitKey` header and is the only provider in the map that returns
provenance (`publisher {domain, name}`, `publishedDate` and
`availability {discoverable, readyToLicense}`). A paid fetch is blocked on
this account: `GET /dev/v2/rates/{url}` returns 403 on every URL tried,
including results marked `readyToLicense: true`, and token creation returns
500, so `readyToLicense` is not a licensability signal here. Premium
publishers in the index (CNN, NPR, Reuters, The Atlantic, AP, PBS, Pew) are
all `discoverable: true, readyToLicense: false`. No TollBit price has been
observed; rate, token and content stay `fixture-tested`.

### Channel3

A product corpus for shopping agents with text/image search, product and price
data, price tracking and affiliate economics. It tests a different kind of
context job: structured commercial evidence rather than editorial web search.

Primary links:

- https://docs.trychannel3.com/

## Settlement and licensing infrastructure

### Supertab

Not a content supplier: Supertab Connect is
licensing, metering and settlement infrastructure sitting between publishers or
API owners and agents. The publisher side declares machine-readable licences
(the RSL open standard) and enforces them at the edge via a Crawler
Authentication Protocol; the agent side acquires a licence token through the
Open Licensing Protocol and presents it as `Authorization: License <JWT>`;
usage events are recorded asynchronously, aggregated tab-style — the vendor's
own framing is a restaurant tab for agents — and settled deferred over
whichever rail the parties choose (cards, ACH, wire, invoicing).

Two distinct uses for Common Measure, which must not be conflated:

- **Acquisition-side adapter target.** Where a supplier fronts its content with
  Connect, the licence-token flow is what an adapter would speak. Same category
  of interest as Cashmere, and unlike the open-web providers, licence metadata
  is the product rather than an absent column.
- **Settlement counterparty for agent deployments.** The tab model — meter
  small events, aggregate, settle deferred — matches the operator-side need to
  settle agent consumption without per-call payment. `PRODUCT.md` keeps
  settlement rails outside the Common Measure boundary; Supertab would be exactly
  such an external rail, with Common Measure supplying the metered evidence and
  Supertab performing the settlement. Nothing in this entry moves settlement
  inside the boundary.

Everything above is vendor documentation: no credential, no call made, no
price observed. The public docs cover the licence-token flow and
SDKs (PHP, Python, TypeScript) but not metering, budget or settlement
endpoints, so the tab mechanics rest on vendor claims only. A direct founder
relationship exists on our side and a design conversation is available before
any credentialled probe; as with the Cashmere note, this must not be described
publicly without permission.

What the intermediary model (`DECISIONS.md` §Product, `ROADMAP.md` §Licensed
access through an external settlement rail)
needs from a rail, and what the design conversation has to establish:

- an aggregator account: one network System with a sub-credential per
  enrolled edge, each settled to its operator's own payment method with
  Supertab as merchant of record, and a pseudonymous subject in the token so
  the publisher sees the network identity and the key id, not the operator;
- the claims in the licence token, and which of them the publisher's edge
  logs;
- whether the Crawler Authentication Protocol verifies a Web Bot Auth
  signature beside the `License` header, or ignores it;
- whether Connect consumes Content Telemetry usage events or requires its
  own reporting, since reporting twice is a cost the operator bears;
- what the crawler-operator side pays (the published prices, $99 and $249 a
  month, are publisher-side), and whether the marketplace lists a trusted
  intermediary.

Without the sub-credential, Supertab is an acquisition-side adapter under
the operator's own account and name, and the network identity is not
presented on those requests.

Primary links:

- https://www.supertab.co/
- https://connect-docs.supertab.co/introduction/overview.md
- https://connect-docs.supertab.co/guides/acquire-license.md
- https://connect-docs.supertab.co/licensing/open-licensing-protocol.md

## Skill supply

The other half of the supply thesis, and the half nothing above covers: a
skill is a unit of *procedure*,
so its result is produced rather than retrieved, and none of the licence,
rights or freshness reasoning in this document transfers to it unchanged.

The contract rests on a survey of 28 real `SKILL.md` bundles installed on
an operator's machine for Claude Code and Codex, 17 distinct names, plus the
Codex system skills. Facts:

| | Skills as supply |
|---|---|
| identity | declared `name` in `SKILL.md`, plus digests of the bytes executed |
| version | **the format has none.** Allowed frontmatter keys are `name`, `description`, `license`, `allowed-tools`, `metadata` — checked against the real validator's allow-list, not against documentation |
| invocation contract | **none that is machine-readable.** Scripts are described in prose for a model to run, so the operator declares how to invoke, in a catalogue |
| licence | a `license` key exists, and is the *first* supply in this system where a supplier states a licence machine-readably. It licenses the procedure; the output's rights are unstated |
| date | none. Nothing published a produced result |
| price | none. No supplier prices a local execution |
| capability | `invoke`, alone ([`docs/contracts/provider.md`](../contracts/provider.md)) |

Two consequences worth stating where a reader of the content sections will
trip over them. First, the licence column being *present* here does not make
skill supply better-licensed than open-web content: what is licensed is the
program, and what enters a context window is its output, which nothing
licenses at all. Second, the freshness column being absent is not the open
web's "unknown date" — it is a category difference, and a freshness-weighted
objective abstains over a skill plan rather than treating it as undated
content.

Where a skill is distributed inside a harness plugin, the plugin is host
integration and the skill inside it is supply. `plugin/` is this product's;
a bundle under a harness's own skills directory is somebody else's supply
that the operator has admitted. [`docs/contracts/processor.md`](../contracts/processor.md) draws the
whole three-way distinction.

## Capability matrix

The per-provider, per-capability states live in one place, the summary table
of [`docs/knowledge-base/provider-verification.md`](provider-verification.md); this document does not
repeat them. Two reading notes carry over. `corroborate` is unverified for
every open-web provider because a request identifier (Exa `requestId`,
Tavily `request_id`, Firecrawl `id`) is not a receipt: the capability asks
for a provider-side artefact that can be joined to a crossing identifier, and
nothing tested returns one. `report` was not exercised against TollBit
because posting a self-reported transaction that did not happen would put
false evidence into a supplier's records; its `selfReport` schema meters
consumption for billing (`timesUsed`, licence identity) and records nothing
about what the content did in an answer.

Cashmere, Channel3 and Supertab are `planned` throughout: no credentials,
and Cashmere additionally requires approval that would mean accepting terms.

## Normalisation warning

These providers are not interchangeable. Search engines, extractors, licensed
corpora and product databases expose different capabilities and rights. The
experiment contract normalises the context delivered to inference while
retaining the differences as evidence; it must never erase them to manufacture
a league table.

Live calls found three forms of cost reporting across providers: decimal
USD, integer credits and no reported cost. Only one can be compared with a
published price without a plan assumption. A cost axis that silently converts
all three into dollars would be manufacturing exactly the comparability the
contract forbids. Where a figure is derived or quoted rather than observed, the
console must say so.
