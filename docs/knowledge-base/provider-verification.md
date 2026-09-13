---
title: Provider verification record
draft: true
---

# Provider verification record

Dated record of what was actually tested against each provider, using the
verification states in [`docs/contracts/provider.md`](../contracts/provider.md). States here are claims
about evidence, not about quality or suitability.

Committed evidence lives in `demo/recon/<provider>/`. The recorded search
and fetch responses also serve the `--replay` run mode
(`demo/recon/replay-manifest.json` pins each capture by hash); a state below
says `replay-tested` only where a committed job replays that capture. Every
`live-verified` state below rests on a capture in this repository; a provider
with no committed capture is `spec-verified` and the row says why.

Rule applied throughout: a state is only as strong as the artefact behind it.
Where an operation was not exercised, it is recorded as not exercised rather
than inherited from a neighbouring operation that was.

---

## Summary

| Provider | State | Basis |
|---|---|---|
| Exa | `live-verified` (search + fetch) | search and contents, dated authenticated calls 1 Aug 2026, cost observed; search `replay-tested` (manifest entry, served by `demo/output/replay/`); contents is the harness `fetch` capability, `live-verified` on the 1 Aug 2026 capture `demo/recon/exa/exa-contents.json` and `fixture-tested` against it — not `replay-tested`, the capture is not in `demo/recon/replay-manifest.json` and no run serves it |
| Tavily | `live-verified` (search + fetch); cost **not** verifiable | search: 1 Aug 2026 capture `demo/recon/tavily/tavily-search.json`, `replay-tested`; fetch (`/extract`): 5 Sep 2026 capture `demo/recon/tavily/tavily-extract.json` (200, 808 ms, page in `raw_content`, no cost field), `replay-tested` through `demo/jobs/recon-w3c-prov-fetch.json`; search charge quoted from the published price, extract unknown (see the Tavily section) |
| Firecrawl | `live-verified` (search + fetch) | search and scrape, dated authenticated calls 1 Aug 2026, credits observed; search `replay-tested` (manifest entry, served by `demo/output/replay/`); scrape is the harness `fetch` capability, `live-verified` on the 1 Aug 2026 capture `demo/recon/firecrawl/firecrawl-scrape.json` and `fixture-tested` against it — not `replay-tested`, the capture is not in `demo/recon/replay-manifest.json` and no run serves it |
| TollBit | `live-verified` (search only); `fixture-tested` (rate, token, content) | search authenticates and returns; every priced operation blocked |
| Parallel | `live-verified` and `replay-tested` (search + fetch) | 5 Sep 2026 captures `demo/recon/parallel/parallel-search.json` (200, 2,890 ms, one `sku_search` observed, no currency disclosed) and `demo/recon/parallel/parallel-extract.json` (200, 404 ms, one `sku_extract_excerpts` observed); both in `demo/recon/replay-manifest.json`, replayed by `demo/jobs/recon-w3c-prov-search.json` and `demo/jobs/recon-w3c-prov-fetch.json` |
| Linkup | `live-verified` and `replay-tested` (search + fetch) | 5 Sep 2026 captures `demo/recon/linkup/linkup-search.json` (200, 3,459 ms, 3 results, no cost field, charge quoted at USD 0.005) and `demo/recon/linkup/linkup-fetch.json` (200, 508 ms, page `markdown`, no cost field, charge unknown); both in the replay manifest |
| Search1API | `spec-verified` (search + fetch) | no `SEARCH1API_API_KEY` is configured, so no live call is recorded in this repository; the adapter follows the published endpoint documentation and its parser is exercised over documented shapes (`crates/commonmeasure-supply/tests/search1api_spec.rs`); a configured key restores the live check through `demo/jobs/recon-w3c-prov-search.json` |
| SERPdive | `spec-verified` (search); live calls failing | three dated calls on 5 Sep 2026 with `SERPDIVE_API_KEY`, three different queries, each answered `502 search_failed` ("The search returned no usable sources. You were not billed"); no bytes to seal; the parser is exercised over documented shapes (`crates/commonmeasure-supply/tests/serpdive_spec.rs`); charge quoted at 1 `mako` credit |
| Keenable | `live-verified` and `replay-tested` (search) | 5 Sep 2026 capture `demo/recon/keenable/keenable-search.json` (200, 492 ms, 10 results in `results[{title,url,description,snippet}]`, no cost field, charge unknown); the single-domain `site` filter for `www.w3.org` returned `dvcs.w3.org` pages, which the job's source policy refused at admission; in the replay manifest |
| You.com | `live-verified` and `replay-tested` (search) | 5 Sep 2026 capture `demo/recon/you/you-search.json` (200, 839 ms, 3 results in `results.web`, no cost field, charge quoted at USD 0.005); in the replay manifest |
| Nimble | `live-verified` and `replay-tested` (search) | 5 Sep 2026 capture `demo/recon/nimble/nimble-search.json` (200, 1,128 ms, 3 results; a longer query timed out at the 30 s exchange budget, a shorter one answered); no cost, credit or usage field, charge quoted at 2 credits; in the replay manifest |
| TinyFish | `spec-verified` (search adapter; no credential) | adapter built 25 Aug 2026 to the published Search API reference, loopback spec tests; no live call, no capture. Supply-class note in the supply map |
| Ozone Live | `spec-verified` (search + fetch) | adapter built 9 Sep 2026 to the machine-readable specification `https://api.ozone.live/openapi.yaml` and the API reference, loopback spec tests (`crates/commonmeasure-supply/tests/ozone_spec.rs`); one live search on 9 Sep 2026 answered 200 in 2,378 ms with 3 rows, but its bytes are licensed passages and are not committed, so no capture and no replay; no cost field and no published price, charge unknown |
| Redpine | `live-verified` (MCP discovery, priced fetch under trial) | first licensed content fetch 4 Aug 2026; quote→confirm exercised, trial meter observed; no currency charge yet observed; REST search untested |
| Cashmere | `planned` | access requires approval; not tested |
| Channel3 | `planned` | no credential; not tested |
| Supertab | `planned` (added 4 August 2026) | no credential; docs read only; not tested |
| Local skills (`skill:*`) | `live-verified` (`invoke`) | two real third-party bundles executed 6 August 2026; results, exit statuses and digests sealed in `demo/output/skills/` |

**No provider is `production-observed`.** Nothing here has carried sustained
operator traffic.

---

## Exa — `live-verified`

**Date:** 1 August 2026.

**What was done:** two authenticated calls with `EXA_API_KEY` in the `x-api-key`
header. `POST /search` with `numResults: 2` and truncated text contents;
`POST /contents` for a single public-sector URL.

**Artefacts:** `demo/recon/exa/exa-search.json`,
`demo/recon/exa/exa-contents.json`, `demo/recon/exa/NOTES.md`.

**Also spec-verified:** https://exa.ai/docs/reference/pricing, read 1 August
2026. Published USD 7 / 1k search requests and USD 1 / 1k content pages, both of
which the observed charges matched exactly.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| `search` | `live-verified` | 200, 2 results, 2344 ms, `costDollars.total` 0.007 |
| `fetch` (`/contents`) | `live-verified` | 200, 217 ms, `costDollars.total` 0.001, served `cached` |
| `quote` | not exercised | no pre-delivery price endpoint identified |
| `licensed` | **absent** | no licence field on any response; `author` present and `null` |
| `report`, `corroborate`, `query` | not exercised | no endpoint identified |

**Qualification on the cost claim:** the *total* is verified against the
published price. The *breakdown* is not reliable — on a combined
search-plus-contents call the entire charge was attributed to `search.neural`
with no `contents` key present. Cost separation must be derived from
`costDollars.total` per call, not from the categories inside it.

---

## Tavily — `live-verified` for search, cost unverifiable

**Date:** 1 August 2026.

**What was done:** one authenticated `POST /search` (`max_results: 2`,
`search_depth: "basic"`) with a bearer token from `TAVILY_API_KEY`, plus three
reads of `GET /usage` — before the search, immediately after, and several
minutes later.

**Artefacts:** `demo/recon/tavily/tavily-search.json`,
`tavily-usage.json`, `tavily-usage-recheck.json`, `tavily-usage-final.json`,
`demo/recon/tavily/NOTES.md`.

**Also spec-verified:** https://docs.tavily.com/documentation/api-credits, read
1 August 2026. Basic search 1 credit, advanced 2 credits, USD 0.008 per credit
on pay-as-you-go.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| `search` | `live-verified` | 200, 2 results, 1326 ms, `request_id` returned |
| `fetch` (`/extract`) | `live-verified`, `replay-tested` | 200, 1 result with `raw_content`, 808 ms, 5 September 2026, `demo/recon/tavily/tavily-extract.json`; charge unknown — the response carries no cost field and the published rate (1 credit per 5 successful extractions, `docs.tavily.com/documentation/api-credits`, read 4 September 2026) states no single-URL figure; a failed extraction is documented as never charged and is quoted at 0 credits |
| `quote` | **absent** | no pre-delivery price |
| `licensed` | **absent** | no licence, publisher, author or rights field |
| `report`, `corroborate`, `query` | not exercised | no endpoint identified |

**`fetch` (`/extract`), 5 September 2026.** One call through the `fetch`
capability against `https://www.w3.org/TR/prov-dm/` returned `200` with the
page in `raw_content` (`demo/recon/tavily/tavily-extract.json`, text elided;
manifest entry `tavily`/`fetch`, replayed by
`demo/jobs/recon-w3c-prov-fetch.json --replay demo/recon`). A URL the
extractor cannot read returns `200` with an empty `results` array and a
`failed_results` entry, sealed as an acquisition with no admissible content.
The extract response carries no cost field, and the published rule (one
credit per five successful extractions, nothing for a failed one;
`docs.tavily.com/documentation/api-credits`, read 4 September 2026) states no
per-call figure, so a successful extraction records an unknown charge and a
call whose only URL failed records zero.

**The cost state is deliberately not `live-verified`.** The search response
carries no cost or credit field, and the account usage counters read zero across
all three reads despite a successful billed search in between. Tavily
acquisition cost in this demo is a **quoted** figure with no observed
counterpart, and must be labelled as such wherever it is displayed. Recording
USD 0.008 as an observed charge would be a fabrication.

---

## Firecrawl — `live-verified`

**Date:** 1 August 2026.

**What was done:** two authenticated calls with a bearer token from
`FIRECRAWL_API_KEY`. `POST /v2/search` with `limit: 2`; `POST /v2/scrape` for a
public-sector URL with `formats: ["markdown"]`.

**Artefacts:** `demo/recon/firecrawl/firecrawl-search.json`,
`demo/recon/firecrawl/firecrawl-scrape.json`,
`demo/recon/firecrawl/NOTES.md`.

**Also spec-verified:** https://docs.firecrawl.dev/api-reference/endpoint/search,
read 1 August 2026. Documents top-level `creditsUsed` on search; does not
document the nested location observed on scrape.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| `search` | `live-verified` | 200, 2 results, 1877 ms, `creditsUsed` 2 at top level |
| `fetch` (`/scrape`) | `live-verified` | 200, 611 ms, `creditsUsed` 1 at `data.metadata.creditsUsed`, `cacheState: hit` |
| `quote` | **absent** | no pre-delivery price |
| `licensed` | **absent** | rich page metadata, no rights field |
| `report`, `corroborate`, `query` | not exercised | no endpoint identified |

**Qualification:** the charge is in **credits, not currency**. No USD figure is
returned and the conversion depends on plan. Any dollar figure shown for
Firecrawl is derived, and must be labelled derived.

---

## TollBit — `live-verified` for search only

**Date:** 1 August 2026.

**What was done:** six authenticated requests with `TOLLBIT_API_KEY` in the
`TollbitKey` header against `https://gateway.tollbit.com`. Two searches (one at
`size: 2`, one at `size: 20`), three rate lookups, one content-token creation
capped at 10,000 micros (USD 0.01).

**Artefacts:** `demo/recon/tollbit/tollbit-search.json`,
`tollbit-search-wide.json`, `tollbit-rates.json`,
`tollbit-rates-second-ready.json`, `tollbit-rates-not-ready.json`,
`tollbit-token-content.json`, `demo/recon/tollbit/NOTES.md`.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| `search` | `live-verified` | 200, 20 results, 914 ms, publisher and availability populated |
| `quote` (`/rates`) | `fixture-tested` | 403 on all three URLs tried, including both marked `readyToLicense: true` |
| token creation | `fixture-tested` | 500 Internal Server Error, no detail body |
| `fetch` (`/content`) | `fixture-tested` | never reached; blocked upstream at rate and token |
| `quote` on the demo publisher (`/rates`, 4 Aug 2026) | `fixture-tested` | the quickstart's own demo publisher (`pioneervalleygazette.com/daydream`) returns the identical problem document in both URL forms (`demo/recon/tollbit/tollbit-rates-demo-publisher.json`): the 403 is account-scoped, not per-page |
| `licensed` | **partial, and weaker than it looks** | publisher identity returned; no machine-readable licence reference ever obtained |
| `report` (`/transactions/selfReport`) | `spec-verified` only | schema read 2 Aug 2026: use counts and licence identity, no citation field (see above); deliberately not called, since posting a transaction that did not occur would corrupt supplier-side evidence |
| `corroborate` | **absent** | no request identifier in the search response body or headers |

**On the claim that TollBit accepts citation reporting from agents**
(spec-verified 2 August 2026 against TollBit's published OpenAPI document,
`github.com/tollbit/tollbit-python-sdk`, `spec/openapi.TollBit.Apis.yaml`):

The endpoint is real, but it reports **use counts, not citations**. The strings
`citation`, `cited` and `attribut` do not occur anywhere in the document. The
tag is "Report Content Usage".

`SelfReportUsage` requires `url`, `timesUsed` (int32), `licensePermissions[]`
and `licenseType`, with optional `licenseId` and a free-form
`metadata: {additionalProperties: {}}`. There is no field for whether content
was cited, which claim it supported, what answer it entered, or any span,
quote or influence measure. `timesUsed` is an integer counter.

The response shows what the endpoint is for: `SelfReportUsageReceipt`
returns `perUnitPriceMicros`, `totalUsePriceMicros`, `currency` and
`license`. `selfReport` is a settlement mechanism, not an attribution one:
the agent declares what it consumed and TollBit prices it. That is post-paid
metering with the buyer as the meter.

Two consequences:

- it is evidence *for* the neutrality position, not against it. A marketplace
  record of what an agent consumed, attested by the agent to an interested
  party, is exactly the conflicted-by-construction case, and a receipt priced
  by the counterparty makes the conflict explicit rather than incidental;
- the only route by which citation data could travel is the untyped `metadata`
  bag, by private convention. If TollBit describes citation reporting to a
  partner, the published contract does not carry it, and anything agreed there
  is bilateral rather than standardised.

Still not called, for the reason given in the table below. Verifying the
`metadata` convention would need TollBit to say what they put in it.

**Assumption register:** [`docs/knowledge-base/unverified-assumptions.md`](unverified-assumptions.md). U2
(the `TollbitKey` header) and U7 (search exists and is usable) are live-verified
by these probes. U3, U5 and U14 — licence vocabulary, price representation, and
whether a paid fetch needs funded billing — remain untested, and U14 is the live
question.

**No TollBit price has ever been observed.** The micro-prices in the existing
fixtures remain fixtures. The demo must not present them as measured.

The account-scoped 403 closes U16 and leaves U14 as the live question. The
unexercised prerequisite is an AgentID registered in the developer dashboard,
named in the token request's `userAgent`; the next step is to re-probe rates
on the demo publisher with it, then spend against a real one.

---

## Parallel — `live-verified` and `replay-tested` (search + fetch)

**Dates:** 18 August 2026 (first live calls) and 5 September 2026 (the
committed captures).

**Search:** `POST https://api.parallel.ai/v1/search` with `PARALLEL_API_KEY`
returns `200`, results carrying `excerpts`, `search_id` set, and `usage` =
`[{sku_search, 1}]`. The charge is observed in billing SKUs; no currency is
disclosed, so a job's USD cap is recorded as unenforceable for the plan.
Capture: `demo/recon/parallel/parallel-search.json` (200, 2,890 ms, three
results offered against `www.w3.org`).

**Fetch (Extract API):** `POST https://api.parallel.ai/v1/extract` retrieves
a named URL. Two outcomes are known, both `200`: a URL whose origin refuses
the fetch returns `results: []` with a per-URL `errors` entry (`http_error`,
`http_status_code: 403`) and empty `usage`, recorded as an acquisition with no
admissible content and no charge; a URL the origin serves returns a `results`
entry (`full_content`, `excerpts`, `publish_date`) billing
`sku_extract_excerpts`. `full_content` is often an empty string while
`excerpts` is populated, so the adapter falls back to the excerpts rather
than admit a textless source. Capture:
`demo/recon/parallel/parallel-extract.json` (extract of
`https://www.w3.org/TR/prov-dm/`, 200, 404 ms, one `sku_extract_excerpts`).

**Replay:** both captures are in `demo/recon/replay-manifest.json`
(`parallel`/`search`, `parallel`/`fetch`) and the jobs that made them,
`demo/jobs/recon-w3c-prov-search.json` and
`demo/jobs/recon-w3c-prov-fetch.json`, replay them with
`--replay demo/recon`.

**What was done to build it:** the search adapter
(`crates/commonmeasure-supply/src/parallel.rs`) was built to Parallel's published Search
API documentation, read this date:
`POST https://api.parallel.ai/v1/search` with `x-api-key` auth; a request
carrying `objective` and `search_queries`, and an optional
`advanced_settings.source_policy.include_domains` domain filter (bare
hostnames, `www.` normalised away, combined include/exclude ≤ 200); a response
of `search_id`, a `results` array of `{url, title, publish_date, excerpts}`,
and a `usage` array of `{name, count}` billing SKUs (`sku_search`). The parser
is exercised over these documented shapes through the real transport and a
loopback origin in `crates/commonmeasure-supply/tests/recorded_replay.rs`.

**Request fields not sent:** two documented request fields — the provider-side
result-count limit and the per-excerpt length cap — live on a Search "advanced
settings" page that could not be retrieved, so their field names are unverified
and deliberately not sent. The adapter instead caps the *offered* candidates to
the job's result limit itself and records that it did so; the sealed response
still carries everything Parallel returned. When those field names are
verified, the cap moves onto the request so the provider trims before billing.

---

## Tier-B open-web providers

**Date of the evidence:** 5 September 2026 for Linkup, Keenable, You.com and
Nimble: one search each, made by `demo/jobs/recon-w3c-prov-search.json` with
`--live`, committed under `demo/recon/<provider>/`, named in
`demo/recon/replay-manifest.json` and replayed by the same job with
`--replay demo/recon`. Search1API has no credential configured and SERPdive's
live calls fail, so neither has evidence in this repository beyond the
published documentation. Each adapter was built to its provider's published
Search API. Outcome of the 5 September calls: Linkup, You.com and Nimble
return three results for a three-result job; Keenable returns ten (it
documents no result-count field, so the adapter caps the offered
candidates); Nimble's longer query timed out at the 30 s exchange budget and
a shorter query answered; Keenable's single-domain `site` filter for
`www.w3.org` returned `dvcs.w3.org` URLs, which the job's source policy
refused at admission. Keenable requires a valid `keen_`-prefixed key and
refuses any other value as malformed (`401 "Malformed API key"`). Each
adapter's parser is also exercised over its provider's documented response
shapes through the real transport and a loopback origin
(`crates/commonmeasure-supply/tests/<provider>_spec.rs`). Where a provider discloses
no cost field on the search response but publishes a flat price, the charge
is recorded `Quoted` from that price, never `Observed`, never zero; where it
publishes no flat price either (Keenable), the charge is recorded unknown.

| Provider | Endpoint / auth | Query text field | Domain scoping | Charge | Search evidence |
|---|---|---|---|---|---|
| Linkup (`crates/commonmeasure-supply/src/linkup.rs`) | `POST /v1/search`, `Authorization: Bearer` | `q`, from `results[].content` | `includeDomains` (array) | `Quoted` $0.005/call | `demo/recon/linkup/linkup-search.json`, 5 Sep 2026, 200, 3,459 ms; `replay-tested` |
| Search1API (`crates/commonmeasure-supply/src/search1api.rs`) | `POST /search`, `Authorization: Bearer` | `query`, from `results[].content` then `snippet` | `include_sites` (array) | `Quoted` 1 credit | none: no `SEARCH1API_API_KEY` configured; `spec-verified`; a key restores the live check |
| SERPdive (`crates/commonmeasure-supply/src/serpdive.rs`) | `POST /v1/search`, `Authorization: Bearer` | `query`, from `results[].content` | none documented — unscoped, admission enforces | `Quoted` 1 `mako` credit | none: three calls on 5 Sep 2026 answered `502 search_failed`, not billed; `spec-verified` |
| Keenable (`crates/commonmeasure-supply/src/keenable.rs`) | `POST /v1/search`, `X-API-Key` | search text, from per-result `snippet` | `site` (single domain only) | unknown — no cost field, no published flat price | `demo/recon/keenable/keenable-search.json`, 5 Sep 2026, 200, 492 ms; `replay-tested` |
| You.com (`crates/commonmeasure-supply/src/you.rs`) | `POST /v1/search`, `X-API-Key` | `query`, from `results.web[].snippets` | `include_domains` (array) | `Quoted` USD 0.005/call | `demo/recon/you/you-search.json`, 5 Sep 2026, 200, 839 ms; `replay-tested` |
| Nimble (`crates/commonmeasure-supply/src/nimble.rs`) | `POST /v2/search`, `Authorization: Bearer` | `query`, from `results[].content` | `include_domains` (array) | `Quoted` 2 credits | `demo/recon/nimble/nimble-search.json`, 5 Sep 2026, 200, 1,128 ms; `replay-tested` |

**Fetch — Linkup `live-verified` and `replay-tested`; Search1API
`spec-verified`.** Both adapters implement the `fetch` capability. Linkup's
was called on 5 September 2026 against `https://www.w3.org/TR/prov-dm/`
(`demo/recon/linkup/linkup-fetch.json`, 200, 508 ms, page `markdown`, no
cost field; manifest entry `linkup`/`fetch`). Search1API's has no credential
configured and rests on the published `/crawl` documentation.

| Provider | Endpoint | Body text field | Charge | Fetch evidence |
|---|---|---|---|---|
| Linkup | `POST /v1/fetch` | `markdown` | unknown — no cost field, no per-fetch price read | `demo/recon/linkup/linkup-fetch.json`, 5 Sep 2026; `replay-tested` |
| Search1API | `POST /crawl` | `results.content` | `Quoted` 1 credit per page | none: no credential configured; `spec-verified` |

**Shared discipline.** Each adapter maps only fields its provider's docs locate
on the response: an absent per-result date is left unmapped, an absent
request-id leaves `provider_request_id` `None`, and empty result text is `None`
rather than an empty string. Domain scoping is wired to the job's allowed source
hosts and sent only when non-empty; SERPdive has no such filter and runs
unscoped, and Keenable's `site` filter takes a single domain, so a job naming
several runs unscoped — in both cases admission still enforces the host policy
over the returned sources. Where a provider documents no server-side result-count
field (Keenable), the adapter caps the offered candidates to the job's limit and
the sealed response still holds everything returned. Each adapter's module doc
names the exact documentation URLs read.

---

## TinyFish — `spec-verified` (search adapter, no credential)

**Date:** 25 August 2026.

**What was done:** an adapter for the Search API
(`crates/commonmeasure-supply/src/tinyfish.rs`) built to the published reference
(`https://docs.tinyfish.ai/search-api/reference.md`, read this date through the
mediated fetch, content hash `sha256:d8e0f2…`), with loopback spec tests
exercising both wire directions (`crates/commonmeasure-supply/tests/tinyfish_spec.rs`).
No `TINYFISH_API_KEY` is held: no live call has been made, no recon bytes
exist, and nothing here may be promoted past `spec-verified` until they do.

Obtaining a key means registering an account and accepting TinyFish's terms,
and that is recorded as an open product decision rather than a credential
chore: TinyFish's wider product line is marketed on defeating anti-bot
protections and login walls, so whether this operator wants a commercial
relationship with the supplier at all is a consent-posture question the
supply-map entry ([`docs/knowledge-base/supply-map.md`](supply-map.md) §TinyFish) puts on the
record first.

Wire summary, all of it from the published reference: `GET` on
`https://api.search.tinyfish.ai` with parameters in the query string and the
key in an `X-API-Key` header; `include_domains` (comma-separated) carries the
job's whole allowed-host set; no result-count field is documented, so the
adapter caps the offered candidates to the job's limit itself; the response
(`query`, `results[{position, site_name, title, snippet, url, date?}]`,
`total_results`, `page`) carries no cost field and no request identifier, so
the charge is `Quoted` USD 0 from the published price ("Search requests are
free at any wallet balance", with account-level access still required — `402`
otherwise) and `provider_request_id` stays `None`. Text comes from `snippet`;
`date` maps to the declared date where present. The documented optional
`purpose` parameter — operator intent traded for "better-quality results" — is
deliberately never sent. Open assumptions are registered in
[`docs/knowledge-base/unverified-assumptions.md`](unverified-assumptions.md) §TinyFish.

---

## Ozone Live — `spec-verified` (search + fetch)

**Date:** 9 September 2026.

**What was done:** an adapter for the search and contents endpoints
(`crates/commonmeasure-supply/src/ozone.rs`) built to the machine-readable
specification (`https://api.ozone.live/openapi.yaml`, read this date through
the mediated fetch, content hash `sha256:92936f…`) and the API reference, with
loopback spec tests exercising both wire directions for search, fetch, the
error envelope and the `top_k` ceiling
(`crates/commonmeasure-supply/tests/ozone_spec.rs`). An `OZONE_LIVE_API_KEY`
is held. One live search was made this date through the real adapter
(`commonmeasure run … --live`, one plan, `top_k` 3, no filters): `200`,
2,378 ms, three article rows, each with a 1,000-character passage, a
`publisher_id`, `licensed: true` and a `published_date`; `usage` reported 13
embedding tokens and no charge; no request identifier. The sealed bytes are
not committed and no recon capture directory exists for Ozone: the passages are
licensed publisher content, which this repository never commits, and the
elision the other captures use would still retain part of each passage. The
state therefore stays `spec-verified`, with the live observations below
recorded as observations and not as evidence anyone can replay.

What the live call showed, none of it replayable from this repository:
`publisher_name` was empty on all three rows although each carried a
`publisher_id` (the manifest resolution the reference describes did not
fire); the `domain` field disagreed with the URL on one row (a
`chroniclelive.co.uk` URL carried `domain: mirror.co.uk`), so the adapter's
host is taken from the URL and never from `domain`; one `published_date`
matched the date in the article's own URL, and one 2019 general-election
article carried `published_date: 2026-04-29`, which is the doubt registered
as OZ1.

**Supply class.** Ozone Live is retrieval over a corpus the Ozone Project
licenses from its member publishers, exposed in the shape of an open-web
grounding API (`POST /v1/search` returns passages with a relevance score),
with an ownership manifest (`GET /v1/publishers/domains`, a flat domain to
publisher-id map) alongside. It is the second licensed supplier in the set
and the first with no quote gate: nothing prices a call before it is made,
and no response carries a charge. It calls no model and generates no answer;
`include_answer`-style parameters are rejected with `400`.

Wire summary, all of it from the specification: `POST /v1/search` with the
key as a bearer token; `query` (1–4000 characters); `top_k` (1–100, default 3)
carries the job's limit, clamped to the ceiling the adapter declares in
`maximum_search_results`; `group_by: article` is sent so one row is one
document, with the best passage as the row's `text` and the rest nested under
`additional_chunks` (kept in native metadata); `filters.domains` carries the
job's whole allowed-host set with `mode: include`; no `recency` window is
sent. The response (`query`, `provider`, `group_by`, `results[]`, `usage`,
`date_filter` when a window was requested) carries no request identifier and
no cost field, and no price is published, so the charge is unknown.
`published_date` maps to the declared date. `POST /v1/contents` with one URL
is the `fetch` capability: a corpus article is served from licensed storage
(`source: gcs_markdown`, `licensed: true`); any other URL falls back to a live
fetch by Ozone's fetcher (`source: live_fetch`, `licensed: false`); a per-item
failure (`ok: false`) yields no envelope and stays sealed.

**Licence.** Every search result carries `licensed: true` and a
`publisher_id`, with `publisher_name` resolved from the ownership manifest.
This is the most rights metadata any provider in the set returns, and it is
still not a licence reference: the boolean says the passage came from
Ozone's licensed corpus, and the identifier names who owns the page. Neither
names an agreement or states what the operator may do with the text, so the
envelope stays `unknown`, the fields stay in native metadata, and the
adapter declares no `licensed` capability. A `require_licence` rule cannot
match Ozone supply today. The way to change that is for the operator to
declare the agreement they hold with Ozone as a reference, in configuration
with operator provenance, as `corpus.json` does for the internal corpus; the
adapter must not invent one.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| `search` | `spec-verified` | specification and reference; loopback tests over the documented shape; no live call recorded |
| `fetch` (`/v1/contents`) | `spec-verified` | specification; loopback tests over the documented per-item outcomes; no live call recorded |
| `quote` | **absent** | no pre-delivery price endpoint exists and no price is published |
| `licensed` | **absent** | `licensed: true` and `publisher_id` on every result; neither is a machine-readable licence or agreement reference |
| `report`, `corroborate`, `query` | not exercised | no endpoint identified |

`GET /v1/publishers/domains` (the ownership map), `POST /v1/similar` and
`GET /v1/capabilities` are documented and not wired: none is an acquisition,
and the ownership map's consumer is a routing decision the runtime does not
yet take. One authenticated call to `GET /v1/publishers` on 9 September 2026
answered `503 upstream_unavailable` ("Publisher ownership data has not been
synced yet"), so the ownership map has not been observed and the hub's
network import has been exercised only on a map of the documented shape
(in the hub's own repository). Open assumptions are registered in
[`docs/knowledge-base/unverified-assumptions.md`](unverified-assumptions.md) §Ozone Live.

---

## Redpine — `live-verified` (MCP), first licensed fetch

**Date:** 4 August 2026, the day the API key arrived.

**What was done:** twelve authenticated JSON-RPC requests to
`https://api.redpine.ai/mcp` (server `redpine-connect`, Streamable HTTP,
session id issued by `initialize`): discovery (`tools/list`, `find-tools`,
`inspect-tool`), one `preview` → `confirm` cycle on `media--sample`, and
`get_balance` before and after.

**Artefacts:** `demo/recon/redpine/*.json`, `demo/recon/redpine/NOTES.md`.
The confirmed capture truncates the licensed transcript text; permitted use
of fetched content is not machine-readable in any response.

**Per operation:**

| Operation | State | Evidence |
|---|---|---|
| discovery | `live-verified` | six meta-tools; per-tool schemas with pricing annotations (`cost_credits: 0.03, cost_type: "exact"` on `media--sample`) |
| `quote` (`preview`) | `live-verified` | free, executes the tool, returns teaser + `preview_id` + a billing note naming what confirm consumes |
| `fetch` (`confirm`) | `live-verified` under trial | three licensed mentions returned (Redpine × AllEars); `cost_charged: "0.000000"`, trial meter moved 5→4 |
| charge observation | **not achieved** | the trial is a meter, not a price; `_meta.cost_usd: 0.0015` disagrees with the $0.03 annotation and neither is an observed charge |
| REST `search/query` | not exercised | needs a collection name; nothing in the MCP surface named one |

This is the first licensed content fetch against any provider: content was
delivered under an identified licence on a trial meter, and no currency charge
was observed.

**Adapter (6 August 2026):** `RedpineAdapter` (`crates/commonmeasure-supply/src/redpine.rs`)
speaks the recorded path — `initialize` and session id, `get_balance`,
`inspect-tool`, `preview`, `confirm` — and declares `search` and `quote`,
composing as search behind the quote gate. `replay-tested` end to end: the
loopback tests (`crates/commonmeasure-supply/tests/redpine_quote_replay.rs`,
`crates/commonmeasure-runtime/tests/quoted_purchase.rs`) and the committed replay run
`demo/output/redpine-replay/` serve the five recorded legs through the real
transport, parser and policy, with the quote in evidence before the purchase
decision and every sealed exchange bound to its recording by hash
(`demo/recon/replay-manifest.json`). `licensed`, `fetch` and `corroborate`
are deliberately absent: permitted use is not machine-readable (register
R4), `media--get_mention` was never exercised, and the receipt rides the
confirm response in-band.

**Adapter live run (6 August 2026):** `live-verified` through the whole run
contract — run `b8f0bcf2`, gitignored `demo/output/live-redpine/`,
pre-registered in `demo/recon/redpine/NOTES.md` before any call. One trial
query spent, exactly as budgeted: the quote observed the meter at 4 of 5
with a correctly re-counted billing note, the purchase was confirmed under
trial coverage, the receipt recorded `cost_charged: "0.000000"` and
`balance_remaining: "0.00"` while the meter moved 4 → 3, three licensed
mentions were admitted with supplier-declared dates and explicitly unknown
licences, and gateway inference answered over the admitted window. The
pricing annotation and `_meta.cost_usd` were returned again unchanged,
still twenty-fold apart (register R1: trial-observed; funded price open).
Three trial queries remain.

## Local skills — `live-verified` for `invoke`

Executed 6 August 2026 against the two bundles installed as Codex system
skills, both offline and credential-free, both receiving identical input
bytes. The run is `demo/output/skills/`; the catalogue that admitted them is
`demo/skills/catalogue.json`.

| Skill | Entrypoint | Observed |
|---|---|---|
| `skill-creator` | `scripts/quick_validate.py` | exit 1, 128 bytes on stdout: the frontmatter fault named, with the keys the format allows |
| `plugin-creator` | `scripts/validate_plugin.py` | exit 1, 64 bytes on stdout: the missing `.codex-plugin/plugin.json` named |

| Operation | State | Evidence |
|---|---|---|
| `invoke` | `live-verified` | both bundles executed, results sealed with their digests, exit statuses recorded as results |
| `search`, `query`, `quote`, `fetch`, `licensed`, `report`, `corroborate` | **not declared** | a skill adapter implements `invoke` alone; the others refuse by contract, not by accident |
| charge observation | **not applicable, not missing** | no supplier prices a local execution; unknown, never zero |
| date | **not applicable** | nothing published a produced result |
| licence | declared for the *procedure* where a bundle states one; neither of these does | the output's rights are stated nowhere, so every source reads unknown |

The standing assumptions a supervised spawn cannot close are S1–S8 in
[`docs/knowledge-base/unverified-assumptions.md`](unverified-assumptions.md).

## Providers not tested

`planned` in every case, with no implementation claim.

- **Cashmere** — public access requires approval, which would mean creating an
  account and accepting terms. Explicitly out of scope for this workstream.
- **Channel3** — no credential.
- **Supertab** — added to scope 4 August 2026. No credential; public docs read
  the same day cover the licence-token flow (OLP/CAP, RSL licences) and SDKs
  but not metering, budget or settlement endpoints, so the tab/settlement
  mechanics in the supply map rest on vendor claims. A founder relationship
  exists; a design conversation is available before any credentialled probe.

---

## What could not be verified

1. **A licensed content fetch at an observed price.** Redpine delivered
   content under an identified licence on a trial meter; no provider in this
   set has charged an observed currency amount for licensed content. The
   priced half of the licensed-supply thesis is unexercised against a live
   API.

2. **TollBit's price representation, licence vocabulary and paid-fetch
   preconditions.** Blocked by a uniform 403 at `/rates` and a 500 at token
   creation. Whether this account simply has no publisher agreements, or those
   specific pages are disallowed, cannot be distinguished from the evidence
   gathered.

3. **Whether TollBit's `availability.readyToLicense` ever predicts a successful
   rate lookup.** It did not in two of two attempts. Two is a small sample and
   the claim here is only that the flag cannot be relied upon, not that it is
   always wrong.

4. **Tavily's actual metered consumption.** Counters did not move within the
   observation window. Not concluded broken; concluded unobservable within the
   window watched.

5. **Whether Exa's bundled contents are genuinely free.** One observation
   showed text for two results arriving without a surcharge on the published
   search price. One observation is not a pricing rule.

6. **Exa's payment headers.** `PAYMENT-REQUIRED`, `PAYMENT-RESPONSE` and
   `Payment-Receipt` appear in `access-control-expose-headers`. The
   documentation URL for this returns 404. A `Payment-Receipt` would bear
   directly on the `corroborate` capability, so this is worth a follow-up, but
   nothing is claimed from a header name.

7. **Rate limits and failure modes under load** for every provider. A few
   requests each says nothing about sustained behaviour. This is the gap
   between `live-verified` and `production-observed`.

8. **Cashmere and Channel3 in their entirety.**

9. **SERPdive's live search.** Three calls on 5 September 2026, with three
   different queries, each answered `502 search_failed` ("The search returned
   no usable sources. You were not billed"). Whether the failure is
   query-specific or account-level cannot be told from three calls.

10. **Search1API's live behaviour.** No credential is configured; nothing
    beyond the published documentation is claimed.
